//! Guest MMU management for RISC-V H-extension

use core::arch::asm;

use crate::arch::vm::mmu::PageTable;
use crate::hypervisor::types::InterruptType;

pub fn hfence_gvma_all() {
    unsafe {
        asm!("hfence.gvma zero, zero");
    }
}

pub fn hfence_gvma(gpa: u64) {
    unsafe {
        asm!("hfence.gvma {0}, zero", in(reg) gpa);
    }
}

pub fn map_stage2_page(
    pagetable: &mut PageTable,
    gpa: u64,
    hpa: u64,
    writable: bool,
    asid: u16,
) -> Result<(), &'static str> {
    let gpa = gpa & !0xfff;
    let hpa = hpa & !0xfff;

    let pte = pagetable
        .walk(gpa as usize, true, asid)
        .ok_or("walk failed")?;

    let ppn = (hpa as usize >> 12) & 0xffff_ffff_fff;
    pte.clear_all();
    pte.readable();
    if writable {
        pte.writable();
    }
    pte.executable();
    pte.set_ppn(ppn);
    pte.validate();

    hfence_gvma(gpa);
    Ok(())
}

pub fn set_guest_root_pagetable(pagetable: &PageTable, vmid: u16) {
    let ppn = pagetable as *const _ as usize >> 12;
    let mode = 9u64;
    let token = (mode << 60) | ((vmid as u64) << 44) | (ppn as u64);
    unsafe {
        asm!("csrw hgatp, {0}", in(reg) token);
        asm!("hfence.gvma zero, zero");
    }
}
