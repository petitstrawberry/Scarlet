use alloc::alloc::{alloc_zeroed, dealloc, Layout};
use alloc::boxed::Box;
use alloc::vec::Vec;
use hashbrown::HashMap;
use spin::{Once, RwLock};

use crate::arch::vm::mmu::{PageTable, PageTableEntry};

const PAGE_SIZE: usize = 4096;
const STAGE2_ROOT_SIZE: usize = PAGE_SIZE;

static STAGE2_ROOTS: Once<RwLock<HashMap<u16, usize>>> = Once::new();
static STAGE2_TABLES: Once<RwLock<HashMap<u16, Vec<Box<PageTable>>>>> = Once::new();

fn get_stage2_roots() -> &'static RwLock<HashMap<u16, usize>> {
    STAGE2_ROOTS.call_once(|| RwLock::new(HashMap::new()))
}

fn get_stage2_tables() -> &'static RwLock<HashMap<u16, Vec<Box<PageTable>>>> {
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
    get_stage2_tables().write().remove(&vmid);
    if let Some(root) = get_stage2_roots().write().remove(&vmid) {
        let layout = Layout::from_size_align(STAGE2_ROOT_SIZE, STAGE2_ROOT_SIZE).unwrap();
        unsafe {
            dealloc(root as *mut u8, layout);
        }
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

fn allocate_stage2_root() -> *mut Stage2PageTable {
    let layout = Layout::from_size_align(STAGE2_ROOT_SIZE, STAGE2_ROOT_SIZE).unwrap();
    unsafe { alloc_zeroed(layout) as *mut Stage2PageTable }
}

fn allocate_stage2_table(vmid: u16) -> *mut PageTable {
    let layout = Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap();
    let ptr = unsafe { alloc_zeroed(layout) as *mut PageTable };
    if ptr.is_null() {
        return ptr;
    }
    let boxed = unsafe { Box::from_raw(ptr) };
    let raw = boxed.as_ref() as *const PageTable as *mut PageTable;
    if let Some(vec) = get_stage2_tables().write().get_mut(&vmid) {
        vec.push(boxed);
    }
    raw
}

pub fn verify_hgatp_stage2(_expected_pagetable: &Stage2PageTable, _vmid: u16) {
    todo!("verify_hgatp_stage2 not implemented for aarch64")
}

pub fn set_guest_root_stage2(_pagetable: &Stage2PageTable, _vmid: u16) {
    todo!("set_guest_root_stage2 not implemented for aarch64")
}

pub fn walk_stage2(
    pagetable: &mut Stage2PageTable,
    gpa: usize,
    vmid: u16,
) -> Option<*mut PageTableEntry> {
    let mut current_table = pagetable.entries.as_mut_ptr();

    let vpn0 = (gpa >> 12) & 0x1ff;
    let vpn1 = (gpa >> 21) & 0x1ff;
    let vpn2 = (gpa >> 30) & 0x1ff;

    let pte2 = unsafe { &mut *current_table.add(vpn2) };
    if !pte2.is_valid() {
        let new_table = allocate_stage2_table(vmid);
        if new_table.is_null() {
            return None;
        }
        pte2.set_ppn(new_table as usize >> 12);
        pte2.validate();
    }
    current_table = (pte2.get_ppn() << 12) as *mut PageTableEntry;

    let pte1 = unsafe { &mut *current_table.add(vpn1) };
    if !pte1.is_valid() {
        let new_table = allocate_stage2_table(vmid);
        if new_table.is_null() {
            return None;
        }
        pte1.set_ppn(new_table as usize >> 12);
        pte1.validate();
    }
    current_table = (pte1.get_ppn() << 12) as *mut PageTableEntry;

    let final_pte = unsafe { current_table.add(vpn0) };
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

    Ok(())
}
