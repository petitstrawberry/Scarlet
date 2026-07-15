//! Virtual memory module for RISC-V architecture.
//!
//! The virtual memory module is responsible for managing the virtual memory of the system.
//! The module provides functions to initialize the virtual memory system, map physical memory to
//! virtual memory, and switch page tables.
//!

pub mod mmu;

extern crate alloc;

use alloc::vec::Vec;
use alloc::{boxed::Box, vec};
use hashbrown::HashMap;
use mmu::PageTable;
#[cfg(test)]
use mmu::PageTableEntry;
use spin::{Mutex, MutexGuard, Once, RwLock};

use core::sync::atomic::{AtomicU64, Ordering};

use crate::mem::page::{Page, allocate_raw_pages, allocate_raw_pages_aligned, free_raw_pages};

use crate::arch::Arch;
use crate::arch::get_cpu;
use crate::arch::get_user_trapvector_paddr;
use crate::early_println;
use crate::environment::{KERNEL_KSTACK_REGION_END, KERNEL_KSTACK_REGION_START, TRAMPOLINE_VA_END};
use crate::vm::addr::kernel_virt_to_phys;
use crate::vm::manager::VirtualMemoryManager;
use crate::vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission};

static KERNEL_SATP: AtomicU64 = AtomicU64::new(0);

pub fn save_kernel_page_table() {
    let satp: u64;
    unsafe {
        core::arch::asm!("csrr {}, satp", out(reg) satp);
    }
    KERNEL_SATP.store(satp, Ordering::Release);
}

pub fn switch_to_kernel_page_table() {
    let satp = KERNEL_SATP.load(Ordering::Acquire);
    assert!(satp != 0, "kernel page table not initialized");
    unsafe {
        core::arch::asm!(
            "csrw satp, {}",
            "sfence.vma",
            in(reg) satp,
        );
    }
}

unsafe extern "C" {
    static __TRAMPOLINE_START: usize;
    static __TRAMPOLINE_END: usize;
}

const NUM_OF_ASID: usize = u16::MAX as usize + 1; // Maximum ASID value
static ASID_BITMAP_TABLES: Once<RwLock<Box<[u64]>>> = Once::new();
static PAGE_TABLE_LOCKS: [Mutex<()>; NUM_OF_ASID] =
    [const { Mutex::new(()) }; NUM_OF_ASID];

/// Exclusive access to one ASID's stage-1 page-table hierarchy.
pub struct RootPageTableGuard {
    asid: u16,
    table: *mut PageTable,
    _guard: MutexGuard<'static, ()>,
}

impl RootPageTableGuard {
    fn table(&self) -> &PageTable {
        // SAFETY: the ASID registry owns the root while `_guard` prevents teardown.
        unsafe { &*self.table }
    }

    fn table_mut(&mut self) -> &mut PageTable {
        // SAFETY: `_guard` is the unique lock for this ASID's complete hierarchy.
        unsafe { &mut *self.table }
    }

    /// Returns the root page-table address for diagnostics.
    ///
    /// # Returns
    ///
    /// The virtual address of the guarded root page table.
    pub(crate) fn root_address(&self) -> usize {
        self.table as usize
    }

    pub(crate) fn switch(&self) {
        self.table().switch(self.asid);
    }

    pub(crate) fn get_val_for_satp(&self) -> u64 {
        self.table().get_val_for_satp(self.asid)
    }

    pub(crate) fn map_memory_area(
        &mut self,
        mmap: VirtualMemoryMap,
        accessed: bool,
        dirty: bool,
    ) -> Result<(), &'static str> {
        let asid = self.asid;
        self.table_mut()
            .map_memory_area(asid, mmap, accessed, dirty)
    }

    pub(crate) fn map(
        &mut self,
        vaddr: usize,
        paddr: usize,
        flags: usize,
        user: bool,
        write: bool,
    ) {
        let asid = self.asid;
        self.table_mut()
            .map(asid, vaddr, paddr, flags, user, write);
    }

    #[cfg(test)]
    pub(crate) fn translate(&mut self, vaddr: usize) -> Option<usize> {
        self.table_mut().translate(vaddr)
    }

    pub(crate) fn unmap_range(&mut self, vaddr_start: usize, vaddr_end: usize) {
        let asid = self.asid;
        self.table_mut().unmap_range(asid, vaddr_start, vaddr_end);
    }

    pub(crate) fn unmap_all(&mut self) {
        self.table_mut().unmap_all();
    }

    #[cfg(test)]
    pub(crate) fn walk_to_level(
        &mut self,
        vaddr: usize,
        level: usize,
        alloc: bool,
    ) -> Option<&mut PageTableEntry> {
        let asid = self.asid;
        self.table_mut().walk_to_level(vaddr, level, alloc, asid)
    }
}

fn get_asid_tables() -> &'static RwLock<Box<[u64]>> {
    ASID_BITMAP_TABLES.call_once(|| {
        // Directly allocate on heap to avoid stack overflow
        let mut tables = alloc::vec![0u64; NUM_OF_ASID / 64].into_boxed_slice();
        tables[0] = 1; // Mark the first ASID as used to avoid returning 0, which is reserved
        RwLock::new(tables)
    })
}
// static mut ROOT_PAGE_TABLES: Lazy<RwLock<HashMap<u16, *mut PageTable>>> = Lazy::new(|| RwLock::new(HashMap::new()));
static PAGE_TABLES: Once<RwLock<HashMap<u16, Vec<usize>>>> = Once::new();

fn get_page_tables() -> &'static RwLock<HashMap<u16, Vec<usize>>> {
    PAGE_TABLES.call_once(|| RwLock::new(HashMap::new()))
}

fn new_pagetable() -> *mut PageTable {
    let ptr = allocate_raw_pages(1) as *mut PageTable;
    if ptr.is_null() {
        panic!("Failed to allocate a new page table");
    }
    ptr
}

fn free_pagetable(ptr: *mut PageTable) {
    if !ptr.is_null() {
        free_raw_pages(ptr as *mut Page, 1);
    }
}

/// Allocates a new raw page table for the given ASID.
///
/// # Arguments
/// * `asid` - The Address Space ID (ASID) for which the page table is allocated.
///
/// # Returns
/// A raw pointer to the newly allocated page table.
///
/// # Safety
///
/// The caller must hold the [`RootPageTableGuard`] for `asid` until the returned
/// table has been published into that guarded hierarchy.
///
#[allow(static_mut_refs)]
unsafe fn new_raw_pagetable(asid: u16) -> *mut PageTable {
    let ptr = new_pagetable();

    let mut page_tables = get_page_tables().write();
    match page_tables.get_mut(&asid) {
        Some(vec) => vec.push(ptr as usize),
        None => {
            panic!("ASID {} not found in page tables", asid);
        }
    }

    ptr
}

pub fn alloc_virtual_address_space() -> u16 {
    let mut asid_table = get_asid_tables().write();
    for word_idx in 0..(NUM_OF_ASID / 64) {
        let word = asid_table[word_idx];
        if word != u64::MAX {
            let bit_pos = (!word).trailing_zeros() as usize;
            asid_table[word_idx] |= 1 << bit_pos;
            let asid = (word_idx * 64 + bit_pos) as u16;
            let root_pagetable_ptr = new_pagetable();
            let mut page_tables = get_page_tables().write();
            page_tables.insert(asid, vec![root_pagetable_ptr as usize]);

            if root_pagetable_ptr.is_null() {
                panic!("Failed to allocate a new root page table");
            }

            return asid;
        }
    }
    panic!("No available root page table");
}

pub fn alloc_virtual_address_space_for_stage2() -> u16 {
    let mut asid_table = get_asid_tables().write();
    for word_idx in 0..(NUM_OF_ASID / 64) {
        let word = asid_table[word_idx];
        if word != u64::MAX {
            let bit_pos = (!word).trailing_zeros() as usize;
            asid_table[word_idx] |= 1 << bit_pos;
            let asid = (word_idx * 64 + bit_pos) as u16;
            let ptr = allocate_raw_pages_aligned(4, 16384) as *mut PageTable;
            if ptr.is_null() {
                panic!("Failed to allocate 16KiB aligned root page table");
            }
            assert!(
                ptr as usize % 16384 == 0,
                "Allocated page table is not 16KiB aligned"
            );
            let mut page_tables = get_page_tables().write();
            page_tables.insert(asid, vec![ptr as usize]);
            return asid;
        }
    }
    panic!("No available root page table");
}

pub fn free_virtual_address_space(asid: u16) {
    let asid = asid as usize;
    if asid < NUM_OF_ASID {
        let _page_table_guard = PAGE_TABLE_LOCKS[asid].lock();
        let bit_pos = asid % 64;
        let word_idx = asid / 64;
        let mut asid_table = get_asid_tables().write();
        if asid_table[word_idx] & (1 << bit_pos) == 0 {
            panic!("ASID {} is already free", asid);
        }
        let mut page_tables = get_page_tables().write();
        if let Some(tables) = page_tables.remove(&(asid as u16)) {
            for addr in tables {
                free_pagetable(addr as *mut PageTable);
            }
        }
        asid_table[word_idx] &= !(1 << bit_pos);
    } else {
        panic!("Invalid ASID: {}", asid);
    }
}

pub fn is_asid_used(asid: u16) -> bool {
    let asid = asid as usize;
    if asid < NUM_OF_ASID {
        let word_idx = asid / 64;
        let bit_pos = asid % 64;
        let asid_table = get_asid_tables().read();
        (asid_table[word_idx] & (1 << bit_pos)) != 0
    } else {
        false
    }
}

fn get_root_pagetable_ptr(asid: u16) -> Option<*mut PageTable> {
    if is_asid_used(asid) {
        let page_tables = get_page_tables().read();
        page_tables.get(&asid).map(|vec| vec[0] as *mut PageTable)
    } else {
        None
    }
}

pub fn get_root_pagetable(asid: u16) -> Option<RootPageTableGuard> {
    let guard = PAGE_TABLE_LOCKS[asid as usize].lock();
    let addr = get_root_pagetable_ptr(asid)?;
    if addr.is_null() {
        None
    } else {
        Some(RootPageTableGuard {
            asid,
            table: addr,
            _guard: guard,
        })
    }
}

fn setup_trampoline_at_end(manager: &VirtualMemoryManager, trampoline_vaddr_end: usize) {
    let trampoline_start =
        kernel_virt_to_phys(unsafe { &__TRAMPOLINE_START as *const usize as usize });
    let trampoline_end =
        kernel_virt_to_phys(unsafe { &__TRAMPOLINE_END as *const usize as usize }) - 1;
    let trampoline_size = trampoline_end - trampoline_start;

    let arch = get_cpu().as_paddr_cpu();
    let trampoline_vaddr_start = trampoline_vaddr_end - trampoline_size;

    let trap_entry_paddr = kernel_virt_to_phys(get_user_trapvector_paddr());
    let arch_paddr = kernel_virt_to_phys(arch as *const Arch as usize);
    let trap_entry_offset = trap_entry_paddr - trampoline_start;
    let arch_offset = arch_paddr - trampoline_start;

    let trap_entry_vaddr = trampoline_vaddr_start + trap_entry_offset;
    let arch_vaddr = trampoline_vaddr_start + arch_offset;

    #[cfg(any(debug_assertions, test))]
    {
        early_println!(
            "Trampoline space planned  : {:#x} - {:#x}",
            trampoline_vaddr_start,
            trampoline_vaddr_end
        );
        early_println!(
            "  Trampoline paddr        : {:#x} - {:#x}",
            trampoline_start,
            trampoline_end
        );
        early_println!("  Trap entry paddr        : {:#x}", trap_entry_paddr);
        early_println!("  Arch paddr              : {:#x}", arch_paddr);
        early_println!("  Trap entry vaddr        : {:#x}", trap_entry_vaddr);
        early_println!("  Arch vaddr              : {:#x}", arch_vaddr);
    }

    let trampoline_map = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: trampoline_vaddr_start,
            end: trampoline_vaddr_end,
        },
        pmarea: MemoryArea {
            start: trampoline_start,
            end: trampoline_end,
        },
        vm_start: trampoline_vaddr_start,
        permissions: VirtualMemoryPermission::Read as usize
            | VirtualMemoryPermission::Write as usize
            | VirtualMemoryPermission::Execute as usize,
        is_shared: true,
        memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
        owner: None,
    };

    if let Err(e) = manager.add_memory_map(trampoline_map.clone()) {
        #[cfg(any(debug_assertions, test))]
        {
            early_println!("[vm] add trampoline map failed: {}", e);
            if let Some(m) = manager.search_memory_map(trampoline_vaddr_start) {
                early_println!(
                    "[vm] map@trampoline_start: {:#x}-{:#x}",
                    m.vmarea.start,
                    m.vmarea.end
                );
            } else {
                early_println!("[vm] map@trampoline_start: <none>");
            }
            if let Some(m) = manager.search_memory_map(trampoline_vaddr_end) {
                early_println!(
                    "[vm] map@trampoline_end  : {:#x}-{:#x}",
                    m.vmarea.start,
                    m.vmarea.end
                );
            } else {
                early_println!("[vm] map@trampoline_end  : <none>");
            }
            manager.with_memmaps(|mm| {
                early_println!("[vm] current VMA count   : {}", mm.len());
                for (_k, m) in mm.iter() {
                    early_println!("[vm]   VMA {:#x}-{:#x}", m.vmarea.start, m.vmarea.end);
                }
            });
        }
        panic!("Failed to add trampoline memory map: {}", e);
    }

    manager
        .get_root_page_table()
        .unwrap()
        .map_memory_area(trampoline_map, true, true)
        .map_err(|e| panic!("Failed to map trampoline memory area: {}", e))
        .unwrap();

    crate::vm::set_trampoline_trap_vector(trap_entry_vaddr);
    crate::vm::set_trampoline_arch(arch.get_cpuid(), arch_vaddr);
}

pub fn setup_trampoline_for_kernel(manager: &VirtualMemoryManager) {
    setup_trampoline_at_end(manager, TRAMPOLINE_VA_END);

    #[cfg(any(debug_assertions, test))]
    {
        crate::println!(
            "[vm] riscv64 high-va(kstack) region: {:#x}-{:#x}",
            KERNEL_KSTACK_REGION_START,
            KERNEL_KSTACK_REGION_END
        );
        debug_assert!(KERNEL_KSTACK_REGION_START <= KERNEL_KSTACK_REGION_END);
        debug_assert!(KERNEL_KSTACK_REGION_END < TRAMPOLINE_VA_END);
    }
}

pub fn setup_trampoline_for_user(manager: &VirtualMemoryManager) {
    setup_trampoline_at_end(manager, TRAMPOLINE_VA_END);
}

pub fn register_trampoline_for_ap() {
    let trampoline_start =
        kernel_virt_to_phys(unsafe { &__TRAMPOLINE_START as *const usize as usize });
    let trampoline_end =
        kernel_virt_to_phys(unsafe { &__TRAMPOLINE_END as *const usize as usize }) - 1;
    let trampoline_size = trampoline_end - trampoline_start;

    let arch = get_cpu().as_paddr_cpu();
    let trampoline_vaddr_start = TRAMPOLINE_VA_END - trampoline_size;
    let arch_paddr = kernel_virt_to_phys(arch as *const Arch as usize);
    let arch_offset = arch_paddr - trampoline_start;
    let arch_vaddr = trampoline_vaddr_start + arch_offset;

    crate::vm::set_trampoline_arch(arch.get_cpuid(), arch_vaddr);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_get_page_table() {
        let asid = alloc_virtual_address_space();
        let root = get_root_pagetable(asid).expect("root page table not found");
        assert_ne!(root.root_address(), 0);
        drop(root);
        free_virtual_address_space(asid);
    }

    #[test_case]
    fn test_get_root_page_table_idx() {
        let asid = alloc_virtual_address_space();
        let root_page_table_idx = get_root_pagetable(asid as u16);
        assert!(root_page_table_idx.is_some());
        drop(root_page_table_idx);
        free_virtual_address_space(asid);
    }

    #[test_case]
    fn test_alloc_virtual_address_space() {
        let asid_0 = alloc_virtual_address_space();
        crate::println!("Allocated ASID: {}", asid_0);
        assert!(is_asid_used(asid_0));
        let asid_1 = alloc_virtual_address_space();
        crate::println!("Allocated ASID: {}", asid_1);
        assert_eq!(asid_1, asid_0 + 1);
        assert!(is_asid_used(asid_1));
        free_virtual_address_space(asid_1);
        assert!(!is_asid_used(asid_1));

        free_virtual_address_space(asid_0);
        assert!(!is_asid_used(asid_0));
    }
}
