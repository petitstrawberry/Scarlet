//! RISC-V kernel context switching implementation
//!
//! This module provides low-level context switching functionality for RISC-V,
//! enabling kernel tasks to yield execution and resume later at the same point.

use crate::arch::context::KernelContext;

/// Switch from the current kernel context to the next kernel context
///
/// This function performs a complete kernel context switch:
/// 1. Saves callee-saved registers (sp, ra, s0-s11) to prev_ctx
/// 2. Restores callee-saved registers from next_ctx
/// 3. Returns to the point where next_ctx was previously switched out
///
/// # Arguments
/// * `prev_ctx` - Mutable reference to store the current context
/// * `next_ctx` - Reference to the context to switch to
///
/// # Safety
/// This function must only be called from kernel code with valid contexts.
/// The stack pointers in both contexts must point to valid, allocated stacks.
///
/// # Returns
/// This function returns twice:
/// - Once immediately (when switching away from this context)
/// - Once when this context is resumed later
#[unsafe(naked)]
pub unsafe extern "C" fn switch_to(prev_ctx: *mut KernelContext, next_ctx: *const KernelContext) {
    riscv_naked_asm!(
        // Save current context (prev_ctx)
        // a0 = prev_ctx, a1 = next_ctx

        // Save stack pointer
        "SCARLET_S sp, 0*SCARLET_WORD_BYTES(a0)",
        // Save return address
        "SCARLET_S ra, 1*SCARLET_WORD_BYTES(a0)",
        // Save callee-saved registers s0-s11
        "SCARLET_S s0, 2*SCARLET_WORD_BYTES(a0)",
        "SCARLET_S s1, 3*SCARLET_WORD_BYTES(a0)",
        "SCARLET_S s2, 4*SCARLET_WORD_BYTES(a0)",
        "SCARLET_S s3, 5*SCARLET_WORD_BYTES(a0)",
        "SCARLET_S s4, 6*SCARLET_WORD_BYTES(a0)",
        "SCARLET_S s5, 7*SCARLET_WORD_BYTES(a0)",
        "SCARLET_S s6, 8*SCARLET_WORD_BYTES(a0)",
        "SCARLET_S s7, 9*SCARLET_WORD_BYTES(a0)",
        "SCARLET_S s8, 10*SCARLET_WORD_BYTES(a0)",
        "SCARLET_S s9, 11*SCARLET_WORD_BYTES(a0)",
        "SCARLET_S s10, 12*SCARLET_WORD_BYTES(a0)",
        "SCARLET_S s11, 13*SCARLET_WORD_BYTES(a0)",
        // Restore next context (next_ctx)
        // Load stack pointer
        "SCARLET_L sp, 0*SCARLET_WORD_BYTES(a1)",
        // Load return address
        "SCARLET_L ra, 1*SCARLET_WORD_BYTES(a1)",
        // Load callee-saved registers s0-s11
        "SCARLET_L s0, 2*SCARLET_WORD_BYTES(a1)",
        "SCARLET_L s1, 3*SCARLET_WORD_BYTES(a1)",
        "SCARLET_L s2, 4*SCARLET_WORD_BYTES(a1)",
        "SCARLET_L s3, 5*SCARLET_WORD_BYTES(a1)",
        "SCARLET_L s4, 6*SCARLET_WORD_BYTES(a1)",
        "SCARLET_L s5, 7*SCARLET_WORD_BYTES(a1)",
        "SCARLET_L s6, 8*SCARLET_WORD_BYTES(a1)",
        "SCARLET_L s7, 9*SCARLET_WORD_BYTES(a1)",
        "SCARLET_L s8, 10*SCARLET_WORD_BYTES(a1)",
        "SCARLET_L s9, 11*SCARLET_WORD_BYTES(a1)",
        "SCARLET_L s10, 12*SCARLET_WORD_BYTES(a1)",
        "SCARLET_L s11, 13*SCARLET_WORD_BYTES(a1)",
        // Return to the saved return address
        // This will either:
        // - Return to the original caller (first time)
        // - Resume where this context was previously switched out
        "ret",
    );
}

/// Initialize a kernel context for first-time execution
///
/// This function sets up a kernel context to start executing at the specified
/// entry point when first switched to.
///
/// # Arguments
/// * `ctx` - Mutable reference to the context to initialize
/// * `entry_point` - Function pointer to start executing
/// * `stack_top` - Top of the stack for this context
pub fn init_kernel_context(ctx: &mut KernelContext, entry_point: fn(), stack_top: u64) {
    // Set up initial state for first-time execution
    ctx.sp = usize::try_from(stack_top).expect("kernel stack exceeds XLEN");
    ctx.ra = entry_point as usize;

    // Clear all saved registers
    ctx.s = [0; 12];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::TASK_KERNEL_STACK_SIZE;
    use alloc::boxed::Box;

    /// Test kernel context initialization
    #[test_case]
    fn test_init_kernel_context() {
        let mut ctx = KernelContext::new();
        let stack = Box::new([0u8; TASK_KERNEL_STACK_SIZE]);
        let stack_top = stack.as_ptr() as u64 + TASK_KERNEL_STACK_SIZE as u64;

        fn test_entry() {
            // Test entry point
        }

        init_kernel_context(&mut ctx, test_entry, stack_top);

        assert_eq!(ctx.sp as u64, stack_top);
        assert_eq!(ctx.ra, test_entry as usize);
        assert_eq!(ctx.s, [0; 12]);
    }
}
