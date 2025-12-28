use core::arch::asm;

use crate::early_println;

use super::Trapframe;

pub mod exception;
pub mod interrupt;
pub mod kernel;
pub mod user;

pub fn print_traplog(tf: &Trapframe) {
    let cause: usize;
    let tval: usize;
    let status: usize;
    let sepc: usize;
    let stvec: usize;
    let satp: usize;
    let sscratch: usize;
    unsafe {
        asm!("csrr {}, scause", out(reg) cause);
        asm!("csrr {}, stval", out(reg) tval);
        asm!("csrr {}, sstatus", out(reg) status);
        asm!("csrr {}, sepc", out(reg) sepc);
        asm!("csrr {}, stvec", out(reg) stvec);
        asm!("csrr {}, satp", out(reg) satp);
        asm!("csrr {}, sscratch", out(reg) sscratch);
    }
    let spp = (status >> 8) & 0b1;

    early_println!("trapframe:\n{:#x?}", tf);
    early_println!("cause: {}", cause);
    early_println!("tval: 0x{:x}", tval);
    early_println!("status: 0x{:x}", status);
    early_println!("spp: {}", spp);
    early_println!("sepc: 0x{:x}", sepc);
    early_println!("stvec: 0x{:x}", stvec);
    early_println!("satp: 0x{:x}", satp);
    early_println!("sscratch: 0x{:x}", sscratch);
}
