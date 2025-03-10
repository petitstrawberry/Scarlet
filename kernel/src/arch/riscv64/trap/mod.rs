use core::arch::asm;

use crate::println;
use crate::print;

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
    println!("trapframe:\n{:x?}", tf);
    println!("cause: {}", cause);
    println!("tval: 0x{:x}", tval);
}