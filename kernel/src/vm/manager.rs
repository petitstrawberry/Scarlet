//! Virtual Memory Manager module.
//! 
//! This module provides the core functionality for managing virtual memory in the kernel.
//! It handles address space management, memory mappings, and page table operations.
//!
//! # Key Components
//!
//! - `VirtualMemoryManager`: Main structure for managing virtual memory mappings and address spaces
//! - Memory maps: Track mappings between virtual and physical memory areas
//! - ASID (Address Space ID): Identifies different address spaces
//!
//! # Functionality
//!
//! The manager enables:
//! - Creating and tracking virtual to physical memory mappings
//! - Managing different address spaces via ASIDs
//! - Searching for memory mappings by virtual address
//! - Accessing the root page table for the current address space
//!
//! # Examples
//!
//! ```
//! let mut manager = VirtualMemoryManager::new();
//! manager.set_asid(42);
//! 
//! // Add a memory mapping
//! let vma = MemoryArea { start: 0x0, end: 0x1000 };
//! let pma = MemoryArea { start: 0x80000000, end: 0x80001000 };
//! let map = VirtualMemoryMap { vmarea: vma, pmarea: pma };
//! manager.add_memory_map(map);
//! 
//! // Search for a memory mapping
//! if let Some(found_map) = manager.search_memory_map(0x500) {
//!     // Found the mapping
//! }
//!

extern crate alloc;
use alloc::vec::Vec;

use crate::{arch::vm::{get_page_table, get_root_page_table_idx, mmu::PageTable}, environment::PAGE_SIZE};

use super::vmem::VirtualMemoryMap;

#[derive(Debug, Clone)]
pub struct VirtualMemoryManager {
    memmap: Vec<VirtualMemoryMap>,
    asid: usize,
}

impl VirtualMemoryManager {
    /// Creates a new virtual memory manager.
    /// 
    /// # Returns
    /// A new virtual memory manager with default values.
    pub fn new() -> Self {
        VirtualMemoryManager {
            memmap: Vec::new(),
            asid: 0,
        }
    }

    /// Sets the ASID (Address Space ID) for the virtual memory manager.
    /// 
    /// # Arguments
    /// * `asid` - The ASID to set
    pub fn set_asid(&mut self, asid: usize) {
        self.asid = asid;
    }

    /// Returns the ASID (Address Space ID) for the virtual memory manager.
    /// 
    /// # Returns
    /// The ASID for the virtual memory manager.
    pub fn get_asid(&self) -> usize {
        self.asid
    }

    pub fn get_memmap(&self) -> &Vec<VirtualMemoryMap> {
        &self.memmap
    }

    /// Adds a memory map to the virtual memory manager.
    /// 
    /// # Arguments
    /// * `map` - The memory map to add
    pub fn add_memory_map(&mut self, map: VirtualMemoryMap) {
        // Check if the address and size is aligned
        if map.vmarea.start % PAGE_SIZE != 0 || map.pmarea.start % PAGE_SIZE != 0 ||
            map.vmarea.size() % PAGE_SIZE != 0 || map.pmarea.size() % PAGE_SIZE != 0 {
            panic!("Address or size is not aligned to PAGE_SIZE = {:#x}: {:#x?}", PAGE_SIZE, map);   
        }

        self.memmap.push(map);
    }

    /// Returns the memory map at the given index.
    /// 
    /// # Arguments
    /// * `idx` - The index of the memory map to retrieve
    /// 
    /// # Returns
    /// The memory map at the given index, if it exists.
    pub fn get_memory_map(&self, idx: usize) -> Option<&VirtualMemoryMap> {
        self.memmap.get(idx)
    }

    /// Removes the memory map at the given index.
    /// 
    /// # Arguments
    /// * `idx` - The index of the memory map to remove
    /// 
    /// # Returns
    /// The removed memory map, if it exists.
    pub fn remove_memory_map(&mut self, idx: usize) -> Option<VirtualMemoryMap> {
        if idx < self.memmap.len() {
            Some(self.memmap.remove(idx))
        } else {
            None
        }
    }

    /// Searches for a memory map containing the given virtual address.
    /// 
    /// # Arguments
    /// * `vaddr` - The virtual address to search for
    /// 
    /// # Returns
    /// The memory map containing the given virtual address, if it exists.
    pub fn search_memory_map(&self, vaddr: usize) -> Option<&VirtualMemoryMap> {
        let mut ret = None;
        for map in self.memmap.iter() {
            if map.vmarea.start <= vaddr && vaddr <= map.vmarea.end {
                ret = Some(map);
            }
        }
        ret
    }

    /// Searches for the index of a memory map containing the given virtual address.
    /// 
    /// # Arguments
    /// * `vaddr` - The virtual address to search for
    /// 
    /// # Returns
    /// The index of the memory map containing the given virtual address, if it exists.
    pub fn search_memory_map_idx(&self, vaddr: usize) -> Option<usize> {
        let mut ret = None;
        for (i, map) in self.memmap.iter().enumerate() {
            if map.vmarea.start <= vaddr && vaddr <= map.vmarea.end {
                ret = Some(i);
            }
        }
        ret
    }

    /// Returns the root page table for the current address space.
    /// 
    /// # Returns
    /// The root page table for the current address space, if it exists.
    pub fn get_root_page_table(&self) -> Option<&mut PageTable> {
        let idx = get_root_page_table_idx(self.asid);
        if let Some(root_page_table_idx) = idx {
            get_page_table(root_page_table_idx)
        } else {
            None
        }
    }

    /// Translate a virtual address to physical address
    /// 
    /// # Arguments
    /// 
    /// * `vaddr` - The virtual address to translate
    /// 
    /// # Returns
    /// 
    /// The translated physical address. Returns None if no mapping exists for the address
    pub fn translate_vaddr(&self, vaddr: usize) -> Option<usize> {
        // Search memory mapping
        for map in self.memmap.iter() {
            if vaddr >= map.vmarea.start && vaddr <= map.vmarea.end {
                // Calculate offset
                let offset = vaddr - map.vmarea.start;
                // Calculate physical address
                let paddr = map.pmarea.start + offset;
                return Some(paddr);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {    
    use crate::arch::vm::alloc_virtual_address_space;
    use crate::vm::VirtualMemoryMap;
    use crate::vm::{manager::VirtualMemoryManager, vmem::MemoryArea};

    #[test_case]
    fn test_new_virtual_memory_manager() {
        let vmm = VirtualMemoryManager::new();
        assert_eq!(vmm.get_asid(), 0);
    }

    #[test_case]
    fn test_set_and_get_asid() {
        let mut vmm = VirtualMemoryManager::new();
        vmm.set_asid(42);
        assert_eq!(vmm.get_asid(), 42);
    }

    #[test_case]
    fn test_add_and_get_memory_map() {
        let mut vmm = VirtualMemoryManager::new();
        let vma = MemoryArea { start: 0x1000, end: 0x1fff };
        let map = VirtualMemoryMap { vmarea: vma, pmarea: vma, permissions: 0 };
        vmm.add_memory_map(map);
        assert_eq!(vmm.get_memory_map(0).unwrap().vmarea.start, 0x1000);
    }

    #[test_case]
    fn test_remove_memory_map() {
        let mut vmm = VirtualMemoryManager::new();
        let vma = MemoryArea { start: 0x1000, end: 0x1fff };
        let map = VirtualMemoryMap { vmarea: vma, pmarea: vma, permissions: 0 };
        vmm.add_memory_map(map);
        let removed_map = vmm.remove_memory_map(0).unwrap();
        assert_eq!(removed_map.vmarea.start, 0x1000);
        assert!(vmm.get_memory_map(0).is_none());
    }

    #[test_case]
    fn test_search_memory_map() {
        let mut vmm = VirtualMemoryManager::new();
        let vma1 = MemoryArea { start: 0x1000, end: 0x1fff };
        let map1 = VirtualMemoryMap { vmarea: vma1, pmarea: vma1, permissions: 0 };
        let vma2 = MemoryArea { start: 0x3000, end: 0x3fff };
        let map2 = VirtualMemoryMap { vmarea: vma2, pmarea: vma2, permissions: 0 };
        vmm.add_memory_map(map1);
        vmm.add_memory_map(map2);
        let found_map = vmm.search_memory_map(0x3500).unwrap();
        assert_eq!(found_map.vmarea.start, 0x3000);
    }

    #[test_case]
    fn test_get_root_page_table() {
        let mut vmm = VirtualMemoryManager::new();
        let asid = alloc_virtual_address_space();
        vmm.set_asid(asid);
        let page_table = vmm.get_root_page_table();
        assert!(page_table.is_some());
    }
}