//! Kernel-mode trap handling for x86_64
//!
//! Handles traps/interrupts that occur while executing in kernel mode

use core::arch::naked_asm;

use super::super::Trapframe;

/// Kernel trap entry point (naked function wrapper)
#[unsafe(naked)]
pub unsafe extern "sysv64" fn _kernel_trap_entry() {
    naked_asm!(
        "push rax",
        "push rbx",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",
        "push rbp",
        "push r8",
        "push r9",
        "push r10",
        "push r11",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov ax, ds",
        "push rax",
        "mov ax, es",
        "push rax",
        "push fs",
        "push gs",
        "mov ax, 0x10",
        "mov ds, ax",
        "mov es, ax",
        "mov rdi, rsp",
        "call {handler}",
        "pop gs",
        "pop fs",
        "pop rax",
        "mov es, ax",
        "pop rax",
        "mov ds, ax",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop r11",
        "pop r10",
        "pop r9",
        "pop r8",
        "pop rbp",
        "pop rdi",
        "pop rsi",
        "pop rdx",
        "pop rcx",
        "pop rbx",
        "pop rax",
        "iretq",
        handler = sym arch_kernel_trap_handler,
    );
}

/// Actual kernel trap handler
#[unsafe(no_mangle)]
pub extern "C" fn arch_kernel_trap_handler(regs: &mut Trapframe) {
    super::super::interrupt::eoi();
    let _ = regs;
}

/// Load the IDT with kernel trap vectors
pub fn load_idt() {
    super::init_idt();
}

/// Get the kernel trap vector address
pub fn get_kernel_trap_entry() -> usize {
    _kernel_trap_entry as usize
}
