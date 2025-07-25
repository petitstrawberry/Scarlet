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
//! let vm_area = MemoryArea { start: 0x0, end: 0x1000 };
//! let pm_area = MemoryArea { start: 0x80000000, end: 0x80001000 };
//! let map = VirtualMemoryMap { vmarea: vm_area, pmarea: pm_area };
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
    mmap_base: usize,        // Mmap from this base address
    page_tables: Vec<Arc<PageTable>>,
    
    /// Cache for the last searched memory map to accelerate repeated accesses
    /// Format: (start_addr, end_addr, map_start_key)
    last_search_cache: Option<(usize, usize, usize)>,
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
            mmap_base: 0x40000000, // 1 GB base address for mmap (Default)
            page_tables: Vec::new(),
            last_search_cache: None,
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
    /// This method uses efficient overlap detection with ordered data structures.
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

        // Efficient overlap detection using BTreeMap's ordered nature
        // Instead of linear scan, we only check nearby memory maps using range queries
        
        // Check for overlapping mappings in the relevant range
        let range_end = map.vmarea.end;
        
        // Find any memory map that might overlap by checking:
        // 1. Maps that start before or at our range_end
        // 2. Maps whose range extends past our range_start
        for (_, existing_map) in self.memmap.range(..range_end) {
            // Check if there's an actual overlap
            if !(map.vmarea.end < existing_map.vmarea.start || map.vmarea.start > existing_map.vmarea.end) {
                return Err("Memory mapping overlaps with existing mapping");
            }
        }
        
        // Also check maps that start at or after our range_end (they might extend backwards)
        for (_, existing_map) in self.memmap.range(range_end..) {
            // Early termination: if this map starts after our range ends, no more overlaps possible
            if existing_map.vmarea.start > range_end {
                break;
            }
            // Check overlap
            if !(map.vmarea.end < existing_map.vmarea.start || map.vmarea.start > existing_map.vmarea.end) {
                return Err("Memory mapping overlaps with existing mapping");
            }
        }

        // Clear cache as the memory layout has changed
        self.last_search_cache = None;
        
        // Insert the new mapping
        self.memmap.insert(map.vmarea.start, map);
        Ok(())
    }

    /// Removes the memory map containing the given virtual address.
    /// 
    /// This method uses efficient search with caching to locate the target mapping.
    /// 
    /// # Arguments
    /// * `vaddr` - The virtual address contained in the memory map to remove
    /// 
    /// # Returns
    /// The removed memory map, if it exists.
    pub fn remove_memory_map_by_addr(&mut self, vaddr: usize) -> Option<VirtualMemoryMap> {
        // Use our efficient search to find the memory map
        let start_addr = self.find_memory_map_with_cache_update(vaddr)?;
        
        // Clear cache since we're removing this memory map
        if let Some((_, _, cache_key)) = self.last_search_cache {
            if cache_key == start_addr {
                self.last_search_cache = None;
            }
        }
        
        // Remove and return the memory map
        self.memmap.remove(&start_addr)
    }

    /// Removes all memory maps.
    /// 
    /// # Returns
    /// The removed memory maps.
    /// 
    /// # Note
    /// This method returns an iterator instead of a cloned Vec for efficiency.
    pub fn remove_all_memory_maps(&mut self) -> impl Iterator<Item = VirtualMemoryMap> {
        // Clear cache since all mappings are being removed
        self.last_search_cache = None;
        
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
    /// Implements caching for efficient range search in memory mappings.
    /// 
    /// # Arguments
    /// * `vaddr` - The virtual address to search for
    /// 
    /// # Returns
    /// The memory map containing the given virtual address, if it exists.
    pub fn search_memory_map(&self, vaddr: usize) -> Option<&VirtualMemoryMap> {
        // Optimization: Check cache first (O(1))
        if let Some((cache_start, cache_end, cache_key)) = self.last_search_cache {
            if cache_start <= vaddr && vaddr <= cache_end {
                // Cache hit! Return the cached result
                return self.memmap.get(&cache_key);
            }
        }
        
        // Cache miss: Use BTreeMap's efficient range search (O(log n))
        self.find_memory_map_optimized(vaddr)
    }
    
    /// Efficient memory map search using BTreeMap's ordered nature
    /// 
    /// This method uses the ordered property of BTreeMap to efficiently find
    /// the memory mapping containing the given address.
    /// 
    /// # Arguments
    /// * `vaddr` - Virtual address to search for
    /// 
    /// # Returns
    /// The memory map containing the address, if found
    fn find_memory_map_optimized(&self, vaddr: usize) -> Option<&VirtualMemoryMap> {
        // Strategy: Use BTreeMap's range() to find the rightmost memory map that could contain vaddr
        // This avoids scanning all maps by leveraging the sorted nature of BTreeMap
        
        // Find all maps with start_addr <= vaddr, then check the last one
        // This is O(log n) to find the position + O(log n) to verify
        if let Some((_start_addr, map)) = self.memmap.range(..=vaddr).next_back() {
            if map.vmarea.start <= vaddr && vaddr <= map.vmarea.end {
                return Some(map);
            }
        }
        
        None
    }
    
    /// Searches for a memory map containing the given virtual address (mutable version).
    /// 
    /// This version allows mutable access and updates the search cache.
    /// 
    /// # Arguments
    /// * `vaddr` - The virtual address to search for
    /// 
    /// # Returns
    /// Mutable reference to the memory map containing the given virtual address, if it exists.
    pub fn search_memory_map_mut(&mut self, vaddr: usize) -> Option<&mut VirtualMemoryMap> {
        // First, find the memory map (this might update cache)
        let result_key = self.find_memory_map_with_cache_update(vaddr)?;
        
        // Return mutable reference
        self.memmap.get_mut(&result_key)
    }
    
    /// Helper method that finds memory map and updates cache
    /// 
    /// # Arguments
    /// * `vaddr` - Virtual address to search for
    /// 
    /// # Returns
    /// The start address key of the found memory map, if any
    fn find_memory_map_with_cache_update(&mut self, vaddr: usize) -> Option<usize> {
        // Check cache first
        if let Some((cache_start, cache_end, cache_key)) = self.last_search_cache {
            if cache_start <= vaddr && vaddr <= cache_end {
                return Some(cache_key);
            }
        }
        
        // Find memory map using BTreeMap range search
        if let Some((start_addr, map)) = self.memmap.range(..=vaddr).next_back() {
            if map.vmarea.start <= vaddr && vaddr <= map.vmarea.end {
                // Update cache for future searches
                self.last_search_cache = Some((map.vmarea.start, map.vmarea.end, *start_addr));
                return Some(*start_addr);
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
    /// This method uses efficient search with caching for optimal performance.
    /// 
    /// # Arguments
    /// 
    /// * `vaddr` - The virtual address to translate
    /// 
    /// # Returns
    /// 
    /// The translated physical address. Returns None if no mapping exists for the address
    pub fn translate_vaddr(&self, vaddr: usize) -> Option<usize> {
        // Use our optimized search method
        if let Some(map) = self.search_memory_map(vaddr) {
            // Calculate offset within the memory area
            let offset = vaddr - map.vmarea.start;
            // Calculate and return physical address
            Some(map.pmarea.start + offset)
        } else {
            None
        }
    }

    /// Gets the mmap base address
    /// 
    /// # Returns
    /// The base address for mmap operations
    pub fn get_mmap_base(&self) -> usize {
        self.mmap_base
    }
    
    /// Sets the mmap base address
    /// This allows dynamic adjustment of the mmap region
    /// 
    /// # Arguments
    /// * `base` - New base address for mmap operations
    pub fn set_mmap_base(&mut self, base: usize) {
        self.mmap_base = base;
    }
    
    /// Find a suitable address for new memory mapping
    /// 
    /// # Arguments
    /// * `size` - Size of the mapping needed
    /// * `alignment` - Required alignment (typically PAGE_SIZE)
    /// 
    /// # Returns
    /// A suitable virtual address for the new mapping, or None if no space available
    pub fn find_unmapped_area(&self, size: usize, alignment: usize) -> Option<usize> {
        let aligned_size = (size + alignment - 1) & !(alignment - 1);
        
        // Start search from mmap_base
        let mut search_addr = self.mmap_base;
        
        // Simple first-fit algorithm
        for (_start, memory_map) in self.memmap.range(self.mmap_base..) {
            // Check if there's enough space before this memory map
            if search_addr + aligned_size <= memory_map.vmarea.start {
                return Some(search_addr);
            }
            
            // Move search point past this memory map
            search_addr = memory_map.vmarea.end + 1;
            
            // Align the search address
            search_addr = (search_addr + alignment - 1) & !(alignment - 1);
        }
        
        // Check if there's space after the last memory map
        // For simplicity, we assume a reasonable upper limit for the address space
        const MAX_USER_ADDR: usize = 0x80000000; // 2GB limit for user space
        if search_addr + aligned_size <= MAX_USER_ADDR {
            Some(search_addr)
        } else {
            None
        }
    }
    
    /// Get memory statistics and usage information
    /// This provides detailed information about memory usage patterns
    /// 
    /// # Returns
    /// A tuple containing (total_maps, total_virtual_size, fragmentation_info)
    pub fn get_memory_stats(&self) -> (usize, usize, usize) {
        let total_maps = self.memmap.len();
        let total_virtual_size: usize = self.memmap.values()
            .map(|memory_map| memory_map.vmarea.end - memory_map.vmarea.start + 1)
            .sum();
        
        // Calculate fragmentation by finding gaps between memory maps
        let mut gaps = 0;
        let mut prev_end = None;
        
        for memory_map in self.memmap.values() {
            if let Some(prev) = prev_end {
                if memory_map.vmarea.start > prev + 1 {
                    gaps += 1;
                }
            }
            prev_end = Some(memory_map.vmarea.end);
        }
        
        (total_maps, total_virtual_size, gaps)
    }
    
    /// Perform memory map coalescing optimization
    /// This attempts to merge adjacent memory maps with compatible properties
    /// 
    /// # Returns
    /// Number of memory maps that were successfully coalesced
    pub fn coalesce_memory_maps(&mut self) -> usize {
        let mut coalesced_count = 0;
        let mut to_remove = Vec::new();
        let mut to_add = Vec::new();
        
        let mut prev_start: Option<usize> = None;
        let mut prev_map: Option<VirtualMemoryMap> = None;
        
        // Collect memory maps that can be merged
        for (&start, memory_map) in &self.memmap {
            if let (Some(prev_s), Some(prev_memory_map)) = (prev_start, &prev_map) {
                // Check if memory maps are adjacent and can be merged
                if prev_memory_map.vmarea.end + 1 == memory_map.vmarea.start &&
                   Self::can_merge_memory_maps(prev_memory_map, memory_map) {
                    
                    // Create merged memory map
                    let merged_map = VirtualMemoryMap {
                        vmarea: super::vmem::MemoryArea {
                            start: prev_memory_map.vmarea.start,
                            end: memory_map.vmarea.end,
                        },
                        pmarea: super::vmem::MemoryArea {
                            start: prev_memory_map.pmarea.start,
                            end: prev_memory_map.pmarea.start + (memory_map.vmarea.end - prev_memory_map.vmarea.start),
                        },
                        permissions: prev_memory_map.permissions, // Use permissions from first map
                        is_shared: prev_memory_map.is_shared,
                    };
                    
                    // Mark old memory maps for removal and add merged map
                    to_remove.push(prev_s);
                    to_remove.push(start);
                    to_add.push(merged_map);
                    coalesced_count += 1;
                    
                    // Skip setting prev for next iteration since we merged
                    prev_start = None;
                    prev_map = None;
                    continue;
                }
            }
            
            prev_start = Some(start);
            prev_map = Some(memory_map.clone());
        }
        
        // Apply changes
        for start in to_remove {
            self.memmap.remove(&start);
        }
        for memory_map in to_add {
            self.memmap.insert(memory_map.vmarea.start, memory_map);
        }
        
        // Clear cache after coalescing
        if coalesced_count > 0 {
            self.last_search_cache = None;
        }
        
        coalesced_count
    }
    
    /// Check if two memory maps can be merged
    /// 
    /// # Arguments
    /// * `map1` - First memory map
    /// * `map2` - Second memory map
    /// 
    /// # Returns
    /// true if memory maps can be safely merged
    fn can_merge_memory_maps(map1: &VirtualMemoryMap, map2: &VirtualMemoryMap) -> bool {
        // Memory maps can be merged if:
        // 1. They have the same permissions
        // 2. They have the same sharing status
        // 3. Physical addresses are also contiguous
        map1.permissions == map2.permissions &&
        map1.is_shared == map2.is_shared &&
        map1.pmarea.end + 1 == map2.pmarea.start
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

    #[test_case]
    fn test_memory_optimization_features() {
        use crate::environment::PAGE_SIZE;
        
        // Test memory optimization features
        let mut manager = VirtualMemoryManager::new();
        
        // Test mmap_base functionality
        assert_eq!(manager.get_mmap_base(), 0x40000000);
        manager.set_mmap_base(0x50000000);
        assert_eq!(manager.get_mmap_base(), 0x50000000);
        
        // Test find_unmapped_area
        let alignment = PAGE_SIZE;
        let size = PAGE_SIZE;
        
        // Should find space at mmap_base when empty
        let addr = manager.find_unmapped_area(size, alignment);
        assert!(addr.is_some());
        assert_eq!(addr.unwrap(), 0x50000000);
        
        // Add some memory maps to test collision avoidance
        let map1 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x80000000, end: 0x80000fff },
            crate::vm::vmem::MemoryArea { start: 0x50000000, end: 0x50000fff },
            0o644,
            false
        );
        manager.add_memory_map(map1).unwrap();
        
        // Should find space after the first mapping
        let addr2 = manager.find_unmapped_area(size, alignment);
        assert!(addr2.is_some());
        assert!(addr2.unwrap() > 0x50000fff);
        
        // Test memory statistics
        let (total_vmas, total_size, gaps) = manager.get_memory_stats();
        assert_eq!(total_vmas, 1);
        assert_eq!(total_size, PAGE_SIZE);
        assert_eq!(gaps, 0); // No gaps with single VMA
        
        // Add another non-adjacent VMA to create a gap
        let map2 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x80002000, end: 0x80002fff },
            crate::vm::vmem::MemoryArea { start: 0x50002000, end: 0x50002fff },
            0o644,
            false
        );
        manager.add_memory_map(map2).unwrap();
        
        let (total_maps, total_size, gaps) = manager.get_memory_stats();
        assert_eq!(total_maps, 2);
        assert_eq!(total_size, PAGE_SIZE * 2);
        assert_eq!(gaps, 1); // One gap between memory maps
        
        // Test memory map coalescing (should fail due to non-adjacent physical addresses)
        let coalesced = manager.coalesce_memory_maps();
        assert_eq!(coalesced, 0); // No coalescing possible due to gap
    }
    
    #[test_case]
    fn test_memory_map_coalescing() {
        use crate::environment::PAGE_SIZE;
        
        // Test memory map coalescing with adjacent compatible maps
        let mut manager = VirtualMemoryManager::new();
        
        // Add two adjacent memory maps that can be merged
        let map1 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x80000000, end: 0x80000fff },
            crate::vm::vmem::MemoryArea { start: 0x10000000, end: 0x10000fff },
            0o644,
            false
        );
        let map2 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x80001000, end: 0x80001fff },
            crate::vm::vmem::MemoryArea { start: 0x10001000, end: 0x10001fff },
            0o644, // Same permissions
            false  // Same sharing status
        );
        
        manager.add_memory_map(map1).unwrap();
        manager.add_memory_map(map2).unwrap();
        
        // Before coalescing
        let (total_maps_before, _, _) = manager.get_memory_stats();
        assert_eq!(total_maps_before, 2);
        
        // Perform coalescing
        let coalesced = manager.coalesce_memory_maps();
        assert_eq!(coalesced, 1); // Should merge one pair
        
        // After coalescing
        let (total_maps_after, total_size, gaps) = manager.get_memory_stats();
        assert_eq!(total_maps_after, 1); // Should be merged into single map
        assert_eq!(total_size, PAGE_SIZE * 2); // Total size should remain same
        assert_eq!(gaps, 0); // No gaps after merging
        
        // Verify the merged map covers the entire range
        let merged_map = manager.search_memory_map(0x10000000);
        assert!(merged_map.is_some());
        assert_eq!(merged_map.unwrap().vmarea.start, 0x10000000);
        assert_eq!(merged_map.unwrap().vmarea.end, 0x10001fff);
    }
}