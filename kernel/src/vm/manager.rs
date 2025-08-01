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

use super::vmem::{VirtualMemoryMap, MemoryArea};

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

        // Optimal overlap detection using BTreeMap's ordered nature
        // Check only the directly adjacent maps (at most 2 maps) for O(log n) performance
        
        // 1. Check the map that starts immediately before the new map
        if let Some((_, prev_map)) = self.memmap.range(..map.vmarea.start).next_back() {
            // If the previous map extends into our range, there's an overlap
            if prev_map.vmarea.end > map.vmarea.start {
                return Err("Memory mapping overlaps with a preceding map");
            }
        }
        
        // 2. Check the map that starts at or after the new map's start position
        if let Some((_, next_map)) = self.memmap.range(map.vmarea.start..).next() {
            // If the next map starts before our range ends, there's an overlap
            if next_map.vmarea.start < map.vmarea.end {
                return Err("Memory mapping overlaps with a succeeding map");
            }
        }

        // Clear cache as the memory layout has changed
        self.last_search_cache = None;
        
        // Insert the new mapping (MMU mapping will be done lazily on page fault)
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
        
        // Remove the memory map
        let removed_map = self.memmap.remove(&start_addr)?;
        
        // Remove the mapping from MMU (page table) to prevent stale TLB entries
        self.unmap_range_from_mmu(removed_map.vmarea.start, removed_map.vmarea.end);
        
        Some(removed_map)
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
        // Strategy: Use BTreeMap's range() to find the memory map that could contain vaddr
        // Since the key is vmarea.start, we need to find the map where:
        // vmarea.start <= vaddr <= vmarea.end
        
        // Find the map with the largest start address <= vaddr
        if let Some((_, map)) = self.memmap.range(..=vaddr).next_back() {
            // Check if vaddr is within this map's range
            if vaddr <= map.vmarea.end {
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

    /// Lazy map a virtual address to MMU on demand (called from page fault handler)
    /// 
    /// This method finds the memory mapping for the given virtual address and
    /// maps only the specific page to the MMU on demand.
    /// 
    /// # Arguments
    /// * `vaddr` - The virtual address that caused the page fault
    /// 
    /// # Returns
    /// * `Ok(())` - Successfully mapped the page
    /// * `Err(&'static str)` - Failed to map (no mapping found or MMU error)
    pub fn lazy_map_page(&mut self, vaddr: usize) -> Result<(), &'static str> {
        // Find the memory mapping for this virtual address
        let memory_map = match self.search_memory_map(vaddr) {
            Some(map) => map,
            None => return Err("No memory mapping found for virtual address"),
        };
        
        // Calculate the page-aligned virtual and physical addresses
        let page_vaddr = vaddr & !(PAGE_SIZE - 1);
        let offset_in_mapping = page_vaddr - memory_map.vmarea.start;
        let page_paddr = memory_map.pmarea.start + offset_in_mapping;
        
        // Map this single page to the MMU
        if let Some(root_pagetable) = self.get_root_page_table() {
            root_pagetable.map(self.asid, page_vaddr, page_paddr, memory_map.permissions);
            Ok(())
        } else {
            Err("No root page table available")
        }
    }

    /// Unmap a virtual address range from MMU
    /// 
    /// This method unmaps the specified virtual address range from the MMU.
    /// Used when memory mappings are removed.
    /// 
    /// # Arguments
    /// * `vaddr_start` - Start of virtual address range
    /// * `vaddr_end` - End of virtual address range (inclusive)
    pub fn unmap_range_from_mmu(&mut self, vaddr_start: usize, vaddr_end: usize) {
        if let Some(root_pagetable) = self.get_root_page_table() {
            let num_pages = (vaddr_end - vaddr_start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;
            
            for i in 0..num_pages {
                let page_vaddr = (vaddr_start & !(PAGE_SIZE - 1)) + i * PAGE_SIZE;
                if page_vaddr <= vaddr_end {
                    root_pagetable.unmap(self.get_asid(), page_vaddr);
                }
            }
        }
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

    /// Add a memory map at a fixed address, handling overlapping mappings by splitting them
    /// 
    /// This method is designed for FIXED memory mappings where the caller wants to map
    /// at a specific virtual address, potentially overwriting existing mappings.
    /// Any existing mappings that overlap with the new mapping will be properly split
    /// or removed to make room for the new mapping.
    /// 
    /// # Arguments
    /// * `map` - The memory map to add at a fixed location
    /// 
    /// # Returns
    /// * `Ok(Vec<VirtualMemoryMap>)` - Returns a vector of overwritten (intersected) memory regions that were replaced by the new mapping.
    /// * `Err(&'static str)` - Error message if the operation failed
    ///
    /// # Design
    /// For each existing mapping that overlaps with the new mapping:
    /// - The function calculates the intersection (overwritten region) between the new mapping and each overlapping existing mapping.
    /// - Only the intersection (overwritten part) is returned for each overlap.
    /// - If the new mapping completely contains the existing mapping, the entire existing mapping is returned as the intersection.
    /// - If the new mapping partially overlaps, only the overlapped region is returned.
    /// - Non-overlapping parts of existing mappings are preserved (split and kept).
    ///
    /// The caller is responsible for handling any managed pages associated with the overwritten mappings.
    pub fn add_memory_map_fixed(&mut self, map: VirtualMemoryMap) -> Result<Vec<VirtualMemoryMap>, &'static str>
    {
        // Validate alignment like the regular add_memory_map
        if map.vmarea.start % PAGE_SIZE != 0 || map.pmarea.start % PAGE_SIZE != 0 ||
            map.vmarea.size() % PAGE_SIZE != 0 || map.pmarea.size() % PAGE_SIZE != 0 {
            return Err("Address or size is not aligned to PAGE_SIZE");
        }

        let new_start = map.vmarea.start;
        let new_end = map.vmarea.end;
        let mut overwritten_mappings = Vec::new();
        let mut mappings_to_add = Vec::new();

        // Find all overlapping mappings and process them
        let overlapping_keys: alloc::vec::Vec<usize> = self.memmap
            .range(..)
            .filter_map(|(start_addr, existing_map)| {
                let existing_start = existing_map.vmarea.start;
                let existing_end = existing_map.vmarea.end;
                if new_start <= existing_end && new_end >= existing_start {
                    Some(*start_addr)
                } else {
                    None
                }
            })
            .collect();

        // Process each overlapping mapping
        for key in overlapping_keys {
            if let Some(existing_map) = self.memmap.remove(&key) {
                let existing_start = existing_map.vmarea.start;
                let existing_end = existing_map.vmarea.end;

                // Calculate the overwritten (intersection) part
                let overlap_start = core::cmp::max(new_start, existing_start);
                let overlap_end = core::cmp::min(new_end, existing_end);
                if overlap_start <= overlap_end {
                    // Cut out the pmarea at the same offset as the intersection
                    let pm_offset = overlap_start - existing_start;
                    let overwritten_map = VirtualMemoryMap {
                        vmarea: MemoryArea {
                            start: overlap_start,
                            end: overlap_end,
                        },
                        pmarea: MemoryArea {
                            start: existing_map.pmarea.start + pm_offset,
                            end: existing_map.pmarea.start + pm_offset + (overlap_end - overlap_start),
                        },
                        permissions: existing_map.permissions,
                        is_shared: existing_map.is_shared,
                        owner: existing_map.owner.clone(),
                    };
                    overwritten_mappings.push(overwritten_map);
                }

                // Case 1: New mapping completely contains the existing mapping
                if new_start <= existing_start && new_end >= existing_end {
                    // Remove entire existing mapping
                    continue;
                }

                // Case 2: Partial overlap - need to split
                // Keep the part before the new mapping (if any)
                if existing_start < new_start {
                    let before_map = VirtualMemoryMap {
                        vmarea: MemoryArea {
                            start: existing_start,
                            end: new_start - 1,
                        },
                        pmarea: MemoryArea {
                            start: existing_map.pmarea.start,
                            end: existing_map.pmarea.start + (new_start - existing_start) - 1,
                        },
                        permissions: existing_map.permissions,
                        is_shared: existing_map.is_shared,
                        owner: existing_map.owner.clone(),
                    };
                    mappings_to_add.push(before_map);
                }

                // Keep the part after the new mapping (if any)
                if existing_end > new_end {
                    let after_offset = (new_end + 1) - existing_start;
                    let after_map = VirtualMemoryMap {
                        vmarea: MemoryArea {
                            start: new_end + 1,
                            end: existing_end,
                        },
                        pmarea: MemoryArea {
                            start: existing_map.pmarea.start + after_offset,
                            end: existing_map.pmarea.end,
                        },
                        permissions: existing_map.permissions,
                        is_shared: existing_map.is_shared,
                        owner: existing_map.owner.clone(),
                    };
                    mappings_to_add.push(after_map);
                }
            }
        }

        // Clear cache since we've modified the memory layout
        self.last_search_cache = None;

        // Remove overlapping mappings from MMU (page table) to prevent stale TLB entries
        for overwritten_map in &overwritten_mappings {
            // crate::println!("Unmapping overwritten mapping: {:x?}", overwritten_map);
            self.unmap_range_from_mmu(overwritten_map.vmarea.start, overwritten_map.vmarea.end);
        }

        // Add the split mappings back (MMU mapping will be done lazily on page fault)
        for split_map in mappings_to_add {
            // crate::println!("Adding split mapping: {:x?}", split_map);
            self.memmap.insert(split_map.vmarea.start, split_map);
        }

        // crate::println!("Adding new mapping: {:x?}", map);
        // Add the new mapping (MMU mapping will be done lazily on page fault)
        self.memmap.insert(map.vmarea.start, map);

        Ok(overwritten_mappings)
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
                            end: memory_map.pmarea.end,
                        },
                        permissions: prev_memory_map.permissions, // Use permissions from first map
                        is_shared: prev_memory_map.is_shared,
                        owner: prev_memory_map.owner.clone(),
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
    use crate::arch::vm::{alloc_virtual_address_space, get_root_pagetable};
    use crate::environment::PAGE_SIZE;
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
        let map = VirtualMemoryMap { vmarea: vma, pmarea: vma, permissions: 0, is_shared: false, owner: None };
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
        let map = VirtualMemoryMap { vmarea: vma, pmarea: vma, permissions: 0, is_shared: false, owner: None };
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
        let map1 = VirtualMemoryMap { vmarea: vma1, pmarea: vma1, permissions: 0, is_shared: false, owner: None };
        let vma2 = MemoryArea { start: 0x3000, end: 0x3fff };
        let map2 = VirtualMemoryMap { vmarea: vma2, pmarea: vma2, permissions: 0, is_shared: false, owner: None };
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
            crate::vm::vmem::MemoryArea { start: 0x80000000, end: 0x80000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x50000000, end: 0x50000fff }, // vmarea
            0o644,
            false,
            None
        );
        manager.add_memory_map(map1).unwrap();
        
        // Should find space after the first mapping
        let addr2 = manager.find_unmapped_area(size, alignment);
        assert!(addr2.is_some());
        assert!(addr2.unwrap() > 0x50000fff);
        
        // Test memory statistics
        let (total_maps, total_size, gaps) = manager.get_memory_stats();
        assert_eq!(total_maps, 1);
        assert_eq!(total_size, PAGE_SIZE);
        assert_eq!(gaps, 0); // No gaps with single map
        
        // Add another non-adjacent map to create a gap
        let map2 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x80002000, end: 0x80002fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x50002000, end: 0x50002fff }, // vmarea
            0o644,
            false,
            None
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
            false,
            None
        );
        let map2 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x80001000, end: 0x80001fff },
            crate::vm::vmem::MemoryArea { start: 0x10001000, end: 0x10001fff },
            0o644, // Same permissions
            false,  // Same sharing status
            None
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

    #[test_case]
    fn test_complex_overlap_detection() {
        let mut manager = VirtualMemoryManager::new();
        
        // Set up existing memory maps for comprehensive overlap testing
        // Map 1: [0x1000, 0x2000)
        let map1 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x10000000, end: 0x10000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x1000, end: 0x1fff },        // vmarea
            0o644,
            false,
            None
        );
        manager.add_memory_map(map1).unwrap();
        
        // Map 2: [0x4000, 0x5000)
        let map2 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x20000000, end: 0x20000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x4000, end: 0x4fff },        // vmarea
            0o644,
            false,
            None
        );
        manager.add_memory_map(map2).unwrap();
        
        // Map 3: [0x7000, 0x8000)
        let map3 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x30000000, end: 0x30000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x7000, end: 0x7fff },        // vmarea
            0o644,
            false,
            None
        );
        manager.add_memory_map(map3).unwrap();
        
        // Test Case 1: Overlap with previous map (end boundary)
        // Try to add [0x1800, 0x2800) - overlaps with map1's end
        let overlap_with_prev = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x40000000, end: 0x40000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x1800, end: 0x27ff },        // vmarea
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(overlap_with_prev).is_err());
        
        // Test Case 2: Overlap with next map (start boundary)
        // Try to add [0x3800, 0x4800) - overlaps with map2's start
        let overlap_with_next = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x50000000, end: 0x50000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x3800, end: 0x47ff },        // vmarea
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(overlap_with_next).is_err());
        
        // Test Case 3: Complete containment by existing map
        // Try to add [0x1200, 0x1800) - completely inside map1
        let contained_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x60000000, end: 0x600005ff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x1200, end: 0x17ff },        // vmarea
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(contained_map).is_err());
        
        // Test Case 4: Containing an existing map
        // Try to add [0x800, 0x2800) - contains map1 completely
        let containing_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x70000000, end: 0x70001fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x800, end: 0x27ff },         // vmarea
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(containing_map).is_err());
        
        // Test Case 5: Exact boundary collision (touching exactly)
        // Try to add [0x2000, 0x3000) - starts exactly where map1 ends
        let exact_boundary = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x80000000, end: 0x80000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x2000, end: 0x2fff },        // vmarea
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(exact_boundary).is_ok()); // Should succeed (touching but not overlapping)
        
        // Test Case 6: Valid gap insertion
        // Add [0x5000, 0x6000) - fits perfectly between map2 and map3
        let gap_insertion = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x90000000, end: 0x90000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x5000, end: 0x5fff },        // vmarea
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(gap_insertion).is_ok());
        
        // Test Case 7: Edge case - inserting at the very beginning
        // Add [0x0, 0x1000) - before all existing maps
        let beginning_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0xa0000000, end: 0xa0000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x0, end: 0xfff },            // vmarea
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(beginning_map).is_ok());
        
        // Test Case 8: Edge case - inserting at the very end
        // Add [0x8000, 0x9000) - after all existing maps
        let end_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0xb0000000, end: 0xb0000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x8000, end: 0x8fff },        // vmarea
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(end_map).is_ok());
        
        // Verify final state: should have 7 maps total
        assert_eq!(manager.memmap_len(), 7);
        
        // Verify all maps are accessible and correctly ordered
        let starts: [usize; 7] = [0x0, 0x1000, 0x2000, 0x4000, 0x5000, 0x7000, 0x8000];
        let mut i = 0;
        for map in manager.memmap_iter() {
            assert_eq!(map.vmarea.start, starts[i]);
            i += 1;
        }
        assert_eq!(i, 7);
    }
    
    #[test_case]
    fn test_alignment_and_edge_cases() {
        let mut manager = VirtualMemoryManager::new();
        
        // Test Case 1: Non-aligned virtual address (should fail)
        let misaligned_virtual = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x10000000, end: 0x10000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x1001, end: 0x2000 },        // vmarea - Not PAGE_SIZE aligned
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(misaligned_virtual).is_err());
        
        // Test Case 2: Non-aligned physical address (should fail)
        let misaligned_physical = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x10000001, end: 0x10001000 }, // pmarea - Not PAGE_SIZE aligned
            crate::vm::vmem::MemoryArea { start: 0x1000, end: 0x1fff },        // vmarea
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(misaligned_physical).is_err());
        
        // Test Case 3: Non-aligned size (should fail)
        let misaligned_size = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x10000000, end: 0x10000800 }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x1000, end: 0x1800 },        // vmarea - Size is not PAGE_SIZE multiple
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(misaligned_size).is_err());
        
        // Test Case 4: Zero-size mapping (should fail)
        let zero_size = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x10000000, end: 0x10000000 }, // pmarea - Start == End
            crate::vm::vmem::MemoryArea { start: 0x1000, end: 0x1000 },        // vmarea - Start == End
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(zero_size).is_err());
        
        // Test Case 5: Single page mapping (should succeed)
        let single_page = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x10000000, end: 0x10000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x1000, end: 0x1fff },        // vmarea
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(single_page).is_ok());
        
        // Test Case 6: Large mapping (multiple pages)
        let large_mapping = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x20000000, end: 0x2000ffff }, // pmarea - 64KB
            crate::vm::vmem::MemoryArea { start: 0x10000, end: 0x1ffff },      // vmarea - 64KB
            0o644,
            false,
            None
        );
        assert!(manager.add_memory_map(large_mapping).is_ok());
        
        assert_eq!(manager.memmap_len(), 2);
    }
    
    #[test_case]
    fn test_cache_invalidation_on_add() {
        let mut manager = VirtualMemoryManager::new();
        
        // Add initial mapping
        let map1 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x10000000, end: 0x10000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x1000, end: 0x1fff },        // vmarea
            0o644,
            false,
            None
        );
        manager.add_memory_map(map1).unwrap();
        
        // Search to populate cache
        let found = manager.search_memory_map(0x1500);
        assert!(found.is_some());
        
        // Verify cache is populated (indirect test through repeated search performance)
        let found_again = manager.search_memory_map(0x1500);
        assert!(found_again.is_some());
        
        // Add another mapping, which should invalidate cache
        let map2 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x20000000, end: 0x20000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x3000, end: 0x3fff },        // vmarea
            0o644,
            false,
            None
        );
        manager.add_memory_map(map2).unwrap();
        
        // Search should still work correctly after cache invalidation
        let found_after_invalidation = manager.search_memory_map(0x1500);
        assert!(found_after_invalidation.is_some());
        assert_eq!(found_after_invalidation.unwrap().vmarea.start, 0x1000);
        
        let found_new = manager.search_memory_map(0x3500);
        assert!(found_new.is_some());
        assert_eq!(found_new.unwrap().vmarea.start, 0x3000);
    }

    #[test_case]
    fn test_add_memory_map_fixed_complete_overlap() {        
        let mut manager = VirtualMemoryManager::new();
        
        // Add initial mapping at [0x2000, 0x3000)
        let initial_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x10000000, end: 0x10000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x2000, end: 0x2fff },        // vmarea
            0o644,
            false,
            None
        );
        manager.add_memory_map(initial_map).unwrap();
        assert_eq!(manager.memmap_len(), 1);
        
        // Add fixed mapping that completely contains the existing mapping [0x1000, 0x4000)
        let fixed_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x20000000, end: 0x20002fff }, // pmarea - 3 pages
            crate::vm::vmem::MemoryArea { start: 0x1000, end: 0x3fff },        // vmarea - 3 pages
            0o755,
            true,
            None
        );
        
        let result = manager.add_memory_map_fixed(fixed_map);
        assert!(result.is_ok());
        
        let overwritten_mappings = result.unwrap();
        assert_eq!(overwritten_mappings.len(), 1); // Should have removed one mapping
        assert_eq!(overwritten_mappings[0].vmarea.start, 0x2000);
        
        // Should now have only the new fixed mapping
        assert_eq!(manager.memmap_len(), 1);
        let remaining_map = manager.search_memory_map(0x2000);
        assert!(remaining_map.is_some());
        assert_eq!(remaining_map.unwrap().vmarea.start, 0x1000);
        assert_eq!(remaining_map.unwrap().vmarea.end, 0x3fff);
        assert_eq!(remaining_map.unwrap().permissions, 0o755);
        assert_eq!(remaining_map.unwrap().is_shared, true);
    }

    #[test_case]
    fn test_add_memory_map_fixed_partial_overlap() {        
        let mut manager = VirtualMemoryManager::new();
        
        // Add initial mapping at [0x1000, 0x3000) - 2 pages
        let initial_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x10000000, end: 0x10001fff }, // pmarea - 2 pages
            crate::vm::vmem::MemoryArea { start: 0x1000, end: 0x2fff },        // vmarea - 2 pages  
            0o644,
            false,
            None
        );
        manager.add_memory_map(initial_map).unwrap();
        assert_eq!(manager.memmap_len(), 1);
        
        // Add fixed mapping that overlaps from middle: [0x2000, 0x4000) - 2 pages
        let fixed_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x20000000, end: 0x20001fff }, // pmarea - 2 pages
            crate::vm::vmem::MemoryArea { start: 0x2000, end: 0x3fff },        // vmarea - 2 pages
            0o755,
            true,
            None
        );
        
        let result = manager.add_memory_map_fixed(fixed_map);
        assert!(result.is_ok());
        
        let overwritten_mappings = result.unwrap();
        assert_eq!(overwritten_mappings.len(), 1); // Should have removed the original mapping
        
        // Should now have 2 mappings: the split part [0x1000, 0x2000) and the new fixed [0x2000, 0x4000)
        assert_eq!(manager.memmap_len(), 2);
        
        // Check the remaining part of the original mapping
        let remaining_original = manager.search_memory_map(0x1500);
        assert!(remaining_original.is_some());
        assert_eq!(remaining_original.unwrap().vmarea.start, 0x1000);
        assert_eq!(remaining_original.unwrap().vmarea.end, 0x1fff);
        assert_eq!(remaining_original.unwrap().permissions, 0o644);
        
        // Check the new fixed mapping
        let new_fixed = manager.search_memory_map(0x3000);
        assert!(new_fixed.is_some());
        assert_eq!(new_fixed.unwrap().vmarea.start, 0x2000);
        assert_eq!(new_fixed.unwrap().vmarea.end, 0x3fff);
        assert_eq!(new_fixed.unwrap().permissions, 0o755);
        assert_eq!(new_fixed.unwrap().is_shared, true);
    }

    #[test_case]
    fn test_add_memory_map_fixed_split_both_ends() {
        let mut manager = VirtualMemoryManager::new();
        
        // Add initial mapping at [0x1000, 0x5000) - 4 pages
        let initial_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x10000000, end: 0x10003fff }, // pmarea - 4 pages
            crate::vm::vmem::MemoryArea { start: 0x1000, end: 0x4fff },        // vmarea - 4 pages
            0o644,
            false,
            None
        );
        manager.add_memory_map(initial_map).unwrap();
        assert_eq!(manager.memmap_len(), 1);
        
        // Add fixed mapping in the middle: [0x2000, 0x4000) - 2 pages
        let fixed_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x20000000, end: 0x20001fff }, // pmarea - 2 pages
            crate::vm::vmem::MemoryArea { start: 0x2000, end: 0x3fff },        // vmarea - 2 pages
            0o755,
            true,
            None
        );
        
        let result = manager.add_memory_map_fixed(fixed_map);
        assert!(result.is_ok());
        
        let overwritten_mappings = result.unwrap();
        assert_eq!(overwritten_mappings.len(), 1); // Should have removed the original mapping
        
        // Should now have 3 mappings: before [0x1000, 0x2000), fixed [0x2000, 0x4000), after [0x4000, 0x5000)
        assert_eq!(manager.memmap_len(), 3);
        
        // Check the part before the fixed mapping
        let before_part = manager.search_memory_map(0x1500);
        assert!(before_part.is_some());
        assert_eq!(before_part.unwrap().vmarea.start, 0x1000);
        assert_eq!(before_part.unwrap().vmarea.end, 0x1fff);
        assert_eq!(before_part.unwrap().permissions, 0o644);
        
        // Check the new fixed mapping
        let fixed_part = manager.search_memory_map(0x3000);
        assert!(fixed_part.is_some());
        assert_eq!(fixed_part.unwrap().vmarea.start, 0x2000);
        assert_eq!(fixed_part.unwrap().vmarea.end, 0x3fff);
        assert_eq!(fixed_part.unwrap().permissions, 0o755);
        assert_eq!(fixed_part.unwrap().is_shared, true);
        
        // Check the part after the fixed mapping
        let after_part = manager.search_memory_map(0x4500);
        assert!(after_part.is_some());
        assert_eq!(after_part.unwrap().vmarea.start, 0x4000);
        assert_eq!(after_part.unwrap().vmarea.end, 0x4fff);
        assert_eq!(after_part.unwrap().permissions, 0o644);
    }

    #[test_case]
    fn test_add_memory_map_fixed_no_overlap() {
        let mut manager = VirtualMemoryManager::new();
        
        // Add initial mapping at [0x1000, 0x2000)
        let initial_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x10000000, end: 0x10000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x1000, end: 0x1fff },        // vmarea
            0o644,
            false,
            None
        );
        manager.add_memory_map(initial_map).unwrap();
        
        // Add fixed mapping with no overlap at [0x3000, 0x4000)
        let fixed_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea { start: 0x20000000, end: 0x20000fff }, // pmarea
            crate::vm::vmem::MemoryArea { start: 0x3000, end: 0x3fff },        // vmarea
            0o755,
            true,
            None
        );
        
        let result = manager.add_memory_map_fixed(fixed_map);
        assert!(result.is_ok());
        
        let overwritten_mappings = result.unwrap();
        assert_eq!(overwritten_mappings.len(), 0); // No mappings should be removed
        
        // Should now have 2 mappings
        assert_eq!(manager.memmap_len(), 2);
        
        // Both mappings should be intact
        let first_map = manager.search_memory_map(0x1500);
        assert!(first_map.is_some());
        assert_eq!(first_map.unwrap().vmarea.start, 0x1000);
        
        let second_map = manager.search_memory_map(0x3500);
        assert!(second_map.is_some());
        assert_eq!(second_map.unwrap().vmarea.start, 0x3000);
    }

    #[test_case]
    fn test_lazy_mapping_and_unmapping() {
        let mut manager = VirtualMemoryManager::new();
        let vma = MemoryArea { start: 0x1000, end: 0x1fff };
        let map = VirtualMemoryMap { vmarea: vma, pmarea: vma, permissions: 0o644, is_shared: false, owner: None };
        let asid = alloc_virtual_address_space();
        manager.set_asid(asid);
        manager.add_memory_map(map).unwrap();
        
        // Trigger lazy mapping by simulating a page fault at virtual address 0x1500
        assert!(manager.lazy_map_page(0x1500).is_ok());
        
        // The page should now be mapped in the MMU
        // For testing, we can't directly check MMU state, so we verify by translating the address
        let translated_addr = manager.translate_vaddr(0x1500);
        assert!(translated_addr.is_some());
        assert_eq!(translated_addr.unwrap() & !(PAGE_SIZE - 1), 0x1000); // Should be page-aligned
        
        // Test unmapping functionality by removing the memory map
        // This also unmaps from MMU due to our implementation
        manager.remove_memory_map_by_addr(0x1500);
        
        // Translation should now fail as the memory map is removed
        let translated_addr_after_unmap = manager.translate_vaddr(0x1500);
        assert!(translated_addr_after_unmap.is_none());
    }
}