//! AArch64 virtual memory management
//!
//! Virtual memory management for AArch64 architecture with 4-level page tables.

pub mod mmu;

extern crate alloc;

use alloc::vec::Vec;
use alloc::{boxed::Box, vec};
use core::sync::atomic::{AtomicU64, Ordering};
use hashbrown::HashMap;
use mmu::PageTable;
use spin::Once;
use spin::RwLock;

use crate::mem::page::{allocate_raw_pages, free_raw_pages};

use crate::arch::Arch;
use crate::arch::get_cpu;
use crate::arch::get_user_trapvector_paddr;
use crate::early_println;
use crate::environment::{KERNEL_KSTACK_REGION_END, KERNEL_KSTACK_REGION_START, TRAMPOLINE_VA_END};
use crate::vm::addr::kernel_virt_to_phys;
use crate::vm::manager::VirtualMemoryManager;
use crate::vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission};

static KERNEL_TTBR0: AtomicU64 = AtomicU64::new(0);
static KERNEL_TTBR1: AtomicU64 = AtomicU64::new(0);

pub fn save_kernel_page_table() {
    let ttbr0: u64;
    let ttbr1: u64;
    unsafe {
        core::arch::asm!(
            "mrs {}, ttbr0_el1",
            "mrs {}, ttbr1_el1",
            out(reg) ttbr0,
            out(reg) ttbr1,
        );
    }
    KERNEL_TTBR0.store(ttbr0, Ordering::Release);
    KERNEL_TTBR1.store(ttbr1, Ordering::Release);
}

pub fn switch_to_kernel_page_table() {
    let ttbr0 = KERNEL_TTBR0.load(Ordering::Acquire);
    let ttbr1 = KERNEL_TTBR1.load(Ordering::Acquire);
    assert!(ttbr0 != 0, "kernel page table not initialized");
    assert!(ttbr1 != 0, "kernel TTBR1 not initialized");
    unsafe {
        core::arch::asm!(
            "msr ttbr0_el1, {}",
            "msr ttbr1_el1, {}",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            in(reg) ttbr0,
            in(reg) ttbr1,
        );
    }
}

unsafe extern "C" {
    static __TRAMPOLINE_START: usize;
    static __TRAMPOLINE_END: usize;
}

const NUM_OF_ASID: usize = u16::MAX as usize + 1; // Maximum ASID value
static ASID_BITMAP_TABLES: Once<RwLock<Box<[u64]>>> = Once::new();

fn get_asid_tables() -> &'static RwLock<Box<[u64]>> {
    ASID_BITMAP_TABLES.call_once(|| {
        // Directly allocate on heap to avoid stack overflow
        let mut tables = alloc::vec![0u64; NUM_OF_ASID / 64].into_boxed_slice();
        tables[0] = 1; // Mark the first ASID as used to avoid returning 0, which is reserved
        RwLock::new(tables)
    })
}

static PAGE_TABLES: Once<RwLock<HashMap<u16, Vec<usize>>>> = Once::new();

fn get_page_tables() -> &'static RwLock<HashMap<u16, Vec<usize>>> {
    PAGE_TABLES.call_once(|| RwLock::new(HashMap::new()))
}

pub fn get_pagetable(ptr: *mut PageTable) -> Option<&'static mut PageTable> {
    unsafe {
        if ptr.is_null() {
            return None;
        }
        Some(&mut *ptr)
    }
}

fn new_pagetable() -> *mut PageTable {
    let ptr = allocate_raw_pages(1) as *mut PageTable;
    if ptr.is_null() {
        panic!("Failed to allocate a new page table");
    }
    unsafe {
        core::ptr::write_bytes(ptr as *mut u8, 0, core::mem::size_of::<PageTable>());
        crate::arch::aarch64::clean_dcache_to_poc_range(
            ptr as usize,
            crate::environment::PAGE_SIZE,
        );
        ptr
    }
}

fn free_pagetable(ptr: *mut PageTable) {
    if !ptr.is_null() {
        crate::mem::page::free_raw_pages(ptr as *mut crate::mem::page::Page, 1);
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
/// This function is unsafe because it dereferences a raw pointer, which can lead to undefined behavior
/// if the pointer is null or invalid.
///
#[allow(static_mut_refs)]
pub unsafe fn new_raw_pagetable(asid: u16) -> *mut PageTable {
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

pub fn free_virtual_address_space(asid: u16) {
    let asid = asid as usize;
    if asid < NUM_OF_ASID {
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

pub fn get_root_pagetable_ptr(asid: u16) -> Option<*mut PageTable> {
    if is_asid_used(asid) {
        let page_tables = get_page_tables().read();
        page_tables.get(&asid).map(|vec| vec[0] as *mut PageTable)
    } else {
        None
    }
}

pub fn get_root_pagetable(asid: u16) -> Option<&'static mut PageTable> {
    let addr = get_root_pagetable_ptr(asid)?;
    unsafe {
        if addr.is_null() {
            None
        } else {
            Some(&mut *addr)
        }
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
        .map_memory_area(manager.get_asid(), trampoline_map, true, true)
        .map_err(|e| panic!("Failed to map trampoline memory area: {}", e))
        .unwrap();

    crate::vm::set_trampoline_trap_vector(trap_entry_vaddr);
    crate::vm::set_trampoline_arch(arch.get_cpuid(), arch_vaddr);
}

pub fn setup_trampoline_for_kernel(manager: &VirtualMemoryManager) {
    #[cfg(any(debug_assertions, test))]
    fn log_early_console_pte(stage: &str, manager: &VirtualMemoryManager) {
        let console_vaddr = crate::earlyfb::console_lock_addr();
        let leaf = manager
            .get_root_page_table()
            .and_then(|root| root.walk(console_vaddr, false, manager.get_asid()))
            .map(|pte| pte.entry & 0xfff);

        match leaf {
            Some(bits) => crate::early_println!(
                "[vm] {} EARLY_CONSOLE leaf bits: {:#x} (va={:#x})",
                stage,
                bits,
                console_vaddr
            ),
            None => crate::early_println!(
                "[vm] {} EARLY_CONSOLE leaf missing (va={:#x})",
                stage,
                console_vaddr
            ),
        }
    }

    setup_trampoline_at_end(manager, TRAMPOLINE_VA_END);

    // Sanity check: the per-task kernel stack windows are part of the same high-VA
    // (trampoline-managed) TTBR1 address space.
    #[cfg(any(debug_assertions, test))]
    {
        crate::early_println!(
            "[vm] aarch64 high-va(kstack) region: {:#x}-{:#x}",
            KERNEL_KSTACK_REGION_START,
            KERNEL_KSTACK_REGION_END
        );
        debug_assert!(KERNEL_KSTACK_REGION_START <= KERNEL_KSTACK_REGION_END);
        debug_assert!(KERNEL_KSTACK_REGION_END < TRAMPOLINE_VA_END);
    }

    // Keep TTBR1 fixed to the kernel page table (trampoline/high-VA live there).
    #[cfg(any(debug_assertions, test))]
    crate::early_println!("[vm] setup_trampoline_for_kernel: switch_ttbr1...");
    #[cfg(any(debug_assertions, test))]
    log_early_console_pte("pre-switch", manager);
    mmu::sync_el1_translation_registers_if_needed();
    manager
        .get_root_page_table()
        .expect("Kernel root page table not set")
        .switch_ttbr1(manager.get_asid());
    #[cfg(any(debug_assertions, test))]
    log_early_console_pte("post-switch", manager);
    #[cfg(any(debug_assertions, test))]
    crate::early_println!("[vm] setup_trampoline_for_kernel: switch_ttbr1 ok");
}

/// AArch64: trampoline/high-VA live in the fixed TTBR1 kernel mapping.
/// Per-task TTBR0 page tables should not pre-map the trampoline.
pub fn setup_trampoline_for_user(_manager: &VirtualMemoryManager) {}

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
        let ptr = unsafe { new_raw_pagetable(asid) };
        let page_table = get_pagetable(ptr);
        assert!(page_table.is_some());
        free_virtual_address_space(asid);
    }

    #[test_case]
    fn test_get_root_page_table_idx() {
        let asid = alloc_virtual_address_space();
        let root_page_table_idx = get_root_pagetable(asid as u16);
        assert!(root_page_table_idx.is_some());
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
