use core::arch::asm;

use crate::{early_println, println};

use super::Trapframe;

pub mod interrupt;
pub mod exception;
pub mod kernel;
pub mod user;

pub fn print_traplog(tf: &Trapframe) {
    let cause: usize;
    let tval: usize;
    unsafe {
        asm!("csrr {}, scause", out(reg) cause);
        asm!("csrr {}, stval", out(reg) tval);
    }
    early_println!("trapframe:\n{:#x?}", tf);
    early_println!("cause: {}", cause);
    early_println!("tval: 0x{:x}", tval);
}