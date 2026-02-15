//! RISC-V H-extension hypervisor support
//!
//! Hardware-assisted virtualization using the RISC-V Hypervisor extension.
//! The kernel runs in HS-mode and manages guests in VS/VU-mode.

mod csr;
mod mmu;
mod trap;
mod vcpu;
mod vmexit;

use core::sync::atomic::{AtomicU16, Ordering};

use crate::environment::PAGE_SIZE;
use crate::hypervisor::memory::MemorySlotFlags;

use mmu::GuestRoot;

static NEXT_VMID: AtomicU16 = AtomicU16::new(1);

pub fn set_guest_root_pagetable(guest_root_token: u64) -> Result<(), &'static str> {
    csr::write_hgatp(guest_root_token);
    csr::hfence_gvma_all();
    Ok(())
}

pub fn configure_guest_mode(mode: crate::arch::Mode) -> Result<(), &'static str> {
    let mut hstatus = csr::read_hstatus();
    match mode {
        crate::arch::Mode::GuestUser => {
            hstatus |= csr::HSTATUS_SPV;
            hstatus &= !csr::HSTATUS_SPVP;
        }
        crate::arch::Mode::GuestKernel => {
            hstatus |= csr::HSTATUS_SPV;
            hstatus |= csr::HSTATUS_SPVP;
        }
        _ => {
            return Err("Invalid mode for guest configuration");
        }
    }
    csr::write_hstatus(hstatus);
    Ok(())
}

pub struct GuestVcpu {
    vcpu: super::vcpu::Vcpu,
}

impl GuestVcpu {
    pub fn new() -> Self {
        GuestVcpu {
            vcpu: super::vcpu::Vcpu::new(crate::arch::Mode::GuestUser),
        }
    }

    pub fn run(&mut self) -> Result<crate::hypervisor::exit::VmExit, &'static str> {
        Err("vcpu tasks handle guest execution")
    }
}
