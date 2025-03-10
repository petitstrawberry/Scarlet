//! Virtual memory module for RISC-V architecture.
//! 
//! The virtual memory module is responsible for managing the virtual memory of the system.
//! The module provides functions to initialize the virtual memory system, map physical memory to
//! virtual memory, and switch page tables.
//! 

pub mod mmu;

extern crate alloc;

use mmu::PageTable;

const NUM_OF_MAX_PAGE_TABLE: usize = 512;
static mut PAGE_TABLES: [PageTable; NUM_OF_MAX_PAGE_TABLE] = [const { PageTable::new() }; NUM_OF_MAX_PAGE_TABLE];
static mut PAGE_TABLES_USED: [bool; NUM_OF_MAX_PAGE_TABLE] = [false; NUM_OF_MAX_PAGE_TABLE];

const NUM_OF_MAX_ROOT_PAGE_TABLE: usize = 16;
static mut ROOT_PAGE_TABLES: [usize; NUM_OF_MAX_ROOT_PAGE_TABLE] = [0; NUM_OF_MAX_ROOT_PAGE_TABLE];
static mut ROOT_PAGE_TABLES_USED: [bool; NUM_OF_MAX_ROOT_PAGE_TABLE] = [false; NUM_OF_MAX_ROOT_PAGE_TABLE];

pub fn new_page_table_idx() -> usize {
    unsafe {
        for i in 0..NUM_OF_MAX_PAGE_TABLE{
            if !PAGE_TABLES_USED[i] {
                PAGE_TABLES_USED[i] = true;
                return i;
            }
        }
        panic!("No available page table");
    }
}

pub fn get_page_table(index: usize) -> Option<&'static mut PageTable> {
    unsafe {
        if PAGE_TABLES_USED[index] {
            Some(&mut PAGE_TABLES[index])
        } else {
            None
        }
    }
}

pub fn alloc_virtual_address_space() -> usize {
    unsafe {
        for i in 0..NUM_OF_MAX_ROOT_PAGE_TABLE {
            if !ROOT_PAGE_TABLES_USED[i] {
                ROOT_PAGE_TABLES_USED[i] = true;
                ROOT_PAGE_TABLES[i] = new_page_table_idx();
                return i;
            }
        }
        panic!("No available root page table");
    }
}

pub fn get_root_page_table_idx(asid: usize) -> Option<usize> {
    unsafe {
        if ROOT_PAGE_TABLES_USED[asid] {
            Some(ROOT_PAGE_TABLES[asid])
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_get_page_table() {
        let idx = new_page_table_idx();
        let page_table = get_page_table(idx);
        assert!(page_table.is_some());
    }
    
    #[test_case]
    fn test_get_root_page_table_idx() {
        let asid = alloc_virtual_address_space();
        let root_page_table_idx = get_root_page_table_idx(asid);
        assert!(root_page_table_idx.is_some());
    }
}