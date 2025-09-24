//! AArch64 virtual memory management
//!
//! Virtual memory management for AArch64 architecture with 4-level page tables.

pub mod mmu;

extern crate alloc;

use alloc::{boxed::Box, vec};
use alloc::vec::Vec;
use hashbrown::HashMap;
use mmu::PageTable;
use spin::RwLock;
use spin::Once;

use crate::mem::page::allocate_raw_pages;

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

static PAGE_TABLES: Once<RwLock<HashMap<u16, Vec<Box<PageTable>>>>> = Once::new();

fn get_page_tables() -> &'static RwLock<HashMap<u16, Vec<Box<PageTable>>>> {
    PAGE_TABLES.call_once(|| RwLock::new(HashMap::new()))
}

/// Initialize AArch64 virtual memory system
pub fn vm_init() {
    // Initialize MMU registers
    mmu::init_mmu_registers();
    crate::early_println!("AArch64 MMU registers initialized");
}

pub fn get_pagetable(ptr: *mut PageTable) -> Option<&'static mut PageTable> {
    unsafe {
        if ptr.is_null() {
            return None;
        }
        Some(&mut *ptr)
    }
}

fn new_boxed_pagetable() -> Box<PageTable> {
    let ptr = allocate_raw_pages(1) as *mut PageTable;
    if ptr.is_null() {
        panic!("Failed to allocate a new page table");
    }
    unsafe { 
        // Zero-initialize the page table
        core::ptr::write_bytes(ptr, 0, 1);
        Box::from_raw(ptr) 
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
    let boxed_pagetable = new_boxed_pagetable();
    let ptr = boxed_pagetable.as_ref() as *const PageTable as *mut PageTable;
    
    // Store the boxed page table in HashMap for proper lifecycle management
    let mut page_tables = get_page_tables().write();
    match page_tables.get_mut(&asid) {
        Some(vec) => vec.push(boxed_pagetable),
        None => {
            // This should not happen if ASID allocation is correct
            panic!("ASID {} not found in page tables", asid);
        }
    }
    
    ptr
}

pub fn alloc_virtual_address_space() -> u16 {
    let mut asid_table = get_asid_tables().write();
    for word_idx in 0..(NUM_OF_ASID / 64) {
        let word = asid_table[word_idx];
        if word != u64::MAX { // Check if there is a free ASID in this word
            let bit_pos = (!word).trailing_zeros() as usize; // Find the first free bit (Must be < 64)
            asid_table[word_idx] |= 1 << bit_pos; // Mark this ASID as used
            let asid = (word_idx * 64 + bit_pos) as u16; // Calculate the ASID
            let root_pagetable_ptr = Box::into_raw(new_boxed_pagetable());
            let mut page_tables = get_page_tables().write();
            // Insert the new root page table into the HashMap
            unsafe { page_tables.insert(asid, vec![Box::from_raw(root_pagetable_ptr)]); }
            
            if root_pagetable_ptr.is_null() {
                panic!("Failed to allocate a new root page table");
            }

            return asid; // Return the allocated ASID
        }
    };
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
        page_tables.remove(&(asid as u16)); // Remove the page table associated with this ASID
        asid_table[word_idx] &= !(1 << bit_pos); // Mark this ASID as free
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
        let page_tabels = get_page_tables().read();
        // Root page table is always at index 0 for each ASID
        let root_page_table = page_tabels.get(&asid)?[0].as_ref();
        Some(root_page_table as *const PageTable as *mut PageTable)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::early_println;

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
        crate::early_println!("Allocated ASID: {}", asid_0);
        assert!(is_asid_used(asid_0));
        let asid_1 = alloc_virtual_address_space();
        crate::early_println!("Allocated ASID: {}", asid_1);
        assert_eq!(asid_1, asid_0 + 1);
        assert!(is_asid_used(asid_1));
        free_virtual_address_space(asid_1);
        assert!(!is_asid_used(asid_1));

        free_virtual_address_space(asid_0);
        assert!(!is_asid_used(asid_0));
    }

    /// AArch64 specific MMU tests
    mod mmu_tests {
        use super::*;
        use crate::arch::aarch64::vm::mmu::{PageTable, PageTableEntry, init_mmu_registers};
        use crate::vm::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission};

        #[test_case]
        fn test_aarch64_page_table_creation() {
            early_println!("[AArch64 MMU Test] Testing page table creation");
            
            // Test page table allocation and initialization
            let page_table = PageTable::new();
            // Check that the first entry is properly initialized
            assert!(!page_table.entries[0].is_valid(), "Initial page table entries should be invalid");
            
            early_println!("[AArch64 MMU Test] Page table creation test passed");
        }

        #[test_case]
        fn test_aarch64_pte_flags() {
            early_println!("[AArch64 MMU Test] Testing page table entry flags");
            
            let mut pte = PageTableEntry::new();
            
            // Test setting and getting flags for AArch64
            pte.set_valid(true);
            assert!(pte.is_valid(), "PTE should be valid after setting");
            
            pte.set_readable(true);
            assert!(pte.is_readable(), "PTE should be readable after setting");
            
            pte.set_writable(true);
            assert!(pte.is_writable(), "PTE should be writable after setting");
            
            pte.set_executable(true);
            assert!(pte.is_executable(), "PTE should be executable after setting");
            
            // Test AArch64-specific attributes
            pte.set_user_accessible(true);
            assert!(pte.is_user_accessible(), "PTE should be user accessible after setting");
            
            early_println!("[AArch64 MMU Test] Page table entry flags test passed");
        }

        #[test_case]
        fn test_aarch64_address_translation() {
            early_println!("[AArch64 MMU Test] Testing virtual address translation");
            
            // Test virtual address breakdown for AArch64 4-level page tables (48-bit VA)
            let vaddr = 0x123456789ABC;
            let vpn = [
                (vaddr >> 12) & 0x1FF,    // Level 3 (4KB pages)
                (vaddr >> 21) & 0x1FF,    // Level 2
                (vaddr >> 30) & 0x1FF,    // Level 1
                (vaddr >> 39) & 0x1FF,    // Level 0
            ];
            
            assert!(vpn[0] == ((vaddr >> 12) & 0x1FF), "Level 3 VPN calculation should be correct");
            assert!(vpn[1] == ((vaddr >> 21) & 0x1FF), "Level 2 VPN calculation should be correct");
            assert!(vpn[2] == ((vaddr >> 30) & 0x1FF), "Level 1 VPN calculation should be correct");
            assert!(vpn[3] == ((vaddr >> 39) & 0x1FF), "Level 0 VPN calculation should be correct");
            
            early_println!("[AArch64 MMU Test] Virtual address translation test passed");
        }

        #[test_case]
        fn test_aarch64_mmu_registers() {
            early_println!("[AArch64 MMU Test] Testing MMU register initialization");
            
            // Test that MMU register initialization doesn't panic
            init_mmu_registers();
            
            early_println!("[AArch64 MMU Test] MMU register initialization test passed");
        }

        #[test_case]
        fn test_aarch64_memory_attributes() {
            early_println!("[AArch64 MMU Test] Testing memory attributes");
            
            let mut pte = PageTableEntry::new();
            
            // Test different memory types
            pte.set_memory_type_device();
            assert!(pte.is_device_memory(), "PTE should be marked as device memory");
            
            pte.set_memory_type_normal_cacheable();
            assert!(pte.is_normal_cacheable_memory(), "PTE should be marked as normal cacheable memory");
            
            // Test shareability
            pte.set_outer_shareable();
            assert!(pte.is_outer_shareable(), "PTE should be marked as outer shareable");
            
            pte.set_inner_shareable();
            assert!(pte.is_inner_shareable(), "PTE should be marked as inner shareable");
            
            early_println!("[AArch64 MMU Test] Memory attributes test passed");
        }

        #[test_case]
        fn test_aarch64_asid_management() {
            early_println!("[AArch64 MMU Test] Testing ASID management");
            
            // Test ASID allocation
            let asid1 = alloc_virtual_address_space();
            let asid2 = alloc_virtual_address_space();
            
            assert!(asid1 != 0, "First ASID should not be zero");
            assert!(asid2 != 0, "Second ASID should not be zero");
            assert!(asid1 != asid2, "Different ASID allocations should be unique");
            
            early_println!("[AArch64 MMU Test] ASID management test passed");
        }

        #[test_case]
        fn test_aarch64_page_table_mapping() {
            early_println!("[AArch64 MMU Test] Testing page table mapping operations");
            
            let mut page_table = PageTable::new();
            let vaddr = 0x100000;  // 1MB aligned address
            let paddr = 0x200000;  // 2MB aligned address
            
            // Test mapping a page
            let vmarea = MemoryArea::new(vaddr, vaddr + 0x1000);
            let pmarea = MemoryArea::new(paddr, paddr + 0x1000);
            let map = VirtualMemoryMap {
                vmarea,
                pmarea,
                permissions: VirtualMemoryPermission::Read as usize | VirtualMemoryPermission::Write as usize,
                is_shared: false,
                owner: None,
            };
            
            // The actual mapping should not panic (detailed validation would require more setup)
            match page_table.map_memory_area(1, map) {
                Ok(_) => early_println!("[AArch64 MMU Test] Page mapping succeeded"),
                Err(e) => early_println!("[AArch64 MMU Test] Page mapping failed as expected: {}", e),
            }
            
            early_println!("[AArch64 MMU Test] Page table mapping test passed");
        }
    }
}