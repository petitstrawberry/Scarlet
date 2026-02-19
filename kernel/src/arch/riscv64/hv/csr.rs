//! CSR read/write helpers for RISC-V H-extension

use core::arch::asm;

use crate::arch::hv::csr;

macro_rules! csr_read {
    ($name:ident, $csr:literal) => {
        pub fn $name() -> u64 {
            let val: u64;
            unsafe {
                asm!(
                    concat!("csrr {0}, ", $csr),
                    out(reg) val,
                    options(nostack)
                );
            }
            val
        }
    };
}

macro_rules! csr_write {
    ($name:ident, $csr:literal) => {
        pub fn $name(val: u64) {
            unsafe {
                asm!(
                    concat!("csrw ", $csr, ", {0}"),
                    in(reg) val,
                    options(nostack)
                );
            }
        }
    };
}

// H-extension CSRs

// HS-mode registers
csr_read!(read_hstatus, "hstatus");
csr_write!(write_hstatus, "hstatus");

csr_read!(read_hgatp, "hgatp");
csr_write!(write_hgatp, "hgatp");

csr_read!(read_hgeie, "hgeie");
csr_write!(write_hgeie, "hgeie");

// hgeip is read-only (Hypervisor Guest External Interrupt Pending)
// Writing to it causes an illegal instruction trap
csr_read!(read_hgeip, "hgeip");

csr_read!(read_hideleg, "hideleg");
csr_write!(write_hideleg, "hideleg");

csr_read!(read_hedeleg, "hedeleg");
csr_write!(write_hedeleg, "hedeleg");

csr_read!(read_htval, "htval");
csr_read!(read_htinst, "htinst");

// Virtual supervisor CSRs (for guest state)
csr_read!(read_vsscratch, "vsscratch");
csr_write!(write_vsscratch, "vsscratch");

csr_read!(read_vsepc, "vsepc");
csr_write!(write_vsepc, "vsepc");

csr_read!(read_vscause, "vscause");
csr_write!(write_vscause, "vscause");

csr_read!(read_vstval, "vstval");
csr_write!(write_vstval, "vstval");

csr_read!(read_vsatp, "vsatp");
csr_write!(write_vsatp, "vsatp");

csr_read!(read_vsstatus, "vsstatus");
csr_write!(write_vsstatus, "vsstatus");

csr_read!(read_vsie, "vsie");
csr_write!(write_vsie, "vsie");

csr_read!(read_vsip, "vsip");
csr_write!(write_vsip, "vsip");

csr_read!(read_hvip, "hvip");
csr_write!(write_hvip, "hvip");

csr_read!(read_vstimecmp, "vstimecmp");
csr_write!(write_vstimecmp, "vstimecmp");

// Non-H-extension CSRs that we need to access in the hypervisor
csr_read!(read_scause, "scause");
csr_read!(read_stval, "stval");
csr_read!(read_sepc, "sepc");

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GuestCsrState {
    pub sscratch: u64,
    pub sepc: u64,
    pub scause: u64,
    pub stval: u64,
    pub satp: u64,
    pub sstatus: u64,
    pub sie: u64,
    pub sip: u64,
}

impl GuestCsrState {
    pub fn save() -> Self {
        Self {
            sscratch: read_vsscratch(),
            sepc: read_vsepc(),
            scause: read_vscause(),
            stval: read_vstval(),
            satp: read_vsatp(),
            sstatus: read_vsstatus(),
            sie: read_vsie(),
            sip: read_vsip(),
        }
    }

    pub fn restore(&self) {
        write_vsscratch(self.sscratch);
        write_vsepc(self.sepc);
        write_vscause(self.scause);
        write_vstval(self.stval);
        write_vsatp(self.satp);
        write_vsstatus(self.sstatus);
        write_vsie(self.sie);
        write_vsip(self.sip);
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct HypervisorCsrState {
    pub hstatus: u64,
    pub hgatp: u64,
    pub hgeie: u64,
    pub hideleg: u64,
    pub hedeleg: u64,
}

impl HypervisorCsrState {
    pub fn save() -> Self {
        Self {
            hstatus: read_hstatus(),
            hgatp: read_hgatp(),
            hgeie: read_hgeie(),
            hideleg: read_hideleg(),
            hedeleg: read_hedeleg(),
        }
    }

    pub fn restore(&self) {
        write_hstatus(self.hstatus);
        write_hgatp(self.hgatp);
        write_hgeie(self.hgeie);
        write_hideleg(self.hideleg);
        write_hedeleg(self.hedeleg);
    }
}
