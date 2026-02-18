//! Guest MMU management for RISC-V H-extension

use core::arch::asm;

use crate::arch::vm::{mmu::PageTable, new_raw_pagetable};

pub fn hfence_gvma_all() {
    unsafe {
        asm!("hfence.gvma zero, zero");
    }
}

pub fn hfence_gvma(gpa: u64) {
    let gpa_page = gpa >> 12;
    unsafe {
        asm!("hfence.gvma {0}, zero", in(reg) gpa_page);
    }
}

pub fn read_hgatp() -> u64 {
    let val: u64;
    unsafe {
        asm!("csrr {0}, hgatp", out(reg) val);
    }
    val
}

pub fn verify_hgatp(expected_pagetable: &PageTable, vmid: u16) {
    let hgatp = read_hgatp();
    let expected_ppn = expected_pagetable as *const _ as usize >> 12;
    let expected_token = (9u64 << 60) | ((vmid as u64) << 44) | (expected_ppn as u64);
    let actual_ppn = hgatp & 0xffff_ffff_fff;
    let actual_vmid = (hgatp >> 44) & 0xffff;
    let actual_mode = hgatp >> 60;

    crate::early_println!(
        "[verify_hgatp] expected_token={:#x} actual_hgatp={:#x}",
        expected_token,
        hgatp
    );
    crate::early_println!(
        "[verify_hgatp] mode={} vmid={} ppn={:#x}",
        actual_mode,
        actual_vmid,
        actual_ppn
    );

    if actual_ppn != expected_ppn as u64 {
        crate::early_println!(
            "[verify_hgatp] MISMATCH! expected_ppn={:#x} actual_ppn={:#x}",
            expected_ppn,
            actual_ppn
        );
    }
    if actual_vmid != vmid as u64 {
        crate::early_println!(
            "[verify_hgatp] VMID MISMATCH! expected={} actual={}",
            vmid,
            actual_vmid
        );
    }
}

fn debug_walk_stage2(
    pagetable: &mut PageTable,
    gpa: usize,
    asid: u16,
) -> Option<*mut crate::arch::vm::mmu::PageTableEntry> {
    use crate::arch::vm::mmu::PageTableEntry;

    let mut current_table = pagetable.entries.as_mut_ptr();

    crate::early_println!(
        "[debug_walk] GPA={:#x} root_table={:#x}",
        gpa,
        current_table as usize
    );

    let vpn3 = (gpa >> 39) & 0x7ff;
    let pte_addr = unsafe { current_table.add(vpn3) as usize };
    let pte = unsafe { &mut *current_table.add(vpn3) };

    crate::early_println!(
        "[debug_walk] level=3 vpn={} pte_addr={:#x} pte={:#x} valid={}",
        vpn3,
        pte_addr,
        pte.entry,
        pte.is_valid()
    );

    if !pte.is_valid() {
        let new_table = unsafe { new_raw_pagetable(asid) };
        if new_table.is_null() {
            return None;
        }
        crate::early_println!(
            "[debug_walk] level=3 allocated new_table={:#x}",
            new_table as usize
        );
        pte.set_ppn(new_table as usize >> 12);
        pte.validate();
        crate::early_println!(
            "[debug_walk] level=3 pte after={:#x} ppn={:#x}",
            pte.entry,
            pte.get_ppn()
        );
    }
    let next_addr = pte.get_ppn() << 12;
    crate::early_println!("[debug_walk] level=3 -> next_table={:#x}", next_addr);
    current_table = next_addr as *mut PageTableEntry;

    for level in (1..=2).rev() {
        let vpn = (gpa >> (12 + 9 * level)) & 0x1ff;
        let pte_addr = unsafe { current_table.add(vpn) as usize };
        let pte = unsafe { &mut *current_table.add(vpn) };

        crate::early_println!(
            "[debug_walk] level={} vpn={} pte_addr={:#x} pte={:#x} valid={}",
            level,
            vpn,
            pte_addr,
            pte.entry,
            pte.is_valid()
        );

        if !pte.is_valid() {
            let new_table = unsafe { new_raw_pagetable(asid) };
            if new_table.is_null() {
                return None;
            }
            crate::early_println!(
                "[debug_walk] level={} allocated new_table={:#x}",
                level,
                new_table as usize
            );
            pte.set_ppn(new_table as usize >> 12);
            pte.validate();
            crate::early_println!(
                "[debug_walk] level={} pte after={:#x} ppn={:#x}",
                level,
                pte.entry,
                pte.get_ppn()
            );
        }
        let next_addr = pte.get_ppn() << 12;
        crate::early_println!(
            "[debug_walk] level={} -> next_table={:#x}",
            level,
            next_addr
        );
        current_table = next_addr as *mut PageTableEntry;
    }

    let vpn = (gpa >> 12) & 0x1ff;
    let final_pte = unsafe { current_table.add(vpn) };
    crate::early_println!(
        "[debug_walk] final level=0: vpn={} pte_addr={:#x} pte={:#x}",
        vpn,
        final_pte as usize,
        unsafe { *final_pte }.entry
    );
    Some(final_pte)
}

pub fn map_stage2_page(
    pagetable: &mut PageTable,
    gpa: u64,
    hpa: u64,
    writable: bool,
    accessed: bool,
    dirty: bool,
    asid: u16,
) -> Result<(), &'static str> {
    let gpa = gpa & !0xfff;
    let hpa = hpa & !0xfff;

    crate::early_println!("[map_stage2] gpa={:#x} hpa={:#x} asid={}", gpa, hpa, asid);
    crate::early_println!(
        "[map_stage2] pagetable ptr={:#x}",
        pagetable as *const _ as usize
    );

    let hgatp = read_hgatp();
    let hgatp_ppn = hgatp & 0xffff_ffff_fff;
    let expected_ppn = pagetable as *const _ as usize >> 12;
    crate::early_println!(
        "[map_stage2] hgatp={:#x} hgatp_ppn={:#x} expected_ppn={:#x}",
        hgatp,
        hgatp_ppn,
        expected_ppn
    );

    let pte_ptr = debug_walk_stage2(pagetable, gpa as usize, asid).ok_or("walk failed")?;
    let pte = unsafe { &mut *pte_ptr };

    let ppn = (hpa as usize >> 12) & 0xffff_ffff_fff;
    pte.clear_all();
    pte.readable();
    if writable {
        pte.writable();
    }
    if accessed {
        pte.accessed();
    }
    if dirty {
        pte.dirty();
    }
    pte.executable();
    pte.set_ppn(ppn);
    pte.validate();

    crate::early_println!("[map_stage2] pte entry={:#x} ppn={:#x}", pte.entry, ppn);
    crate::early_println!("[map_stage2] pte addr={:#x}", pte as *const _ as usize);

    hfence_gvma_all();
    crate::early_println!("[map_stage2] hfence.gvma all done");
    Ok(())
}

pub fn set_guest_root_pagetable(pagetable: &PageTable, vmid: u16) {
    let ppn = pagetable as *const _ as usize >> 12;
    let mode = 9u64;
    let token = (mode << 60) | ((vmid as u64) << 44) | (ppn as u64);
    crate::early_println!(
        "[set_guest_root] pagetable={:#x} ppn={:#x} vmid={} token={:#x}",
        pagetable as *const _ as usize,
        ppn,
        vmid,
        token
    );
    unsafe {
        asm!("csrw hgatp, {0}", in(reg) token);
        asm!("hfence.gvma zero, zero");
    }
}
