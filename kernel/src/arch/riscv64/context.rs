//! Kernel context switching for RISC-V 64-bit
//!
//! This module implements kernel-level context switching between tasks.
//! It handles saving and restoring callee-saved registers when switching
//! between kernel threads.

use core::arch::naked_asm;
use alloc::boxed::Box;
use alloc::vec;

use crate::arch::trap::user;
use crate::vm::vmem::MemoryArea;

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
    /// Kernel stack for this context (None = uninitialized)
    /// Using Box<[u8]> to directly allocate on heap without stack overflow
    pub kernel_stack: Option<Box<[u8]>>,
}

impl KernelContext {
    /// Create a new kernel context with kernel stack
    /// 
    /// # Returns
    /// A new KernelContext with allocated kernel stack ready for scheduling
    pub fn new() -> Self {
        // Directly allocate on heap to avoid stack overflow
        let kernel_stack = alloc::vec![0u8; crate::environment::TASK_KERNEL_STACK_SIZE].into_boxed_slice();
        let stack_top = kernel_stack.as_ptr() as u64 + kernel_stack.len() as u64; // Initial stack top = stack bottom

        Self {
            sp: stack_top,
            ra: crate::task::task_initial_kernel_entrypoint as u64,
            s: [0; 12],
            kernel_stack: Some(kernel_stack),
        }
    }

    /// Get the bottom of the kernel stack
    pub fn get_kernel_stack_bottom(&self) -> u64 {
        match &self.kernel_stack {
            Some(stack) => stack.as_ptr() as u64 + stack.len() as u64,
            None => 0,
        }
    }

    pub fn get_kernel_stack_memory_area(&self) -> Option<MemoryArea> {
        match &self.kernel_stack {
            Some(stack) => Some(MemoryArea::new(stack.as_ptr() as usize, self.get_kernel_stack_bottom() as usize - 1)),
            None => None,
        }
    }

    pub fn get_kernel_stack_ptr(&self) -> Option<*const u8> {
        match &self.kernel_stack {
            Some(stack) => Some(stack.as_ptr()),
            None => None,
        }
    }

    /// Set the kernel stack for this context
    /// # Arguments
    /// * `stack` - Boxed slice representing the kernel stack memory
    /// 
    pub fn set_kernel_stack(&mut self, stack: Box<[u8]>) {
        self.kernel_stack = Some(stack);
        self.sp = self.get_kernel_stack_bottom();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_kernel_context_new() {
        let entry_point = 0x2000;
        
        let ctx = KernelContext::new(entry_point);
        
        assert_eq!(ctx.ra, entry_point);
        assert_eq!(ctx.s, [0; 12]);
        // Stack bottom should be non-zero and aligned
        assert!(ctx.get_kernel_stack_bottom() > 0);
    }
}
