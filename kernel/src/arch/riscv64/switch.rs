//! RISC-V kernel context switching implementation
//!
//! This module provides low-level context switching functionality for RISC-V,
//! enabling kernel tasks to yield execution and resume later at the same point.

use crate::arch::KernelContext;
use core::arch::{asm, naked_asm};

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
    naked_asm!(
        // Save current context (prev_ctx)
        // a0 = prev_ctx, a1 = next_ctx
        
        // Save stack pointer
        "sd sp, 0(a0)",
        
        // Save return address
        "sd ra, 8(a0)",
        
        // Save callee-saved registers s0-s11
        "sd s0, 16(a0)",
        "sd s1, 24(a0)",
        "sd s2, 32(a0)",
        "sd s3, 40(a0)",
        "sd s4, 48(a0)",
        "sd s5, 56(a0)",
        "sd s6, 64(a0)",
        "sd s7, 72(a0)",
        "sd s8, 80(a0)",
        "sd s9, 88(a0)",
        "sd s10, 96(a0)",
        "sd s11, 104(a0)",
        
        // Restore next context (next_ctx)
        // Load stack pointer
        "ld sp, 0(a1)",
        
        // Load return address
        "ld ra, 8(a1)",
        
        // Load callee-saved registers s0-s11
        "ld s0, 16(a1)",
        "ld s1, 24(a1)",
        "ld s2, 32(a1)",
        "ld s3, 40(a1)",
        "ld s4, 48(a1)",
        "ld s5, 56(a1)",
        "ld s6, 64(a1)",
        "ld s7, 72(a1)",
        "ld s8, 80(a1)",
        "ld s9, 88(a1)",
        "ld s10, 96(a1)",
        "ld s11, 104(a1)",
        
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
    ctx.sp = stack_top;
    ctx.ra = entry_point as u64;
    
    // Clear all saved registers
    ctx.s = [0; 12];
}

/// Wrapper function for safe context switching
///
/// This provides a safe interface to the low-level switch_to function.
///
/// # Arguments
/// * `prev_ctx` - Mutable reference to store the current context
/// * `next_ctx` - Reference to the context to switch to
///
/// # Safety
/// Both contexts must have valid stack pointers and be properly initialized.
pub fn kernel_switch_to(prev_ctx: &mut KernelContext, next_ctx: &KernelContext) {
    unsafe {
        switch_to(prev_ctx as *mut KernelContext, next_ctx as *const KernelContext);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use crate::environment::TASK_KERNEL_STACK_SIZE;

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
        
        assert_eq!(ctx.sp, stack_top);
        assert_eq!(ctx.ra, test_entry as u64);
        assert_eq!(ctx.s, [0; 12]);
    }
}
