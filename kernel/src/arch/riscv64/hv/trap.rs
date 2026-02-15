//! RISC-V H-extension trap information

use crate::arch::Trapframe;

use super::csr;

pub fn is_from_guest() -> bool {
    let hstatus: u64;
    unsafe {
        core::arch::asm!("csrr {0}, hstatus", out(reg) hstatus);
    }
    (hstatus & super::HSTATUS_SPV) != 0
}

pub fn guest_trap_handler(_trapframe: &mut Trapframe, cause: usize) {
    // todo!("Handle guest trap: cause={}, stval={:#x}", cause, csr::read_stval());
    // return 
}