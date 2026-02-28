//! x86_64 context switching assembly
//!
//! Low-level context switch implementation for x86_64

use core::arch::naked_asm;

/// Switch to a new task context (wrapper for API compatibility)
pub unsafe fn switch_to(
    prev: *mut super::context::KernelContext,
    next: *const super::context::KernelContext,
) {
    context_switch(&mut *prev, &*next);
}

/// Switch to a new task context
#[unsafe(naked)]
pub unsafe extern "sysv64" fn context_switch(
    _prev: &mut super::context::KernelContext,
    _next: &super::context::KernelContext,
) {
    naked_asm!(
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi], rsp",
        "mov rsp, [rsi]",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",
        "ret",
    );
}

/// Switch to user mode with the provided trapframe
#[unsafe(naked)]
pub unsafe extern "sysv64" fn switch_to_user(_trapframe: *const super::Trapframe) {
    naked_asm!("ret");
}
