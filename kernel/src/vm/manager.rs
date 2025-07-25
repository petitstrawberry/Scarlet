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
use alloc::{sync::Arc, vec::Vec, collections::BTreeMap};

use crate::{arch::vm::{free_virtual_address_space, get_root_pagetable, is_asid_used, mmu::PageTable}, environment::PAGE_SIZE};

use super::vmem::VirtualMemoryMap;

#[derive(Debug, Clone)]
pub struct VirtualMemoryManager {
    memmap: BTreeMap<usize, VirtualMemoryMap>, // start_addr -> VirtualMemoryMap
    asid: u16,
    mmap_base: usize,           // mmap領域のベースアドレス
    page_tables: Vec<Arc<PageTable>>,
}

impl VirtualMemoryManager {
    /// Creates a new virtual memory manager.
    /// 
    /// # Returns
    /// A new virtual memory manager with default values.
    pub fn new() -> Self {
        VirtualMemoryManager {
            memmap: BTreeMap::new(),
            asid: 0,
            mmap_base: 0x40000000, // 1GB位置から開始（デフォルト）
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

    /// Returns an iterator over all memory maps.
    /// This is the preferred way to iterate over memory maps.
    /// 
    /// # Returns
    /// An iterator over references to all memory maps.
    pub fn memmap_iter(&self) -> impl Iterator<Item = &VirtualMemoryMap> {
        self.memmap.values()
    }

    /// Returns a mutable iterator over all memory maps.
    /// 
    /// # Returns
    /// A mutable iterator over references to all memory maps.
    pub fn memmap_iter_mut(&mut self) -> impl Iterator<Item = &mut VirtualMemoryMap> {
        self.memmap.values_mut()
    }

    /// Returns the number of memory maps.
    /// 
    /// # Returns
    /// The number of memory maps.
    pub fn memmap_len(&self) -> usize {
        self.memmap.len()
    }

    /// Returns true if there are no memory maps.
    /// 
    /// # Returns
    /// True if there are no memory maps.
    pub fn memmap_is_empty(&self) -> bool {
        self.memmap.is_empty()
    }

    /// Gets a memory map by its start address.
    /// 
    /// # Arguments
    /// * `start_addr` - The start address of the memory map
    /// 
    /// # Returns
    /// The memory map with the given start address, if it exists.
    pub fn get_memory_map_by_addr(&self, start_addr: usize) -> Option<&VirtualMemoryMap> {
        self.memmap.get(&start_addr)
    }

    /// Gets a mutable memory map by its start address.
    /// 
    /// # Arguments
    /// * `start_addr` - The start address of the memory map
    /// 
    /// # Returns
    /// The mutable memory map with the given start address, if it exists.
    pub fn get_memory_map_by_addr_mut(&mut self, start_addr: usize) -> Option<&mut VirtualMemoryMap> {
        self.memmap.get_mut(&start_addr)
    }

    /// Adds a memory map to the virtual memory manager.
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

        // Check for overlapping mappings
        for existing_map in self.memmap.values() {
            if !(map.vmarea.end < existing_map.vmarea.start || map.vmarea.start > existing_map.vmarea.end) {
                return Err("Memory mapping overlaps with existing mapping");
            }
        }

        self.memmap.insert(map.vmarea.start, map);
        Ok(())
    }

    /// Returns the memory map at the given index.
    /// 
    /// # Arguments
    /// * `idx` - The index of the memory map to retrieve
    /// 
    /// # Returns
    /// The memory map at the given index, if it exists.
    /// 
    /// # Note
    /// This method is deprecated and inefficient for BTreeMap. 
    /// Use `memmap_iter()` for iteration or `search_memory_map()` for address-based lookup.
    #[deprecated(note = "Use memmap_iter() or search_memory_map() instead")]
    pub fn get_memory_map(&self, idx: usize) -> Option<&VirtualMemoryMap> {
        self.memmap.values().nth(idx)
    }

    /// Removes the memory map at the given index.
    /// 
    /// # Arguments
    /// * `idx` - The index of the memory map to remove
    /// 
    /// # Returns
    /// The removed memory map, if it exists.
    /// 
    /// # Note
    /// This method is deprecated and inefficient for BTreeMap.
    /// Use `remove_memory_map_by_addr()` for address-based removal.
    #[deprecated(note = "Use remove_memory_map_by_addr() instead")]
    pub fn remove_memory_map(&mut self, idx: usize) -> Option<VirtualMemoryMap> {
        if let Some((start_addr, _)) = self.memmap.iter().nth(idx) {
            let start_addr = *start_addr;
            self.memmap.remove(&start_addr)
        } else {
            None
        }
    }

    /// Removes the memory map containing the given virtual address.
    /// 
    /// # Arguments
    /// * `vaddr` - The virtual address contained in the memory map to remove
    /// 
    /// # Returns
    /// The removed memory map, if it exists.
    pub fn remove_memory_map_by_addr(&mut self, vaddr: usize) -> Option<VirtualMemoryMap> {
        // Find the start address of the memory map containing vaddr
        for (&start_addr, map) in self.memmap.iter() {
            if map.vmarea.start <= vaddr && vaddr <= map.vmarea.end {
                return self.memmap.remove(&start_addr);
            }
        }
        None
    }

    /// Removes all memory maps.
    /// 
    /// # Returns
    /// The removed memory maps.
    /// 
    /// # Note
    /// This method returns an iterator instead of a cloned Vec for efficiency.
    pub fn remove_all_memory_maps(&mut self) -> impl Iterator<Item = VirtualMemoryMap> {
        let memmap = core::mem::take(&mut self.memmap);
        memmap.into_values()
    }

    /// Restores the memory maps from a given iterator.
    ///
    /// # Arguments
    /// * `maps` - The iterator of memory maps to restore
    /// 
    /// # Returns
    /// A result indicating success or failure.
    /// 
    pub fn restore_memory_maps<I>(&mut self, maps: I) -> Result<(), &'static str> 
    where 
        I: IntoIterator<Item = VirtualMemoryMap>
    {
        for map in maps {
            if let Err(e) = self.add_memory_map(map) {
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
        for (_, map) in self.memmap.iter() {
            if map.vmarea.start <= vaddr && vaddr <= map.vmarea.end {
                return Some(map);
            }
        }
        None
    }

    /// Searches for the index of a memory map containing the given virtual address.
    /// 
    /// # Arguments
    /// * `vaddr` - The virtual address to search for
    /// 
    /// # Returns
    /// The index of the memory map containing the given virtual address, if it exists.
    /// 
    /// # Note
    /// This method is deprecated for BTreeMap as indices are not meaningful.
    /// Use `search_memory_map()` for direct address-based lookup.
    #[deprecated(note = "Use search_memory_map() instead - indices are not meaningful for BTreeMap")]
    pub fn search_memory_map_idx(&self, vaddr: usize) -> Option<usize> {
        for (i, (_, map)) in self.memmap.iter().enumerate() {
            if map.vmarea.start <= vaddr && vaddr <= map.vmarea.end {
                return Some(i);
            }
        }
        None
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
        for (_, map) in self.memmap.iter() {
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
        vmm.add_memory_map(map).unwrap();
        
        // Use new efficient API instead of deprecated get_memory_map(0)
        assert_eq!(vmm.memmap_len(), 1);
        let first_map = vmm.memmap_iter().next().unwrap();
        assert_eq!(first_map.vmarea.start, 0x1000);
        
        // Test direct address-based access
        assert!(vmm.get_memory_map_by_addr(0x1000).is_some());
        assert_eq!(vmm.get_memory_map_by_addr(0x1000).unwrap().vmarea.start, 0x1000);
    }

    #[test_case]
    fn test_remove_memory_map() {
        let mut vmm = VirtualMemoryManager::new();
        let vma = MemoryArea { start: 0x1000, end: 0x1fff };
        let map = VirtualMemoryMap { vmarea: vma, pmarea: vma, permissions: 0, is_shared: false };
        vmm.add_memory_map(map).unwrap();
        
        // Use address-based removal instead of index-based
        let removed_map = vmm.remove_memory_map_by_addr(0x1000).unwrap();
        assert_eq!(removed_map.vmarea.start, 0x1000);
        
        // Verify removal using efficient API
        assert!(vmm.memmap_is_empty());
        assert_eq!(vmm.memmap_len(), 0);
        assert!(vmm.get_memory_map_by_addr(0x1000).is_none());
    }

    #[test_case]
    fn test_search_memory_map() {
        let mut vmm = VirtualMemoryManager::new();
        let vma1 = MemoryArea { start: 0x1000, end: 0x1fff };
        let map1 = VirtualMemoryMap { vmarea: vma1, pmarea: vma1, permissions: 0, is_shared: false };
        let vma2 = MemoryArea { start: 0x3000, end: 0x3fff };
        let map2 = VirtualMemoryMap { vmarea: vma2, pmarea: vma2, permissions: 0, is_shared: false };
        vmm.add_memory_map(map1).unwrap();
        vmm.add_memory_map(map2).unwrap();
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