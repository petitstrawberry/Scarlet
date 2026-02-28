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
        "mov rsp, rdi",
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
        "mov rcx, 0x23",
        "mov ds, rcx",
        "mov es, rcx",
        "mov rcx, [rsp + 15*8]",
        "mov r9, [rsp + 16*8]",
        "mov r10, [rsp + 17*8]",
        "mov rsp, rcx",
        "push rdx",
        "push 0x1B",
        "push r9",
        "push r10",
        "push 0x23",
        "push rcx",
        "pop rdx",
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
