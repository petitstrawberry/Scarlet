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
    #[cfg(feature = "hypervisor")]
    let hstatus: usize;

    unsafe {
        asm!("csrr {}, scause", out(reg) cause);
        asm!("csrr {}, stval", out(reg) tval);
        asm!("csrr {}, sstatus", out(reg) status);
        asm!("csrr {}, sepc", out(reg) sepc);
        asm!("csrr {}, stvec", out(reg) stvec);
        asm!("csrr {}, satp", out(reg) satp);
        asm!("csrr {}, sscratch", out(reg) sscratch);
        #[cfg(feature = "hypervisor")]
        asm!("csrr {}, hstatus", out(reg) hstatus);
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
    #[cfg(feature = "hypervisor")]
    {
        use crate::initcall::early;

        early_println!("hstatus: 0x{:x}", hstatus);
        early_println!(
            "HSTATUS_SPV: {}",
            (hstatus & crate::arch::hv::trap::HSTATUS_SPV as usize) != 0
        );
    }
}

pub const PRIV_U_MODE: usize = 0;
pub const PRIV_S_MODE: usize = 1;

pub fn prev_mode() -> usize {
    let status: usize;
    unsafe {
        asm!("csrr {}, sstatus", out(reg) status);
    }
    (status >> 8) & 0b1
}
