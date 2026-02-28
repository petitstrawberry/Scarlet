//! x86_64 kernel context management
//!
//! Manages kernel stack switching and context saving/restoring

use core::arch::asm;

/// Kernel context for x86_64
///
/// Saved registers during context switch:
/// - All callee-saved registers: RBX, RBP, R12, R13, R14, R15
/// - Return address (RIP) pushed by call
/// - Stack pointer (RSP)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KernelContext {
    rbx: u64,
    rbp: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rip: u64,
}

impl KernelContext {
    pub const fn new() -> Self {
        KernelContext {
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0,
        }
    }

    /// Create a new kernel context for a task
    ///
    /// # Safety
    ///
    /// The entry function must be valid and the stack must be properly aligned.
    pub unsafe fn new_context(entry: usize, stack_top: usize) -> Self {
        KernelContext {
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: entry as u64,
        }
    }
}

/// Switch from current kernel context to next kernel context
///
/// # Safety
///
/// Both contexts must be valid and properly initialized.
#[naked]
#[inline(always)]
pub unsafe extern "sysv64" fn switch(current: &mut KernelContext, next: &KernelContext) {
    asm!(
        // Save callee-saved registers
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        // Save current RSP to current context
        "mov [rdi + 0x00], rbx",
        "mov [rdi + 0x08], rbp",
        "mov [rdi + 0x10], r12",
        "mov [rdi + 0x18], r13",
        "mov [rdi + 0x20], r14",
        "mov [rdi + 0x28], r15",
        "mov [rdi + 0x30], rax", // Return address
        // Load next context
        "mov rbx, [rsi + 0x00]",
        "mov rbp, [rsi + 0x08]",
        "mov r12, [rsi + 0x10]",
        "mov r13, [rsi + 0x18]",
        "mov r14, [rsi + 0x20]",
        "mov r15, [rsi + 0x28]",
        // Load return address to RAX
        "mov rax, [rsi + 0x30]",
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

/// Initialize a new kernel context for a task
///
/// # Safety
///
/// The stack must be valid and properly aligned.
pub unsafe fn init_task_context(context: &mut KernelContext, entry: usize, _stack_top: usize) {
    *context = KernelContext::new_context(entry, _stack_top);
}
