//! RISC-V H-extension CSR access wrappers

use core::arch::asm;

// hstatus bit fields
pub const HSTATUS_VSBE: u64 = 1 << 5;
pub const HSTATUS_GVA: u64 = 1 << 6;
pub const HSTATUS_SPV: u64 = 1 << 7;
pub const HSTATUS_SPVP: u64 = 1 << 8;
pub const HSTATUS_HU: u64 = 1 << 9;
pub const HSTATUS_VTVM: u64 = 1 << 20;
pub const HSTATUS_VTW: u64 = 1 << 21;
pub const HSTATUS_VTSR: u64 = 1 << 22;
pub const HSTATUS_VSXL_SHIFT: u64 = 32;

// Guest page fault scause values
pub const CAUSE_ECALL_FROM_VS: u64 = 10;
pub const CAUSE_GUEST_INST_PAGE_FAULT: u64 = 20;
pub const CAUSE_GUEST_LOAD_PAGE_FAULT: u64 = 21;
pub const CAUSE_VIRTUAL_INSTRUCTION: u64 = 22;
pub const CAUSE_GUEST_STORE_PAGE_FAULT: u64 = 23;

macro_rules! csr_read {
    ($name:ident, $csr:literal) => {
        #[inline]
        pub fn $name() -> u64 {
            let val: u64;
            unsafe { asm!(concat!("csrr {0}, ", $csr), out(reg) val) };
            val
        }
    };
}

macro_rules! csr_write {
    ($name:ident, $csr:literal) => {
        #[inline]
        pub fn $name(val: u64) {
            unsafe { asm!(concat!("csrw ", $csr, ", {0}"), in(reg) val) };
        }
    };
}

// HS-mode hypervisor CSRs
csr_read!(read_hstatus, "hstatus");
csr_write!(write_hstatus, "hstatus");
csr_read!(read_hedeleg, "hedeleg");
csr_write!(write_hedeleg, "hedeleg");
csr_read!(read_hideleg, "hideleg");
csr_write!(write_hideleg, "hideleg");
csr_read!(read_hie, "hie");
csr_write!(write_hie, "hie");
csr_read!(read_hip, "hip");
csr_read!(read_hvip, "hvip");
csr_write!(write_hvip, "hvip");
csr_read!(read_hgatp, "hgatp");
csr_write!(write_hgatp, "hgatp");
csr_read!(read_htval, "htval");
csr_read!(read_htinst, "htinst");
csr_read!(read_hcounteren, "hcounteren");
csr_write!(write_hcounteren, "hcounteren");
csr_read!(read_htimedelta, "htimedelta");
csr_write!(write_htimedelta, "htimedelta");
csr_read!(read_henvcfg, "henvcfg");
csr_write!(write_henvcfg, "henvcfg");

// VS-mode CSRs (save/restore for guest context)
csr_read!(read_vsstatus, "vsstatus");
csr_write!(write_vsstatus, "vsstatus");
csr_read!(read_vsie, "vsie");
csr_write!(write_vsie, "vsie");
csr_read!(read_vstvec, "vstvec");
csr_write!(write_vstvec, "vstvec");
csr_read!(read_vsscratch, "vsscratch");
csr_write!(write_vsscratch, "vsscratch");
csr_read!(read_vsepc, "vsepc");
csr_write!(write_vsepc, "vsepc");
csr_read!(read_vscause, "vscause");
csr_write!(write_vscause, "vscause");
csr_read!(read_vstval, "vstval");
csr_write!(write_vstval, "vstval");
csr_read!(read_vsip, "vsip");
csr_write!(write_vsip, "vsip");
csr_read!(read_vsatp, "vsatp");
csr_write!(write_vsatp, "vsatp");

// S-mode CSRs used during guest entry/exit
csr_read!(read_sepc, "sepc");
csr_write!(write_sepc, "sepc");
csr_read!(read_scause, "scause");
csr_read!(read_stval, "stval");
csr_read!(read_sstatus, "sstatus");
csr_write!(write_sstatus, "sstatus");

#[inline]
pub fn hfence_gvma_all() {
    // SAFETY: hfence.gvma flushes G-stage TLB entries
    unsafe { asm!("hfence.gvma zero, zero") };
}

#[inline]
pub fn hfence_gvma_gpa(gpa: u64) {
    // SAFETY: hfence.gvma flushes G-stage TLB for a specific GPA
    unsafe { asm!("hfence.gvma {0}, zero", in(reg) gpa >> 12) };
}
