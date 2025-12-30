//! AArch64 kernel context switching implementation
//!
//! This module provides low-level context switching functionality for AArch64,
//! enabling kernel tasks to yield execution and resume later at the same point.

use core::arch::naked_asm;

use crate::arch::KernelContext;

/// Switch from the current kernel context to the next kernel context
///
/// This function performs a kernel context switch by saving/restoring callee-saved
/// registers (SP, LR, X19-X28) between two `KernelContext`s.
///
/// # Safety
///
/// Must be called with valid pointers to `KernelContext`s whose stacks are valid.
#[unsafe(naked)]
pub unsafe extern "C" fn switch_to(prev_ctx: *mut KernelContext, next_ctx: *const KernelContext) {
    // Layout must match `context::KernelContext`:
    //   0x00 sp
    //   0x08 lr
    //   0x10 x[0].. (x19-x28)
    naked_asm!(
        // Save current context (prev_ctx)
        // x0 = prev_ctx, x1 = next_ctx
        "str x30, [x0, #8]", // LR
        "mov x9, sp",
        "str x9, [x0, #0]", // SP
        "stp x19, x20, [x0, #16]",
        "stp x21, x22, [x0, #32]",
        "stp x23, x24, [x0, #48]",
        "stp x25, x26, [x0, #64]",
        "stp x27, x28, [x0, #80]",
        // Restore next context (next_ctx)
        "ldr x30, [x1, #8]", // LR
        "ldr x9, [x1, #0]",  // SP
        "mov sp, x9",
        "ldp x19, x20, [x1, #16]",
        "ldp x21, x22, [x1, #32]",
        "ldp x23, x24, [x1, #48]",
        "ldp x25, x26, [x1, #64]",
        "ldp x27, x28, [x1, #80]",
        "ret",
    );
}
