//! User-mode trap handling for x86_64
//!
//! Handles traps/interrupts that occur while executing in user mode (Ring 3)

use core::arch::asm;

use super::super::Trapframe;

/// User trap entry point (from assembly trampoline)
#[no_mangle]
pub static _user_trap_entry: u64 = _user_trap_entry as u64;

/// User trap entry point function
///
/// This is called when a trap occurs while in user mode (Ring 0).
/// The CPU switches to kernel stack before this code runs.
#[naked]
pub unsafe extern "sysv64" fn _user_trap_entry_impl() {
    asm!(
        // Save all general-purpose registers
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

        // At this point, the stack contains:
        // [SS] [RSP] [RFLAGS] [CS] [RIP] [error_code]
        // + our saved registers

        // Save pointer to the trapframe (current stack pointer)
        "mov rdi, rsp",

        // Call the actual handler
        "call {}",

        // Restore general-purpose registers
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

        // Now fall through to trap exit
        sym arch_user_trap_handler,
    );
}

/// User trap exit point
///
/// This is where we return to user mode after handling a trap.
#[naked]
pub unsafe extern "sysv64" fn _user_trap_exit_impl() {
    asm!(
        // Restore general-purpose registers from trapframe
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

        // Restore user segments
        // Pop error code (if present)
        "add rsp, 8",

        // Restore RSP, RFLAGS, CS, RIP, SS
        "iretq",
        options(noreturn),
    );
}

/// Actual user trap handler
///
/// # Arguments
/// * `trapframe` - Pointer to the saved trapframe
#[no_mangle]
extern "C" fn arch_user_trap_handler(trapframe: &mut Trapframe) {
    use crate::sched::scheduler;

    // Acknowledge interrupt if applicable
    super::super::interrupt::eoi();

    // Get the trap cause
    let _trap_cause = 0; // Would be determined from exception info

    // Handle the trap
    // In a full implementation, this would:
    // 1. Determine if it's a system call, exception, or interrupt
    // 2. For syscalls, dispatch to the syscall handler
    // 3. For exceptions, possibly terminate the task
    // 4. For interrupts, return to user mode

    let cpu_id = super::super::get_current_cpu_id();
    let sched = scheduler::get_scheduler();

    if let Some(task) = sched.get_current_task(cpu_id) {
        // Handle the trap for this task
        let _ = task;
        // ...
    }

    let _ = trapframe; // Suppress unused warning
}

/// First switch to user (direct transition, not via interrupt)
///
/// # Arguments
/// * `trapframe_addr` - Physical address of the trapframe
/// * `trap_exit_addr` - Address of the trap exit trampoline code
///
/// # Safety
/// The trapframe must be valid and properly initialized.
#[naked]
pub unsafe extern "sysv64" fn x86_64_first_switch_to_user_naked(trapframe_addr: usize, trap_exit_addr: usize) {
    asm!(
        // Set up kernel stack pointer
        "mov r15, rdi", // Save trapframe addr

        // Load trapframe pointer
        "mov rsp, rdi",

        // Restore user registers
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

        // Load user segments
        // We need to set up the user GDT entries first
        "mov rcx, 0x23", // User data segment (RPL=3)
        "mov ds, rcx",
        "mov es, rcx",

        // Get user RSP from trapframe
        "mov rcx, [rsp + 15*8]", // RSP is at offset 15 (after all regs)

        // Get user RIP from trapframe
        "mov r9, [rsp + 16*8]", // RIP
        "mov r10, [rsp + 17*8]", // RFLAGS

        // Set user RSP
        "mov rsp, rcx",

        // Swap to user code segment and jump to user RIP
        "push rdx", // Save rdx temporarily
        "push 0x1B", // User code segment (RPL=3)
        "push r9",   // User RIP
        "push r10",  // User RFLAGS
        "push 0x23", // User data segment
        "push rcx",  // User RSP

        // Restore rdx
        "pop rdx",

        "iretq",
        options(noreturn),
    );
}
