//! User-mode trap handling for x86_64
//!
//! Handles traps/interrupts that occur while executing in user mode (Ring 3)

use core::arch::naked_asm;

use super::super::Trapframe;

/// User trap entry point (naked function wrapper)
#[unsafe(naked)]
pub unsafe extern "sysv64" fn _user_trap_entry() {
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
        "mov rdi, rsp",
        "call {handler}",
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
        "jmp {exit}",
        handler = sym arch_user_trap_handler,
        exit = sym _user_trap_exit,
    );
}

/// User trap exit point
#[unsafe(naked)]
pub unsafe extern "sysv64" fn _user_trap_exit() {
    naked_asm!("add rsp, 8", "iretq",);
}

/// Actual user trap handler
#[unsafe(no_mangle)]
pub extern "C" fn arch_user_trap_handler(trapframe: &mut Trapframe) {
    use crate::sched::scheduler;
    super::super::interrupt::eoi();
    let cpu_id = super::super::get_current_cpu_id();
    let sched = scheduler::get_scheduler();
    if let Some(task) = sched.get_current_task(cpu_id) {
        let _ = task;
    }
    let _ = trapframe;
}

/// First switch to user (direct transition, not via interrupt)
#[unsafe(naked)]
pub unsafe extern "sysv64" fn x86_64_first_switch_to_user_naked(
    _trapframe_addr: usize,
    _trap_exit_addr: usize,
) -> ! {
    naked_asm!(
        "mov rbx, rdi",
        "mov ax, 0x23",
        "mov ds, ax",
        "mov es, ax",
        "push 0x23",
        "mov rax, [rbx + 136]",
        "push rax",
        "mov rax, [rbx + 152]",
        "push rax",
        "push 0x1B",
        "mov rax, [rbx + 144]",
        "push rax",
        "mov rax, [rbx + 160]",
        "wrfsbase rax",
        "mov rax, [rbx + 168]",
        "wrgsbase rax",
        "mov r15, [rbx + 112]",
        "mov r14, [rbx + 104]",
        "mov r13, [rbx + 96]",
        "mov r12, [rbx + 88]",
        "mov rbp, [rbx + 80]",
        "mov r11, [rbx + 56]",
        "mov r10, [rbx + 48]",
        "mov r9,  [rbx + 40]",
        "mov r8,  [rbx + 32]",
        "mov rcx, [rbx + 24]",
        "mov rdx, [rbx + 16]",
        "mov rsi, [rbx + 8]",
        "mov rax, [rbx + 64]",
        "mov rdi, [rbx + 0]",
        "mov rbx, [rbx + 72]",
        "iretq",
    );
}

/// Switch to user space (called from scheduler)
pub fn arch_switch_to_user_space(_trapframe: &mut Trapframe) -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}
