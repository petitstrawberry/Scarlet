use core::arch::{asm, naked_asm};
use core::mem::transmute;

use crate::arch::{Trapframe, get_cpu};
use crate::early_println;
use crate::vm::get_kernel_vm_manager;

#[unsafe(link_section = ".trampoline.text")]
#[unsafe(export_name = "_kernel_trap_entry")]
#[unsafe(naked)]
pub extern "C" fn _kernel_trap_entry() {
    unsafe {
        naked_asm!(
            "
            // TODO: Save registers to stack (see RISC-V for reference)
            // TODO: Set up stack pointer, disable interrupts, etc.
            // TODO: Call arch_kernel_trap_handler
            // TODO: Restore registers and return from exception
            b .
            "
        );
    }
}

#[unsafe(export_name = "arch_kernel_trap_handler")]
pub extern "C" fn arch_kernel_trap_handler(addr: usize) {
    let trapframe: &mut Trapframe = unsafe { transmute(addr) };
    // TODO: Read exception syndrome register, handle exception, etc.
    early_println!("[aarch64] arch_kernel_trap_handler called: trapframe={:p}", trapframe);
    // TODO: Implement kernel exception handling logic
}
