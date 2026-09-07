//! Kernel context switching for AArch64
//!
//! This module implements kernel-level context switching between tasks.
//! It handles saving and restoring callee-saved registers when switching
//! between kernel threads.

use alloc::boxed::Box;
use core::arch::naked_asm;

use crate::arch::Trapframe;
use crate::mem::page::{ContiguousPages, Page};
use crate::vm::vmem::MemoryArea;

/// Kernel context for AArch64
///
/// Contains callee-saved registers that need to be preserved across
/// function calls and context switches in kernel mode, as well as
/// the kernel stack information.
#[repr(C)]
#[derive(Debug)]
pub struct KernelContext {
    /// Stack pointer (SP)
    pub sp: u64,
    /// Link register (LR/X30)
    pub lr: u64,
    /// Frame pointer (FP/X29)
    pub fp: u64,
    /// Callee-saved registers X19-X28
    pub x: [u64; 10],
    /// Kernel stack for this context (page-aligned, allocated from PMM)
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
        let stack_top = kernel_stack.as_ptr() as u64
            + (kernel_stack.len() * crate::environment::PAGE_SIZE) as u64;

        Self {
            sp: stack_top - core::mem::size_of::<Trapframe>() as u64, // Reserve space for trapframe
            lr: crate::task::task_initial_kernel_entrypoint as *const () as u64,
            fp: 0,
            x: [0; 10],
            kernel_stack,
        }
    }

    /// Get the bottom of the kernel stack
    pub fn get_kernel_stack_bottom_paddr(&self) -> u64 {
        (self.kernel_stack.as_ptr() as u64)
            + (self.kernel_stack.len() as u64 * crate::environment::PAGE_SIZE as u64)
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

    /// Set stack pointer for this context (VA)
    pub fn set_sp(&mut self, sp_vaddr: u64) {
        self.sp = sp_vaddr;
    }

    /// Set entry point for this context
    ///
    /// # Arguments
    /// * `entry_point` - Function address to set as entry point
    ///
    pub fn set_entry_point(&mut self, entry_point: u64) {
        self.lr = entry_point;
    }

    /// Get entry point of this context
    ///
    /// # Returns
    ///
    /// Function address of the entry point
    pub fn get_entry_point(&self) -> u64 {
        self.lr
    }

    /// Get a mutable reference to the trapframe
    ///
    /// The trapframe is located at the top of the kernel stack, reserved during
    /// context creation. This provides access to the user-space register state.
    ///
    /// # Returns
    /// A mutable reference to the Trapframe
    pub fn get_trapframe(&mut self) -> &mut Trapframe {
        let stack_top = self.kernel_stack.as_ptr() as usize + self.kernel_stack.len();
        let trapframe_addr = stack_top - core::mem::size_of::<Trapframe>();
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
    naked_asm!(
        // Save current context
        // Layout: sp(0x00), lr(0x08), fp(0x10), x[0..9](0x18..0x60)
        "str x30, [x0, #8]",  // Save link register (lr)
        "str x29, [x0, #16]", // Save frame pointer (fp)
        "mov x9, sp",
        "str x9, [x0, #0]",        // Save stack pointer
        "stp x19, x20, [x0, #24]", // Save x19, x20 at offset 0x18
        "stp x21, x22, [x0, #40]", // Save x21, x22
        "stp x23, x24, [x0, #56]", // Save x23, x24
        "stp x25, x26, [x0, #72]", // Save x25, x26
        "stp x27, x28, [x0, #88]", // Save x27, x28
        // Load target context
        "ldr x30, [x1, #8]",  // Load link register (lr)
        "ldr x29, [x1, #16]", // Load frame pointer (fp)
        "ldr x9, [x1, #0]",   // Load stack pointer
        "mov sp, x9",
        "ldp x19, x20, [x1, #24]", // Load x19, x20
        "ldp x21, x22, [x1, #40]", // Load x21, x22
        "ldp x23, x24, [x1, #56]", // Load x23, x24
        "ldp x25, x26, [x1, #72]", // Load x25, x26
        "ldp x27, x28, [x1, #88]", // Load x27, x28
        // Return to target context
        "ret",
    );
}
