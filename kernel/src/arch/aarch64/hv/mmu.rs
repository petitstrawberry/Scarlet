use core::arch::asm;

use alloc::vec::Vec;
use hashbrown::HashMap;
use spin::{Once, RwLock};

use crate::arch::vm::mmu::{PageTable, PageTableEntry};
use crate::mem::page::{allocate_raw_pages, allocate_raw_pages_aligned, free_raw_pages};
use crate::vm::addr::{phys_to_virt, virt_to_phys};

const PAGE_SIZE: usize = 4096;
const STAGE2_ROOT_SIZE: usize = PAGE_SIZE * 2;

/// ARM DDI 0487 stage-2 descriptor bits.
const S2_VALID: u64 = 1 << 0;
const S2_TABLE: u64 = 1 << 1;
const S2_PAGE: u64 = 1 << 1;
const S2_ATTR_SHIFT: u64 = 2;
const S2_AP_SHIFT: u64 = 6;
const S2_SH_SHIFT: u64 = 8;
const S2_AF: u64 = 1 << 10;
const S2_ADDR_SHIFT: u64 = 12;
const S2_PXN: u64 = 1 << 53;
const S2_XN: u64 = 1 << 54;

const S2_ADDR_MASK: u64 = 0x0000_ffff_ffff_f000;
const S2_VTTBR_VMID_SHIFT: u64 = 48;
const S2_VTTBR_VMID_MASK: u64 = 0xff;

const S2_AP_RW: u64 = 0b11 << S2_AP_SHIFT;
const S2_AP_RO: u64 = 0b01 << S2_AP_SHIFT;
const S2_SH_IS: u64 = 0b11 << S2_SH_SHIFT;
const S2_ATTR_NORMAL_WB: u64 = 0b111 << S2_ATTR_SHIFT;

static STAGE2_ROOTS: Once<RwLock<HashMap<u16, usize>>> = Once::new();
static STAGE2_TABLES: Once<RwLock<HashMap<u16, Vec<usize>>>> = Once::new();

fn get_stage2_roots() -> &'static RwLock<HashMap<u16, usize>> {
    STAGE2_ROOTS.call_once(|| RwLock::new(HashMap::new()))
}

fn get_stage2_tables() -> &'static RwLock<HashMap<u16, Vec<usize>>> {
    STAGE2_TABLES.call_once(|| RwLock::new(HashMap::new()))
}

pub fn alloc_vmid() -> u16 {
    static VMID_COUNTER: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(1);
    VMID_COUNTER.fetch_add(1, core::sync::atomic::Ordering::SeqCst)
}

pub fn init_stage2(vmid: u16) -> Result<(), &'static str> {
    let root = allocate_stage2_root();
    if root.is_null() {
        return Err("Failed to allocate Stage2 root");
    }
    get_stage2_roots().write().insert(vmid, root as usize);
    get_stage2_tables().write().insert(vmid, Vec::new());
    Ok(())
}

pub fn free_stage2(vmid: u16) {
    if let Some(tables) = get_stage2_tables().write().remove(&vmid) {
        for addr in tables {
            free_raw_pages(addr as *mut crate::mem::page::Page, 1);
        }
    }
    if let Some(root) = get_stage2_roots().write().remove(&vmid) {
        free_raw_pages(
            root as *mut crate::mem::page::Page,
            STAGE2_ROOT_SIZE / PAGE_SIZE,
        );
    }
}

pub fn get_stage2_root(vmid: u16) -> Option<*mut Stage2PageTable> {
    get_stage2_roots()
        .read()
        .get(&vmid)
        .map(|&addr| addr as *mut Stage2PageTable)
}

#[repr(align(4096))]
#[derive(Debug)]
pub struct Stage2PageTable {
    pub entries: [PageTableEntry; 512],
}

impl Stage2PageTable {
    pub const fn new() -> Self {
        Stage2PageTable {
            entries: [PageTableEntry::new(); 512],
        }
    }
}

impl Default for Stage2PageTable {
    fn default() -> Self {
        Self::new()
    }
}

fn allocate_stage2_root() -> *mut Stage2PageTable {
    allocate_raw_pages_aligned(STAGE2_ROOT_SIZE / PAGE_SIZE, STAGE2_ROOT_SIZE)
        as *mut Stage2PageTable
}

fn allocate_stage2_table(vmid: u16) -> *mut PageTable {
    let ptr = allocate_raw_pages(1) as *mut PageTable;
    if ptr.is_null() {
        return ptr;
    }
    if let Some(vec) = get_stage2_tables().write().get_mut(&vmid) {
        vec.push(ptr as usize);
    }
    ptr
}

#[inline(always)]
fn table_descriptor(table_pa: usize) -> u64 {
    S2_VALID | S2_TABLE | ((table_pa as u64) & S2_ADDR_MASK)
}

#[inline(always)]
fn stage2_page_descriptor(hpa: u64, writable: bool) -> u64 {
    S2_VALID
        | S2_PAGE
        | S2_ATTR_NORMAL_WB
        | if writable { S2_AP_RW } else { S2_AP_RO }
        | S2_SH_IS
        | S2_AF
        | (hpa & S2_ADDR_MASK)
}

#[inline(always)]
fn stage2_index(gpa: usize, level: usize) -> usize {
    match level {
        1 => (gpa >> 30) & 0x3ff,
        2 => (gpa >> 21) & 0x1ff,
        3 => (gpa >> 12) & 0x1ff,
        _ => unreachable!("invalid stage-2 level"),
    }
}

#[inline(always)]
fn descriptor_output_pa(entry: u64) -> usize {
    (entry & S2_ADDR_MASK) as usize
}

pub fn verify_hgatp_stage2(expected_pagetable: &Stage2PageTable, vmid: u16) {
    let vttbr_el2: u64;

    // SAFETY: reading VTTBR_EL2 is valid while running at EL2 in VHE mode.
    unsafe {
        asm!("mrs {vttbr_el2}, vttbr_el2", vttbr_el2 = out(reg) vttbr_el2, options(nostack));
    }

    let expected_root =
        (virt_to_phys(expected_pagetable as *const _ as usize) as u64) & S2_ADDR_MASK;
    let actual_root = vttbr_el2 & S2_ADDR_MASK;
    let actual_vmid = ((vttbr_el2 >> S2_VTTBR_VMID_SHIFT) & S2_VTTBR_VMID_MASK) as u16;

    let expected_vmid = vmid & S2_VTTBR_VMID_MASK as u16;
    if actual_root == expected_root && actual_vmid == expected_vmid {
        crate::println!(
            "[verify_hgatp_stage2] verified root={:#x} vmid={}",
            actual_root,
            actual_vmid
        );
    } else {
        crate::println!(
            "[verify_hgatp_stage2] mismatch expected_root={:#x} actual_root={:#x} expected_vmid={} actual_vmid={}",
            expected_root,
            actual_root,
            expected_vmid,
            actual_vmid
        );
    }
}

pub fn set_guest_root_stage2(pagetable: &Stage2PageTable, vmid: u16) {
    let root_pa = (virt_to_phys(pagetable as *const _ as usize) as u64) & S2_ADDR_MASK;
    let vttbr_el2 = (((vmid as u64) & S2_VTTBR_VMID_MASK) << S2_VTTBR_VMID_SHIFT) | root_pa;

    // SAFETY: writing VTTBR_EL2 and issuing TLB maintenance is valid while the
    // host kernel executes at EL2 in VHE mode.
    unsafe {
        asm!(
            "msr vttbr_el2, {vttbr_el2}",
            "isb",
            "dsb ish",
            "tlbi vmalls12e1is",
            "dsb ish",
            "isb",
            vttbr_el2 = in(reg) vttbr_el2,
            options(nostack),
        );
    }
}

pub fn walk_stage2(
    pagetable: &mut Stage2PageTable,
    gpa: usize,
    vmid: u16,
) -> Option<*mut PageTableEntry> {
    let mut current_table = pagetable as *mut Stage2PageTable as *mut PageTableEntry;

    for level in 1..=2 {
        let index = stage2_index(gpa, level);

        // SAFETY: current_table always points to a valid stage-2 page-table page
        // allocated by this module, and index is constrained to 0..512.
        let pte = unsafe { &mut *current_table.add(index) };

        if pte.is_valid() {
            if (pte.entry & (S2_VALID | S2_TABLE)) != (S2_VALID | S2_TABLE) {
                return None;
            }
        } else {
            let new_table = allocate_stage2_table(vmid);
            if new_table.is_null() {
                return None;
            }
            let new_table_pa = virt_to_phys(new_table as usize);
            pte.entry = table_descriptor(new_table_pa);
            crate::arch::aarch64::clean_dcache_to_poc_range(
                (pte as *const PageTableEntry) as usize,
                core::mem::size_of::<PageTableEntry>(),
            );
        }

        current_table = phys_to_virt(descriptor_output_pa(pte.entry)) as *mut PageTableEntry;
    }

    let index = stage2_index(gpa, 3);
    // SAFETY: current_table points to the final L3 table and index is within range.
    Some(unsafe { current_table.add(index) })
}

pub fn create_stage2_page_mapping(
    pagetable: &mut Stage2PageTable,
    gpa: u64,
    hpa: u64,
    writable: bool,
    vmid: u16,
) -> Result<(), &'static str> {
    let page_mask = (1u64 << S2_ADDR_SHIFT) - 1;
    let gpa = gpa & !page_mask;
    let hpa = hpa & !page_mask;
    let pte = walk_stage2(pagetable, gpa as usize, vmid).ok_or("walk failed")?;

    // SAFETY: walk_stage2 returns a valid pointer to the target L3 entry.
    unsafe {
        if ((*pte).entry & S2_VALID) != 0 && ((*pte).entry & S2_PAGE) == 0 {
            return Err("Cannot replace existing stage2 page table with a leaf");
        }
        (*pte).entry = stage2_page_descriptor(hpa, writable);
    }

    crate::arch::aarch64::clean_dcache_to_poc_range(
        pte as usize,
        core::mem::size_of::<PageTableEntry>(),
    );

    // SAFETY: the updated stage-2 mapping must be made visible before reuse.
    unsafe {
        asm!(
            "dsb ish",
            "tlbi vmalls12e1is",
            "dsb ish",
            "isb",
            options(nostack)
        );
    }

    Ok(())
}

pub fn map_stage2_page_new(
    pagetable: &mut Stage2PageTable,
    gpa: u64,
    hpa: u64,
    writable: bool,
    vmid: u16,
) -> Result<(), &'static str> {
    create_stage2_page_mapping(pagetable, gpa, hpa, writable, vmid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_stage2_page_descriptor_bits() {
        let descriptor = stage2_page_descriptor(0x1234_5000, true);

        assert_eq!(descriptor & S2_VALID, S2_VALID);
        assert_eq!(descriptor & S2_PAGE, S2_PAGE);
        assert_eq!(descriptor & (0b111 << S2_ATTR_SHIFT), S2_ATTR_NORMAL_WB);
        assert_eq!(descriptor & (0b11 << S2_AP_SHIFT), S2_AP_RW);
        assert_eq!(descriptor & (0b11 << S2_SH_SHIFT), S2_SH_IS);
        assert_eq!(descriptor & S2_AF, S2_AF);
        assert_eq!(descriptor & S2_ADDR_MASK, 0x1234_5000);
        assert_eq!(descriptor & S2_XN, 0);
        assert_eq!(descriptor & S2_PXN, 0);
    }

    #[test_case]
    fn test_stage2_root_uses_40bit_ipa_top_bit() {
        assert_eq!(stage2_index(0, 1), 0);
        assert_eq!(stage2_index(1usize << 39, 1), 512);
    }
}
