//! CSR read/write helpers for RISC-V H-extension

use core::arch::asm;

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

csr_read!(read_hstatus, "hstatus");
csr_write!(write_hstatus, "hstatus");

csr_read!(read_hgatp, "hgatp");
csr_write!(write_hgatp, "hgatp");

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

csr_read!(read_htval, "htval");
csr_read!(read_htinst, "htinst");

csr_read!(read_scause, "scause");
csr_read!(read_stval, "stval");

pub fn clear_hstatus_spv() {
    let mut hstatus = read_hstatus();
    hstatus &= !super::HSTATUS_SPV;
    write_hstatus(hstatus);
}
