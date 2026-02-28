//! x86_64 context switching assembly
//!
//! Low-level context switch implementation for x86_64

use core::arch::asm;

/// Switch to a new task context
///
/// This function performs a context switch from the current task
/// to a new task, saving all necessary state.
///
/// # Arguments
/// * `prev` - Mutable reference to the previous task's kernel context
/// * `next` - Reference to the next task's kernel context
///
/// # Safety
/// Both contexts must be valid and properly initialized.
#[naked]
pub unsafe extern "sysv64" fn context_switch(
    prev: &mut super::context::KernelContext,
    next: &super::context::KernelContext,
) {
    asm!(
        // Save callee-saved registers
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // Save stack pointer to previous context
        "mov [rdi], rsp",
        // Load stack pointer from next context
        "mov rsp, [rsi]",
        // Restore callee-saved registers
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",
        "ret",
        options(noreturn)
    );
}

/// Switch to user mode with the provided trapframe
///
/// # Arguments
/// * `trapframe` - Pointer to the user's trapframe
///
/// # Safety
/// The trapframe must be valid and properly initialized.
#[naked]
pub unsafe extern "sysv64" fn switch_to_user(trapframe: *const super::Trapframe) {
    asm!(
        // Load user GDT entries (would be set up by the trampoline)
        // This is a simplified stub - the real switch happens in the trampoline

        // For now, just return
        "ret",
        options(noreturn)
    );
}
