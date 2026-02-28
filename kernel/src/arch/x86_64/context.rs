//! x86_64 kernel context management
//!
//! Manages kernel stack switching and context saving/restoring

use core::arch::naked_asm;

/// Kernel context for x86_64
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
    rsp: u64,
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
            rsp: 0,
        }
    }

    pub unsafe fn new_context(entry: usize, stack_top: usize) -> Self {
        KernelContext {
            rbx: 0,
            rbp: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: entry as u64,
            rsp: stack_top as u64,
        }
    }

    pub fn set_sp(&mut self, sp: u64) {
        self.rsp = sp;
    }

    pub fn get_kernel_stack_memory_area_paddr(&self) -> crate::vm::vmem::MemoryArea {
        crate::vm::vmem::MemoryArea {
            start: self.rsp as usize,
            end: self.rsp as usize + crate::environment::TASK_KERNEL_STACK_SIZE,
        }
    }

    pub fn get_kernel_stack_bottom_paddr(&self) -> u64 {
        self.rsp
    }

    pub fn get_kernel_stack_memory_area(&self) -> crate::vm::vmem::MemoryArea {
        self.get_kernel_stack_memory_area_paddr()
    }
}

/// Switch from current kernel context to next kernel context
#[unsafe(naked)]
pub unsafe extern "sysv64" fn switch(_current: &mut KernelContext, _next: &KernelContext) {
    naked_asm!(
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov [rdi + 0x00], rbx",
        "mov [rdi + 0x08], rbp",
        "mov [rdi + 0x10], r12",
        "mov [rdi + 0x18], r13",
        "mov [rdi + 0x20], r14",
        "mov [rdi + 0x28], r15",
        "mov [rdi + 0x30], rax",
        "mov rbx, [rsi + 0x00]",
        "mov rbp, [rsi + 0x08]",
        "mov r12, [rsi + 0x10]",
        "mov r13, [rsi + 0x18]",
        "mov r14, [rsi + 0x20]",
        "mov r15, [rsi + 0x28]",
        "mov rax, [rsi + 0x30]",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",
        "ret",
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
