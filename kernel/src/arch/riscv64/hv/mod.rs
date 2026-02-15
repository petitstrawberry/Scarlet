//! RISC-V H-extension hypervisor support

pub mod csr;
pub mod guest_vcpu;
pub mod mmu;
pub mod reg_index;
pub mod switch;
pub mod vmexit;

pub use guest_vcpu::{GuestCsrState, GuestVcpu};
pub use reg_index::reg;
pub use switch::run_guest_loop;

pub const HSTATUS_SPV: u64 = 1 << 7;

use crate::arch::{Mode, Trapframe};
use crate::task::mytask;

pub fn guest_trap_handler(trapframe: &mut Trapframe, _cause: usize, _interrupt: bool) {
    if let Some(task) = mytask() {
        task.exit(1);
    }
}

pub fn configure_guest_mode(mode: Mode) -> Result<(), &'static str> {
    use core::arch::asm;
    let mut hstatus: u64;
    unsafe {
        asm!("csrr {0}, hstatus", out(reg) hstatus);
    }
    hstatus |= HSTATUS_SPV;
    match mode {
        Mode::GuestUser => {
            hstatus &= !(1 << 8);
        }
        Mode::GuestKernel => {
            hstatus |= 1 << 8;
        }
        _ => return Err("Invalid mode for guest"),
    }
    unsafe {
        asm!("csrw hstatus, {0}", in(reg) hstatus);
    }
    Ok(())
}

pub fn set_guest_root_pagetable(token: u64) -> Result<(), &'static str> {
    use core::arch::asm;
    unsafe {
        asm!("csrw hgatp, {0}", in(reg) token);
        asm!("hfence.gvma zero, zero");
    }
    Ok(())
}
