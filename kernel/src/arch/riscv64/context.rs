//! Kernel context switching for RISC-V 64-bit
//!
//! This module implements kernel-level context switching between tasks.
//! It handles saving and restoring callee-saved registers when switching
//! between kernel threads.

use core::arch::naked_asm;
use alloc::boxed::Box;

use crate::arch::Trapframe;
use crate::vm::vmem::MemoryArea;

/// Guarded kernel stack
/// 
/// A kernel stack with a virtual guard page to detect stack overflow.
/// The guard page is a virtual address space reservation only - no physical
/// memory is allocated for it.
#[derive(Debug, Clone)]
pub struct GuardedKernelStack {
    /// The actual kernel stack (physical memory allocated)
    stack: Box<[u8]>,
    /// Virtual guard page start address (no physical memory)
    guard_page_start: usize,
}

impl GuardedKernelStack {
    /// Create a new guarded kernel stack
    /// 
    /// Allocates a continuous memory block that includes both the guard page space
    /// and the actual stack. The first page is reserved as guard, followed by the usable stack.
    pub fn new(size: usize) -> Self {
        use crate::environment::PAGE_SIZE;
        
        // Allocate guard page space + actual stack as one continuous block
        let total_size = PAGE_SIZE + size;
        let stack = alloc::vec![0u8; total_size].into_boxed_slice();
        
        // The first PAGE_SIZE bytes are the guard page
        let guard_page_start = stack.as_ptr() as usize;
        
        Self {
            stack,
            guard_page_start,
        }
    }
    
    /// Get the stack bottom (top address, where stack grows down from)
    pub fn stack_bottom(&self) -> usize {
        self.stack.as_ptr() as usize + self.stack.len()
    }
    
    /// Get the stack start address (after the guard page)
    pub fn stack_start(&self) -> usize {
        use crate::environment::PAGE_SIZE;
        self.stack.as_ptr() as usize + PAGE_SIZE
    }
    
    /// Get the usable stack size in bytes (excluding guard page)
    pub fn stack_size(&self) -> usize {
        use crate::environment::PAGE_SIZE;
        self.stack.len() - PAGE_SIZE
    }
    
    /// Get the guard page start address
    pub fn guard_page_start(&self) -> usize {
        self.guard_page_start
    }
    
    /// Get the guard page memory area
    pub fn guard_page_area(&self) -> MemoryArea {
        use crate::environment::PAGE_SIZE;
        MemoryArea::new(self.guard_page_start, self.guard_page_start + PAGE_SIZE - 1)
    }
    
    /// Get the stack memory area (excluding guard page)
    pub fn stack_area(&self) -> MemoryArea {
        use crate::environment::PAGE_SIZE;
        let stack_start = self.stack_start();
        MemoryArea::new(stack_start, self.stack_bottom() - 1)
    }
    
    /// Get a pointer to the usable stack data (after guard page)
    pub fn as_ptr(&self) -> *const u8 {
        use crate::environment::PAGE_SIZE;
        unsafe { self.stack.as_ptr().add(PAGE_SIZE) }
    }
}

/// Kernel context for RISC-V 64-bit
/// 
/// Contains callee-saved registers that need to be preserved across
/// function calls and context switches in kernel mode, as well as
/// the kernel stack information.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct KernelContext {
    /// Stack pointer
    pub sp: u64,
    /// Return address
    pub ra: u64,
    /// Saved registers s0-s11 (callee-saved)
    pub s: [u64; 12],
    /// Guarded kernel stack for this context
    pub kernel_stack: GuardedKernelStack,
}

impl KernelContext {
    /// Create a new kernel context with guarded kernel stack
    /// 
    /// # Returns
    /// A new KernelContext with allocated kernel stack ready for scheduling
    pub fn new() -> Self {
        use crate::environment::TASK_KERNEL_STACK_SIZE;
        
        let kernel_stack = GuardedKernelStack::new(TASK_KERNEL_STACK_SIZE);
        let stack_top = kernel_stack.stack_bottom();

        Self {
            sp: stack_top as u64 - core::mem::size_of::<Trapframe>() as u64, // Reserve space for trapframe
            ra: crate::task::task_initial_kernel_entrypoint as u64,
            s: [0; 12],
            kernel_stack,
        }
    }

    /// Get the bottom of the kernel stack
    pub fn get_kernel_stack_bottom(&self) -> u64 {
        self.kernel_stack.stack_bottom() as u64
    }

    pub fn get_kernel_stack_memory_area(&self) -> MemoryArea {
        self.kernel_stack.stack_area()
    }
    
    /// Get the guard page memory area (virtual address space only)
    pub fn get_guard_page_memory_area(&self) -> MemoryArea {
        self.kernel_stack.guard_page_area()
    }
    
    /// Get the guard page start address for cleanup
    pub fn guard_page_start(&self) -> usize {
        self.kernel_stack.guard_page_start()
    }

    pub fn get_kernel_stack_ptr(&self) -> *const u8 {
        self.kernel_stack.as_ptr()
    }

    /// Set the kernel stack for this context
    /// # Arguments
    /// * `stack` - GuardedKernelStack to use
    /// 
    pub fn set_kernel_stack(&mut self, stack: GuardedKernelStack) {
        self.sp = stack.stack_bottom() as u64;
        self.kernel_stack = stack;
    }

    /// Set entry point for this context
    /// 
    /// # Arguments
    /// * `entry_point` - Function address to set as entry point
    /// 
    pub fn set_entry_point(&mut self, entry_point: u64) {
        self.ra = entry_point;
    }

    /// Get entry point of this context
    /// 
    /// # Returns
    /// 
    /// Function address of the entry point
    pub fn get_entry_point(&self) -> u64 {
        self.ra
    }

    /// Get a mutable reference to the trapframe
    /// 
    /// The trapframe is located at the top of the kernel stack, reserved during
    /// context creation. This provides access to the user-space register state.
    /// 
    /// # Returns
    /// A mutable reference to the Trapframe, or None if no kernel stack is allocated
    pub fn get_trapframe(&mut self) -> &mut Trapframe {
        let stack_top = self.kernel_stack.stack_bottom();
        let trapframe_addr = stack_top - core::mem::size_of::<Trapframe>();
        unsafe {
            &mut *(trapframe_addr as *mut Trapframe)
        }
    }
}

/// Switch from current context to target context
/// 
/// This function saves the current kernel context and loads the target context.
/// When the target task is later switched away from, it will resume execution
/// right after this function call.
/// 
/// # Arguments
/// * `current` - Pointer to current task's kernel context (will be saved)
/// * `target` - Pointer to target task's kernel context (will be loaded)
/// 
/// # Safety
/// This function manipulates CPU registers directly and must only be called
/// with valid context pointers. The caller must ensure proper stack alignment
/// and that both contexts point to valid memory.
#[unsafe(naked)]
pub unsafe extern "C" fn switch_to(current: *mut KernelContext, target: *const KernelContext) {
    
    naked_asm!(
        // Save current context
        "sd sp, 0(a0)",      // Save stack pointer
        "sd ra, 8(a0)",      // Save return address
        "sd s0, 16(a0)",     // Save s0
        "sd s1, 24(a0)",     // Save s1
        "sd s2, 32(a0)",     // Save s2
        "sd s3, 40(a0)",     // Save s3
        "sd s4, 48(a0)",     // Save s4
        "sd s5, 56(a0)",     // Save s5
        "sd s6, 64(a0)",     // Save s6
        "sd s7, 72(a0)",     // Save s7
        "sd s8, 80(a0)",     // Save s8
        "sd s9, 88(a0)",     // Save s9
        "sd s10, 96(a0)",    // Save s10
        "sd s11, 104(a0)",   // Save s11
        
        // Load target context
        "ld sp, 0(a1)",      // Load stack pointer
        "ld ra, 8(a1)",      // Load return address
        "ld s0, 16(a1)",     // Load s0
        "ld s1, 24(a1)",     // Load s1
        "ld s2, 32(a1)",     // Load s2
        "ld s3, 40(a1)",     // Load s3
        "ld s4, 48(a1)",     // Load s4
        "ld s5, 56(a1)",     // Load s5
        "ld s6, 64(a1)",     // Load s6
        "ld s7, 72(a1)",     // Load s7
        "ld s8, 80(a1)",     // Load s8
        "ld s9, 88(a1)",     // Load s9
        "ld s10, 96(a1)",    // Load s10
        "ld s11, 104(a1)",   // Load s11
        
        // Return to target context
        "ret",
    );
}
