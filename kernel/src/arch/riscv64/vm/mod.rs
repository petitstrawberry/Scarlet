//! Virtual memory module for RISC-V architecture.
//! 
//! The virtual memory module is responsible for managing the virtual memory of the system.
//! The module provides functions to initialize the virtual memory system, map physical memory to
//! virtual memory, and switch page tables.
//! 

pub mod mmu;

extern crate alloc;

use alloc::{boxed::Box, vec};
use alloc::vec::Vec;
use hashbrown::HashMap;
use mmu::PageTable;
use spin::RwLock;
use spin::lazy::Lazy;

use crate::mem::page::allocate_raw_pages;

const NUM_OF_ASID: usize = u16::MAX as usize + 1; // Maximum ASID value
static mut ASID_BITMAP_TABLES: Lazy<RwLock<[u64; NUM_OF_ASID / 64]>> = Lazy::new(|| {
    let mut tables = [0; NUM_OF_ASID / 64];
    tables[0] = 1; // Mark the first ASID as used to avoid returning 0, which is reserved
    RwLock::new(tables)
});
// static mut ROOT_PAGE_TABLES: Lazy<RwLock<HashMap<u16, *mut PageTable>>> = Lazy::new(|| RwLock::new(HashMap::new()));
static mut PAGE_TABLES: Lazy<RwLock<HashMap<u16, Vec<Box<PageTable>>>>> = Lazy::new(|| RwLock::new(HashMap::new()));

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
    unsafe { Box::from_raw(ptr) }
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
    let ptr = Box::into_raw(boxed_pagetable);
    let mut page_tables = unsafe { PAGE_TABLES.write() };
    page_tables.get_mut(&asid).expect("Invalid ASID").push(
        unsafe { Box::from_raw(ptr) } // Store the boxed page table in the HashMap
    );
    // Ensure the pointer is valid and return it
    ptr
}

#[allow(static_mut_refs)]
pub fn alloc_virtual_address_space() -> u16 {
    unsafe {
        let mut asid_table = ASID_BITMAP_TABLES.write();
        for word_idx in 0..(NUM_OF_ASID / 64) {
            let word = asid_table[word_idx];
            if word != u64::MAX { // Check if there is a free ASID in this word
                let bit_pos = (!word).trailing_zeros() as usize; // Find the first free bit (Must be < 64)
                asid_table[word_idx] |= 1 << bit_pos; // Mark this ASID as used
                let asid = (word_idx * 64 + bit_pos) as u16; // Calculate the ASID
                let root_pagetable_ptr = Box::into_raw(new_boxed_pagetable());
                let mut page_tables = PAGE_TABLES.write();
                // Insert the new root page table into the HashMap
                page_tables.insert(asid, vec![Box::from_raw(root_pagetable_ptr)]);
                
                if root_pagetable_ptr.is_null() {
                    panic!("Failed to allocate a new root page table");
                }

                return asid; // Return the allocated ASID
            }
        };
        panic!("No available root page table");
    }
}

#[allow(static_mut_refs)]
pub fn free_virtual_address_space(asid: u16) {
    unsafe {
        let asid = asid as usize;
        if asid < NUM_OF_ASID {
            let bit_pos = asid % 64;
            let word_idx = asid / 64;
            let mut asid_table = ASID_BITMAP_TABLES.write();
            if asid_table[word_idx] & (1 << bit_pos) == 0 {
                panic!("ASID {} is already free", asid);
            }
            let mut page_tables = PAGE_TABLES.write();
            page_tables.remove(&(asid as u16)); // Remove the page table associated with this ASID
            asid_table[word_idx] &= !(1 << bit_pos); // Mark this ASID as free
        } else {
            panic!("Invalid ASID: {}", asid);
        }
    }
}

#[allow(static_mut_refs)]
pub fn is_asid_used(asid: u16) -> bool {
    unsafe {
        let asid = asid as usize;
        if asid < NUM_OF_ASID {
            let word_idx = asid / 64;
            let bit_pos = asid % 64;
            let asid_table = ASID_BITMAP_TABLES.read();
            (asid_table[word_idx] & (1 << bit_pos)) != 0
        } else {
            false
        }
    }
}

#[allow(static_mut_refs)]
pub fn get_root_pagetable_ptr(asid: u16) -> Option<*mut PageTable> {
    unsafe {
        if is_asid_used(asid) {
            let page_tabels = PAGE_TABLES.read();
            // Root page table is always at index 0 for each ASID
            let root_page_table = page_tabels.get(&asid)?[0].as_ref();
            Some(root_page_table as *const PageTable as *mut PageTable)

        } else {
            None
        }
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
}