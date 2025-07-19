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
use alloc::{sync::Arc, vec::Vec};

use crate::{arch::vm::{free_virtual_address_space, get_root_pagetable, is_asid_used, mmu::PageTable}, environment::PAGE_SIZE};

use super::vmem::VirtualMemoryMap;

#[derive(Debug, Clone)]
pub struct VirtualMemoryManager {
    memmap: Vec<VirtualMemoryMap>,
    asid: u16,
    page_tables: Vec<Arc<PageTable>>,
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
            page_tables: Vec::new(),
        }
    }

    /// Sets the ASID (Address Space ID) for the virtual memory manager.
    /// 
    /// # Arguments
    /// * `asid` - The ASID to set
    pub fn set_asid(&mut self, asid: u16) {
        if self.asid == asid {
            return; // No change needed
        }
        if self.asid != 0 && is_asid_used(self.asid) {
            // Free the previous address space if it exists
            free_virtual_address_space(self.asid);
        }
        self.asid = asid;
    }

    /// Returns the ASID (Address Space ID) for the virtual memory manager.
    /// 
    /// # Returns
    /// The ASID for the virtual memory manager.
    pub fn get_asid(&self) -> u16 {
        self.asid
    }

    pub fn get_memmap(&self) -> &Vec<VirtualMemoryMap> {
        &self.memmap
    }

    /// Checks if a virtual memory range overlaps with existing mappings.
    /// 
    /// # Arguments
    /// * `start` - The start address of the range to check
    /// * `end` - The end address of the range to check (inclusive)
    /// 
    /// # Returns
    /// Some(VirtualMemoryMap) if there is an overlap, None otherwise
    pub fn check_overlap(&self, start: usize, end: usize) -> Option<&VirtualMemoryMap> {
        for map in self.memmap.iter() {
            // Check if ranges overlap: [start, end] vs [map.start, map.end]
            if start <= map.vmarea.end && end >= map.vmarea.start {
                return Some(map);
            }
        }
        None
    }

    /// Adds a memory map to the virtual memory manager with overlap checking.
    /// 
    /// This method performs overlap detection before adding the mapping.
    /// Use this for:
    /// - User-initiated memory allocation (mmap, malloc, etc.)
    /// - Dynamic memory allocation where overlap is possible
    /// - Any case where memory range conflicts are uncertain
    /// 
    /// # Arguments
    /// * `map` - The memory map to add
    /// 
    /// # Returns
    /// A result indicating success or failure.
    /// 
    pub fn add_memory_map(&mut self, map: VirtualMemoryMap) -> Result<(), &'static str> {
        // Check if the address and size is aligned
        if map.vmarea.start % PAGE_SIZE != 0 || map.pmarea.start % PAGE_SIZE != 0 ||
            map.vmarea.size() % PAGE_SIZE != 0 || map.pmarea.size() % PAGE_SIZE != 0 {
            return Err("Address or size is not aligned to PAGE_SIZE");
        }

        // Check for overlaps
        if let Some(existing_map) = self.check_overlap(map.vmarea.start, map.vmarea.end) {
            crate::println!("Memory overlap detected: new range [{:#x}-{:#x}] overlaps with existing [{:#x}-{:#x}]", 
                map.vmarea.start, map.vmarea.end, existing_map.vmarea.start, existing_map.vmarea.end);
            return Err("Memory range overlaps with existing mapping");
        }

        self.memmap.push(map);
        Ok(())
    }

    /// Adds a memory map to the virtual memory manager.
    /// 
    /// # Safety
    /// This method does NOT check for overlaps. Use this only when you are certain
    /// that the memory range does not overlap with existing mappings, such as:
    /// - System initialization (kernel, device mappings)
    /// - Internal memory management operations (splitting existing ranges)
    /// - Operations on empty/new address spaces
    /// 
    /// For user-initiated memory allocation or uncertain cases, use `add_memory_map`.
    /// 
    /// # Arguments
    /// * `map` - The memory map to add
    /// 
    /// # Returns
    /// A result indicating success or failure.
    /// 
    pub fn add_memory_map_unchecked(&mut self, map: VirtualMemoryMap) -> Result<(), &'static str> {
        // Check if the address and size is aligned
        if map.vmarea.start % PAGE_SIZE != 0 || map.pmarea.start % PAGE_SIZE != 0 ||
            map.vmarea.size() % PAGE_SIZE != 0 || map.pmarea.size() % PAGE_SIZE != 0 {
            return Err("Address or size is not aligned to PAGE_SIZE");
        }

        self.memmap.push(map);
        Ok(())
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

    /// Removes all memory maps.
    /// 
    /// # Returns
    /// The removed memory maps.
    pub fn remove_all_memory_maps(&mut self) -> Vec<VirtualMemoryMap> {
        let mut removed_maps = Vec::new();
        while !self.memmap.is_empty() {
            removed_maps.push(self.memmap.remove(0));
        }
        removed_maps
    }

    /// Restores the memory maps from a given vector.
    ///
    /// # Arguments
    /// * `maps` - The vector of memory maps to restore
    /// 
    /// # Returns
    /// A result indicating success or failure.
    /// 
    pub fn restore_memory_maps(&mut self, maps: Vec<VirtualMemoryMap>) -> Result<(), &'static str> {
        for map in maps {
            if let Err(e) = self.add_memory_map_unchecked(map) {
                return Err(e);
            }
        }
        Ok(())
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

    /// Adds a page table to the virtual memory manager.
    pub fn add_page_table(&mut self, page_table: Arc<PageTable>) {
        self.page_tables.push(page_table);
    }

    /// Returns the root page table for the current address space.
    /// 
    /// # Returns
    /// The root page table for the current address space, if it exists.
    pub fn get_root_page_table(&self) -> Option<&mut PageTable> {
        get_root_pagetable(self.asid)
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

impl Drop for VirtualMemoryManager {
    /// Drops the virtual memory manager, freeing the address space if it is still in use.
    fn drop(&mut self) {
        if self.asid != 0 && is_asid_used(self.asid) {
            // Free the address space if it is still in use
            free_virtual_address_space(self.asid);
        }
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
        let map = VirtualMemoryMap { vmarea: vma, pmarea: vma, permissions: 0, is_shared: false };
        vmm.add_memory_map_unchecked(map).unwrap();
        assert_eq!(vmm.get_memory_map(0).unwrap().vmarea.start, 0x1000);
    }

    #[test_case]
    fn test_remove_memory_map() {
        let mut vmm = VirtualMemoryManager::new();
        let vma = MemoryArea { start: 0x1000, end: 0x1fff };
        let map = VirtualMemoryMap { vmarea: vma, pmarea: vma, permissions: 0, is_shared: false };
        vmm.add_memory_map_unchecked(map).unwrap();
        let removed_map = vmm.remove_memory_map(0).unwrap();
        assert_eq!(removed_map.vmarea.start, 0x1000);
        assert!(vmm.get_memory_map(0).is_none());
    }

    #[test_case]
    fn test_search_memory_map() {
        let mut vmm = VirtualMemoryManager::new();
        let vma1 = MemoryArea { start: 0x1000, end: 0x1fff };
        let map1 = VirtualMemoryMap { vmarea: vma1, pmarea: vma1, permissions: 0, is_shared: false };
        let vma2 = MemoryArea { start: 0x3000, end: 0x3fff };
        let map2 = VirtualMemoryMap { vmarea: vma2, pmarea: vma2, permissions: 0, is_shared: false };
        vmm.add_memory_map_unchecked(map1).unwrap();
        vmm.add_memory_map_unchecked(map2).unwrap();
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

    #[test_case]
    fn test_overlap_detection() {
        let mut vmm = VirtualMemoryManager::new();
        let vma1 = MemoryArea { start: 0x1000, end: 0x1fff };
        let map1 = VirtualMemoryMap { vmarea: vma1, pmarea: vma1, permissions: 0, is_shared: false };
        vmm.add_memory_map_unchecked(map1).unwrap();

        // Test exact overlap
        let overlapping_vma = MemoryArea { start: 0x1500, end: 0x2fff };
        assert!(vmm.check_overlap(overlapping_vma.start, overlapping_vma.end).is_some());

        // Test no overlap
        let non_overlapping_vma = MemoryArea { start: 0x3000, end: 0x3fff };
        assert!(vmm.check_overlap(non_overlapping_vma.start, non_overlapping_vma.end).is_none());
        
        // Test add with overlap check - should fail
        let overlapping_map = VirtualMemoryMap { vmarea: overlapping_vma, pmarea: overlapping_vma, permissions: 0, is_shared: false };
        assert!(vmm.add_memory_map(overlapping_map).is_err());
        
        // Test add with no overlap - should succeed
        let non_overlapping_map = VirtualMemoryMap { vmarea: non_overlapping_vma, pmarea: non_overlapping_vma, permissions: 0, is_shared: false };
        assert!(vmm.add_memory_map(non_overlapping_map).is_ok());
    }
}