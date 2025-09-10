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
    let status: usize;
    unsafe {
        asm!("csrr {}, scause", out(reg) cause);
        asm!("csrr {}, stval", out(reg) tval);
        asm!("csrr {}, sstatus", out(reg) status);
    }
    let spp = (status >> 8) & 0b1;

    early_println!("trapframe:\n{:#x?}", tf);
    early_println!("cause: {}", cause);
    early_println!("tval: 0x{:x}", tval);
    early_println!("status: 0x{:x}", status);
    early_println!("spp: {}", spp);
}