//! Kernel context switching for RISC-V
//!
//! This module implements kernel-level context switching between tasks.
//! It handles saving and restoring callee-saved registers when switching
//! between kernel threads.

use crate::arch::Trapframe;
use crate::mem::page::ContiguousPages;
use crate::vm::vmem::MemoryArea;

/// Kernel context for RISC-V
///
/// Contains callee-saved registers that need to be preserved across
/// function calls and context switches in kernel mode, as well as
/// the kernel stack information.
#[repr(C, align(16))]
#[derive(Debug)]
pub struct KernelContext {
    pub sp: usize,
    pub ra: usize,
    pub s: [usize; 12],
    pub kernel_stack: ContiguousPages,
}

impl KernelContext {
    /// Create a new kernel context with kernel stack
    ///
    /// # Returns
    /// A new KernelContext with allocated kernel stack ready for scheduling
    pub fn new() -> Self {
        // Allocate page-aligned kernel stack from PMM
        let num_pages = crate::environment::TASK_KERNEL_STACK_SIZE / crate::environment::PAGE_SIZE;
        let kernel_stack =
            ContiguousPages::new(num_pages).expect("Failed to allocate kernel stack");
        let stack_top =
            kernel_stack.as_ptr() as usize + kernel_stack.len() * crate::environment::PAGE_SIZE;

        let trapframe_size = core::mem::size_of::<Trapframe>();
        let trapframe_align = core::mem::align_of::<Trapframe>();
        debug_assert!(trapframe_align.is_power_of_two());
        let trapframe_addr = (stack_top - trapframe_size) & !(trapframe_align - 1);

        Self {
            sp: trapframe_addr,
            ra: crate::task::task_initial_kernel_entrypoint as usize,
            s: [0; 12],
            kernel_stack,
        }
    }

    /// Get the bottom of the kernel stack
    pub fn get_kernel_stack_top(&self) -> usize {
        self.kernel_stack.as_ptr() as usize
            + self.kernel_stack.len() * crate::environment::PAGE_SIZE
    }

    pub fn get_kernel_stack_memory_area_paddr(&self) -> MemoryArea {
        MemoryArea::new(
            self.kernel_stack.as_paddr(),
            self.kernel_stack.as_paddr()
                + (self.kernel_stack.len() * crate::environment::PAGE_SIZE)
                - 1,
        )
    }

    pub fn get_kernel_stack_paddr(&self) -> *const u8 {
        self.kernel_stack.as_ptr() as *const u8
    }

    /// Set entry point for this context
    ///
    /// # Arguments
    /// * `entry_point` - Function address to set as entry point
    ///
    pub fn set_entry_point(&mut self, entry_point: u64) {
        self.ra = usize::try_from(entry_point).expect("kernel entry exceeds XLEN");
    }

    /// Get entry point of this context
    ///
    /// # Returns
    ///
    /// Function address of the entry point
    pub fn get_entry_point(&self) -> u64 {
        self.ra as u64
    }

    // Set stack pointer for this context (VA)
    pub fn set_sp(&mut self, sp_vaddr: u64) {
        self.sp = usize::try_from(sp_vaddr).expect("kernel stack exceeds XLEN");
    }

    /// Get a mutable reference to the trapframe
    ///
    /// The trapframe is located at the top of the kernel stack, reserved during
    /// context creation. This provides access to the user-space register state.
    ///
    /// # Returns
    /// A mutable reference to the Trapframe, or None if no kernel stack is allocated
    pub fn get_trapframe(&mut self) -> &mut Trapframe {
        let stack_top = self.kernel_stack.as_ptr() as usize
            + (self.kernel_stack.len() * crate::environment::PAGE_SIZE);
        let trapframe_size = core::mem::size_of::<Trapframe>();
        let trapframe_align = core::mem::align_of::<Trapframe>();
        debug_assert!(trapframe_align.is_power_of_two());

        let trapframe_addr = (stack_top - trapframe_size) & !(trapframe_align - 1);
        debug_assert_eq!(trapframe_addr % trapframe_align, 0);
        unsafe { &mut *(trapframe_addr as *mut Trapframe) }
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
    riscv_naked_asm!(
        // Save current context
        "SCARLET_S sp, 0*SCARLET_WORD_BYTES(a0)", // Save stack pointer
        "SCARLET_S ra, 1*SCARLET_WORD_BYTES(a0)", // Save return address
        "SCARLET_S s0, 2*SCARLET_WORD_BYTES(a0)", // Save s0
        "SCARLET_S s1, 3*SCARLET_WORD_BYTES(a0)", // Save s1
        "SCARLET_S s2, 4*SCARLET_WORD_BYTES(a0)", // Save s2
        "SCARLET_S s3, 5*SCARLET_WORD_BYTES(a0)", // Save s3
        "SCARLET_S s4, 6*SCARLET_WORD_BYTES(a0)", // Save s4
        "SCARLET_S s5, 7*SCARLET_WORD_BYTES(a0)", // Save s5
        "SCARLET_S s6, 8*SCARLET_WORD_BYTES(a0)", // Save s6
        "SCARLET_S s7, 9*SCARLET_WORD_BYTES(a0)", // Save s7
        "SCARLET_S s8, 10*SCARLET_WORD_BYTES(a0)", // Save s8
        "SCARLET_S s9, 11*SCARLET_WORD_BYTES(a0)", // Save s9
        "SCARLET_S s10, 12*SCARLET_WORD_BYTES(a0)", // Save s10
        "SCARLET_S s11, 13*SCARLET_WORD_BYTES(a0)", // Save s11
        // Load target context
        "SCARLET_L sp, 0*SCARLET_WORD_BYTES(a1)", // Load stack pointer
        "SCARLET_L ra, 1*SCARLET_WORD_BYTES(a1)", // Load return address
        "SCARLET_L s0, 2*SCARLET_WORD_BYTES(a1)", // Load s0
        "SCARLET_L s1, 3*SCARLET_WORD_BYTES(a1)", // Load s1
        "SCARLET_L s2, 4*SCARLET_WORD_BYTES(a1)", // Load s2
        "SCARLET_L s3, 5*SCARLET_WORD_BYTES(a1)", // Load s3
        "SCARLET_L s4, 6*SCARLET_WORD_BYTES(a1)", // Load s4
        "SCARLET_L s5, 7*SCARLET_WORD_BYTES(a1)", // Load s5
        "SCARLET_L s6, 8*SCARLET_WORD_BYTES(a1)", // Load s6
        "SCARLET_L s7, 9*SCARLET_WORD_BYTES(a1)", // Load s7
        "SCARLET_L s8, 10*SCARLET_WORD_BYTES(a1)", // Load s8
        "SCARLET_L s9, 11*SCARLET_WORD_BYTES(a1)", // Load s9
        "SCARLET_L s10, 12*SCARLET_WORD_BYTES(a1)", // Load s10
        "SCARLET_L s11, 13*SCARLET_WORD_BYTES(a1)", // Load s11
        // Return to target context
        "ret",
    );
}
