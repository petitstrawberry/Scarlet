//! AArch64 trap handling
//!
//! Exception and trap handling for AArch64 architecture.

use core::arch::asm;

use crate::arch::Trapframe;
use crate::early_println;

pub mod emulator;
pub mod exception;
pub mod interrupt;
pub mod kernel;
pub mod user;

pub fn trap_init() {
    // Currently no global trap init beyond setting VBAR_EL1 via set_trapvector().
}

pub fn print_traplog(tf: &Trapframe) {
    let esr: u64;
    let far: u64;
    let elr: u64;
    let spsr: u64;
    unsafe {
        asm!("mrs {0}, esr_el1", out(reg) esr, options(nostack));
        asm!("mrs {0}, far_el1", out(reg) far, options(nostack));
        asm!("mrs {0}, elr_el1", out(reg) elr, options(nostack));
        asm!("mrs {0}, spsr_el1", out(reg) spsr, options(nostack));
    }

    early_println!("[aarch64] trapframe:\n{:#x?}", tf);
    early_println!("[aarch64] esr_el1: {:#x}", esr);
    early_println!("[aarch64] far_el1: {:#x}", far);
    early_println!("[aarch64] elr_el1: {:#x}", elr);
    early_println!("[aarch64] spsr_el1: {:#x}", spsr);
}
