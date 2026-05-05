//! Guest MMU management for RISC-V H-extension

use alloc::vec::Vec;
use core::arch::asm;
use hashbrown::HashMap;
use spin::{Once, RwLock};

use crate::arch::vm::mmu::{PageTable, PageTableEntry};
use crate::mem::page::{allocate_raw_pages, allocate_raw_pages_aligned, free_raw_pages};
use crate::vm::addr::{phys_to_virt, virt_to_phys};

const PAGE_SIZE: usize = 4096;
const STAGE2_ROOT_SIZE: usize = 16384;

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

#[repr(align(16384))]
#[derive(Debug)]
pub struct Stage2PageTable {
    pub entries: [PageTableEntry; 2048],
}

impl Stage2PageTable {
    pub const fn new() -> Self {
        Stage2PageTable {
            entries: [PageTableEntry::new(); 2048],
        }
    }
}

fn allocate_stage2_root() -> *mut Stage2PageTable {
    let ptr = allocate_raw_pages_aligned(STAGE2_ROOT_SIZE / PAGE_SIZE, STAGE2_ROOT_SIZE);
    if ptr.is_null() {
        return ptr as *mut Stage2PageTable;
    }
    ptr as *mut Stage2PageTable
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

pub fn hfence_gvma_all() {
    unsafe {
        asm!("hfence.gvma zero, zero");
    }
}

pub fn read_hgatp() -> u64 {
    let val: u64;
    unsafe {
        asm!("csrr {0}, hgatp", out(reg) val);
    }
    val
}

pub fn verify_hgatp_stage2(expected_pagetable: &Stage2PageTable, vmid: u16) {
    let hgatp = read_hgatp();
    let expected_ppn = virt_to_phys(expected_pagetable as *const _ as usize) >> 12;
    let actual_ppn = hgatp & 0xffff_ffff_fff;
    let actual_vmid = (hgatp >> 44) & 0xffff;

    crate::println!(
        "[verify_hgatp_stage2] expected_ppn={:#x} actual_ppn={:#x} vmid={}",
        expected_ppn,
        actual_ppn,
        actual_vmid
    );
}

pub fn set_guest_root_stage2(pagetable: &Stage2PageTable, vmid: u16) {
    let ppn = virt_to_phys(pagetable as *const _ as usize) >> 12;
    let token = (9u64 << 60) | ((vmid as u64) << 44) | (ppn as u64);
    // crate::println!(
    //     "[set_guest_root_stage2] ppn={:#x} vmid={} token={:#x}",
    //     ppn,
    //     vmid,
    //     token
    // );
    unsafe {
        asm!("csrw hgatp, {0}", in(reg) token);
        asm!("hfence.gvma zero, zero");
    }
}

pub fn walk_stage2(
    pagetable: &mut Stage2PageTable,
    gpa: usize,
    vmid: u16,
) -> Option<*mut PageTableEntry> {
    let mut current_table = pagetable.entries.as_mut_ptr();

    let vpn3 = (gpa >> 39) & 0x7ff;
    let pte = unsafe { &mut *current_table.add(vpn3) };

    // crate::println!(
    //     "[walk_stage2] L3 vpn={} pte={:#x} valid={}",
    //     vpn3,
    //     pte.entry,
    //     pte.is_valid()
    // );

    if !pte.is_valid() {
        let new_table = allocate_stage2_table(vmid);
        if new_table.is_null() {
            return None;
        }
        pte.set_ppn(virt_to_phys(new_table as usize) >> 12);
        pte.validate();
    }
    current_table = phys_to_virt(pte.get_ppn() << 12) as *mut PageTableEntry;

    for level in (1..=2).rev() {
        let vpn = (gpa >> (12 + 9 * level)) & 0x1ff;
        let pte = unsafe { &mut *current_table.add(vpn) };

        // crate::println!(
        //     "[walk_stage2] L{} vpn={} pte={:#x} valid={}",
        //     level,
        //     vpn,
        //     pte.entry,
        //     pte.is_valid()
        // );

        if !pte.is_valid() {
            let new_table = allocate_stage2_table(vmid);
            if new_table.is_null() {
                return None;
            }
            pte.set_ppn(virt_to_phys(new_table as usize) >> 12);
            pte.validate();
        }
        current_table = phys_to_virt(pte.get_ppn() << 12) as *mut PageTableEntry;
    }

    let vpn = (gpa >> 12) & 0x1ff;
    let final_pte = unsafe { current_table.add(vpn) };
    // crate::println!(
    //     "[walk_stage2] L0 vpn={} pte={:#x}",
    //     vpn,
    //     unsafe { *final_pte }.entry
    // );
    Some(final_pte)
}

pub fn map_stage2_page_new(
    pagetable: &mut Stage2PageTable,
    gpa: u64,
    hpa: u64,
    writable: bool,
    vmid: u16,
) -> Result<(), &'static str> {
    let gpa = gpa as usize & !0xfff;
    let hpa = hpa as usize & !0xfff;

    // crate::println!("[map_stage2_new] gpa={:#x} hpa={:#x}", gpa, hpa);

    let pte = walk_stage2(pagetable, gpa, vmid).ok_or("walk failed")?;

    let ppn = (hpa >> 12) & 0xffff_ffff_fff;
    unsafe {
        (*pte).entry = 0;
        (*pte).entry |= 1;
        (*pte).entry |= 2;
        if writable {
            (*pte).entry |= 4;
        }
        (*pte).entry |= 8;
        (*pte).entry |= 0x10;
        (*pte).entry |= 0x40;
        (*pte).entry |= (ppn as u64) << 10;
    }

    // crate::println!("[map_stage2_new] pte={:#x}", unsafe { *pte }.entry);
    hfence_gvma_all();
    Ok(())
}
