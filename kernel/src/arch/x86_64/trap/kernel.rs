//! Kernel-mode trap handling for x86_64
//!
//! Handles traps/interrupts that occur while executing in kernel mode

use super::super::Trapframe;

/// Kernel trap entry point (from assembly)
#[no_mangle]
pub static _kernel_trap_entry: u64 = _kernel_trap_entry as u64;

/// Kernel trap entry point function
///
/// This is called when a trap occurs while in kernel mode (Ring 0).
#[naked]
pub unsafe extern "sysv64" fn _kernel_trap_entry_impl() {
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

        // Save segment registers
        "mov ax, ds",
        "push rax",
        "mov ax, es",
        "push rax",
        "push fs",
        "push gs",

        // Load kernel data segments
        "mov ax, 0x10",
        "mov ds, ax",
        "mov es, ax",

        // Call the actual handler
        "mov rdi, rsp",
        "call {}",

        // Restore segment registers
        "pop gs",
        "pop fs",
        "pop rax",
        "mov es, ax",
        "pop rax",
        "mov ds, ax",

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

        "iretq",
        options(noreturn),
        sym arch_kernel_trap_handler,
    );
}

/// Actual kernel trap handler
///
/// # Arguments
/// * `regs` - Pointer to the saved registers on the stack
#[no_mangle]
extern "C" fn arch_kernel_trap_handler(regs: &mut Trapframe) {
    use crate::drivers::pic::LocalApic;

    // Get the interrupt vector if this is an interrupt
    let _vector = 0; // Would be determined from the interrupt frame

    // Acknowledge the interrupt
    super::super::interrupt::eoi();

    // Handle the interrupt
    // In a full implementation, this would:
    // 1. Determine the interrupt source
    // 2. Call the appropriate handler
    // 3. Return

    let _ = regs; // Suppress unused warning
}

/// Load the IDT with kernel trap vectors
pub fn load_idt() {
    super::init_idt();
}

/// Get the kernel trap vector address (for use in other parts of the kernel)
pub const fn get_kernel_trap_entry() -> usize {
    _kernel_trap_entry as usize
}
