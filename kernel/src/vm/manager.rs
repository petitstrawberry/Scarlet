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
use alloc::collections::btree_map::Values;
use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use spin::RwLock;

use crate::mem::page::ContiguousPages;
use crate::object::capability::memory_mapping::AccessOp;
use crate::{
    arch::vm::{free_virtual_address_space, get_root_pagetable, is_asid_used, mmu::PageTable},
    environment::PAGE_SIZE,
};

use super::addr::phys_to_virt;
use super::vmem::{MemoryArea, VirtualMemoryMap, VirtualMemoryPermission};

#[derive(Debug, Clone)]
pub struct VirtualMemoryManager {
    inner: Arc<RwLock<InnerVmm>>, // shared, internally synchronized
}

#[derive(Debug)]
struct InnerVmm {
    memmap: BTreeMap<usize, VirtualMemoryMap>,
    asid: u16,
    mmap_base: usize,
    page_tables: Vec<Arc<PageTable>>,
    last_search_cache: Option<(usize, usize, usize)>,
    owner_task_id: Option<usize>,
    private_page_allocations: Vec<ContiguousPages>,
}

impl VirtualMemoryManager {
    fn subrange_pmarea(
        existing_map: &VirtualMemoryMap,
        sub_start: usize,
        sub_end: usize,
    ) -> MemoryArea {
        if existing_map.pmarea.start == 0 && existing_map.pmarea.end == 0 {
            return MemoryArea { start: 0, end: 0 };
        }

        let pm_offset = sub_start - existing_map.vmarea.start;
        MemoryArea {
            start: existing_map.pmarea.start + pm_offset,
            end: existing_map.pmarea.start + pm_offset + (sub_end - sub_start),
        }
    }

    /// Creates a new virtual memory manager.
    ///
    /// # Returns
    /// A new virtual memory manager with default values.
    pub fn new() -> Self {
        let inner = InnerVmm {
            memmap: BTreeMap::new(),
            asid: 0,
            mmap_base: 0x40000000, // 1 GB base address for mmap (Default)
            page_tables: Vec::new(),
            last_search_cache: None,
            owner_task_id: None,
            private_page_allocations: Vec::new(),
        };
        VirtualMemoryManager {
            inner: Arc::new(RwLock::new(inner)),
        }
    }

    /// Set the owner task ID if this manager does not already have one.
    ///
    /// # Arguments
    /// * `task_id` - Task ID owning this virtual address space.
    pub fn set_owner_task_id_if_unset(&self, task_id: usize) {
        let mut g = self.inner.write();
        if g.owner_task_id.is_none() {
            g.owner_task_id = Some(task_id);
        }
    }

    fn track_private_page_allocation(&self, alloc: ContiguousPages) {
        let owner_task_id = self.inner.read().owner_task_id;
        if let Some(owner_task_id) = owner_task_id {
            if let Some(owner_task) = crate::sched::scheduler::get_task_by_id(owner_task_id) {
                owner_task.page_allocations.write().push(alloc);
                return;
            }
        }

        self.inner.write().private_page_allocations.push(alloc);
    }

    fn sync_executable_page_for_mapping(permissions: usize, paddr: usize) {
        if VirtualMemoryPermission::Execute.contained_in(permissions) {
            crate::arch::sync_icache_for_execution(phys_to_virt(paddr), PAGE_SIZE);
        }
    }

    /// Sets the ASID (Address Space ID) for the virtual memory manager.
    ///
    /// # Arguments
    /// * `asid` - The ASID to set
    pub fn set_asid(&self, asid: u16) {
        let mut g = self.inner.write();
        if g.asid == asid {
            return;
        }
        if g.asid != 0 && is_asid_used(g.asid) {
            free_virtual_address_space(g.asid);
        }
        g.asid = asid;
    }

    /// Returns the ASID (Address Space ID) for the virtual memory manager.
    ///
    /// # Returns
    /// The ASID for the virtual memory manager.
    pub fn get_asid(&self) -> u16 {
        self.inner.read().asid
    }

    /// Returns a mutable iterator over all memory maps.
    ///
    /// # Returns
    /// A mutable iterator over references to all memory maps.
    // Mutable iterator is removed in favor of snapshot-based API.

    /// Returns the number of memory maps.
    ///
    /// # Returns
    /// The number of memory maps.
    pub fn memmap_len(&self) -> usize {
        self.inner.read().memmap.len()
    }

    /// Returns true if there are no memory maps.
    ///
    /// # Returns
    /// True if there are no memory maps.
    pub fn memmap_is_empty(&self) -> bool {
        self.inner.read().memmap.is_empty()
    }

    /// Execute a read-only operation while holding a read lock on memmaps.
    /// This avoids cloning and provides high-performance access.
    pub fn with_memmaps<R>(&self, f: impl FnOnce(&BTreeMap<usize, VirtualMemoryMap>) -> R) -> R {
        let g = self.inner.read();
        f(&g.memmap)
    }

    /// Execute a mutable operation while holding a write lock on memmaps.
    /// Prefer using provided APIs; expose for advanced use-cases.
    pub fn with_memmaps_mut<R>(
        &self,
        f: impl FnOnce(&mut BTreeMap<usize, VirtualMemoryMap>) -> R,
    ) -> R {
        let mut g = self.inner.write();
        f(&mut g.memmap)
    }

    /// Execute a read-only iteration over memory maps while holding the lock.
    /// This returns an iterator of `&VirtualMemoryMap` valid only inside the closure.
    pub fn memmaps_iter_with<R, F>(&self, f: F) -> R
    where
        F: for<'a> FnOnce(Values<'a, usize, VirtualMemoryMap>) -> R,
    {
        let g = self.inner.read();
        let iter = g.memmap.values();
        f(iter)
    }

    /// Gets a memory map by its start address.
    ///
    /// # Arguments
    /// * `start_addr` - The start address of the memory map
    ///
    /// # Returns
    /// The memory map with the given start address, if it exists.
    pub fn get_memory_map_by_addr(&self, start_addr: usize) -> Option<VirtualMemoryMap> {
        self.inner.read().memmap.get(&start_addr).cloned()
    }

    /// Gets a mutable memory map by its start address.
    ///
    /// # Arguments
    /// * `start_addr` - The start address of the memory map
    ///
    /// # Returns
    /// The mutable memory map with the given start address, if it exists.
    // Removed: use snapshot + fixed update methods instead

    /// Adds a memory map to the virtual memory manager with overlap checking.
    ///
    /// This method performs overlap detection before adding the mapping.
    /// Use this for:
    /// - User-initiated memory allocation (mmap, malloc, etc.)
    /// - Dynamic memory allocation where overlap is possible
    /// - Any case where memory range conflicts are uncertain
    ///
    /// This method uses efficient overlap detection with ordered data structures.
    ///
    /// # Arguments
    /// * `map` - The memory map to add
    ///
    /// # Returns
    /// A result indicating success or failure.
    ///
    pub fn add_memory_map(&self, map: VirtualMemoryMap) -> Result<(), &'static str> {
        // Check if the address and size is aligned
        if map.vmarea.start % PAGE_SIZE != 0 || map.vmarea.size() % PAGE_SIZE != 0 {
            return Err("Address or size is not aligned to PAGE_SIZE");
        }
        if map.pmarea.start != 0
            && (map.pmarea.start % PAGE_SIZE != 0 || map.pmarea.size() % PAGE_SIZE != 0)
        {
            return Err("pmarea is not aligned to PAGE_SIZE");
        }

        let mut g = self.inner.write();
        // 1. prev adjacency check
        if let Some((_, prev_map)) = g.memmap.range(..map.vmarea.start).next_back() {
            if prev_map.vmarea.end > map.vmarea.start {
                return Err("Memory mapping overlaps with a preceding map");
            }
        }
        // 2. next adjacency check
        if let Some((_, next_map)) = g.memmap.range(map.vmarea.start..).next() {
            if next_map.vmarea.start < map.vmarea.end {
                return Err("Memory mapping overlaps with a succeeding map");
            }
        }

        g.last_search_cache = None;
        g.memmap.insert(map.vmarea.start, map);
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
    pub fn remove_memory_map_by_addr(&self, vaddr: usize) -> Option<VirtualMemoryMap> {
        let mut g = self.inner.write();
        let start_addr = find_memory_map_key_with_cache_update(&mut *g, vaddr)?;
        if let Some((_, _, cache_key)) = g.last_search_cache {
            if cache_key == start_addr {
                g.last_search_cache = None;
            }
        }
        let removed_map = g.memmap.remove(&start_addr);
        drop(g);
        if let Some(m) = removed_map {
            self.unmap_range_from_mmu(m.vmarea.start, m.vmarea.end);
            Some(m)
        } else {
            None
        }
    }

    /// Removes a range of memory maps by virtual address range.
    /// Splits existing mappings if they partially overlap with the range.
    ///
    /// # Arguments
    /// * `vaddr` - The starting virtual address of the range to remove
    /// * `len` - The length of the range to remove
    ///
    /// # Returns
    /// A vector of removed memory maps (only the parts that were fully within the range)
    pub fn remove_memory_map_range(&self, vaddr: usize, len: usize) -> Vec<VirtualMemoryMap> {
        if len == 0 {
            return Vec::new();
        }

        let remove_start = vaddr & !(PAGE_SIZE - 1);
        let remove_end = match vaddr
            .checked_add(len)
            .and_then(|end| end.checked_add(PAGE_SIZE - 1))
            .map(|end| (end & !(PAGE_SIZE - 1)).saturating_sub(1))
        {
            Some(end) if remove_start <= end => end,
            _ => return Vec::new(),
        };
        let mut removed_maps = Vec::new();
        let mut mappings_to_add = Vec::new();

        let mut g = self.inner.write();

        // Find all mappings that overlap with the removal range
        let overlapping_keys: alloc::vec::Vec<usize> = g
            .memmap
            .range(..)
            .filter_map(|(start_addr, existing_map)| {
                let existing_start = existing_map.vmarea.start;
                let existing_end = existing_map.vmarea.end;
                if remove_start <= existing_end && remove_end >= existing_start {
                    Some(*start_addr)
                } else {
                    None
                }
            })
            .collect();

        for key in overlapping_keys {
            if let Some(existing_map) = g.memmap.remove(&key) {
                let existing_start = existing_map.vmarea.start;
                let existing_end = existing_map.vmarea.end;

                // Calculate the overlap (intersection) part
                let overlap_start = core::cmp::max(remove_start, existing_start);
                let overlap_end = core::cmp::min(remove_end, existing_end);

                if overlap_start <= overlap_end {
                    // Create a map for the removed (overlapping) portion
                    let removed_portion = VirtualMemoryMap {
                        vmarea: MemoryArea {
                            start: overlap_start,
                            end: overlap_end,
                        },
                        pmarea: Self::subrange_pmarea(&existing_map, overlap_start, overlap_end),
                        vm_start: existing_map.vm_start,
                        permissions: existing_map.permissions,
                        is_shared: existing_map.is_shared,
                        memory_attribute: existing_map.memory_attribute,
                        owner: existing_map.owner.clone(),
                    };
                    removed_maps.push(removed_portion);
                }

                // Case 1: Removal range completely contains the existing mapping
                if remove_start <= existing_start && remove_end >= existing_end {
                    // Remove entire existing mapping (already removed above)
                    continue;
                }

                // Case 2: Partial overlap - keep the part before the removal range
                if existing_start < remove_start {
                    let before_map = VirtualMemoryMap {
                        vmarea: MemoryArea {
                            start: existing_start,
                            end: remove_start - 1,
                        },
                        pmarea: Self::subrange_pmarea(
                            &existing_map,
                            existing_start,
                            remove_start - 1,
                        ),
                        vm_start: existing_map.vm_start,
                        permissions: existing_map.permissions,
                        is_shared: existing_map.is_shared,
                        memory_attribute: existing_map.memory_attribute,
                        owner: existing_map.owner.clone(),
                    };
                    mappings_to_add.push(before_map);
                }

                // Case 3: Partial overlap - keep the part after the removal range
                if existing_end > remove_end {
                    let after_map = VirtualMemoryMap {
                        vmarea: MemoryArea {
                            start: remove_end + 1,
                            end: existing_end,
                        },
                        pmarea: Self::subrange_pmarea(&existing_map, remove_end + 1, existing_end),
                        vm_start: existing_map.vm_start,
                        permissions: existing_map.permissions,
                        is_shared: existing_map.is_shared,
                        memory_attribute: existing_map.memory_attribute,
                        owner: existing_map.owner.clone(),
                    };
                    mappings_to_add.push(after_map);
                }
            }
        }

        // Re-add the preserved portions
        for map in mappings_to_add {
            g.memmap.insert(map.vmarea.start, map);
        }

        // Clear cache if it might be affected
        if let Some((_, _, cache_key)) = g.last_search_cache {
            if let Some(cached_map) = g.memmap.get(&cache_key) {
                let cache_end = cached_map.vmarea.end;
                if remove_start <= cache_end && remove_end >= cache_key {
                    g.last_search_cache = None;
                }
            } else {
                g.last_search_cache = None;
            }
        }

        drop(g);

        // Unmap the removed range from MMU
        self.unmap_range_from_mmu(remove_start, remove_end);

        removed_maps
    }

    /// Removes all memory maps.
    ///
    /// # Returns
    /// The removed memory maps.
    ///
    /// # Note
    /// This method returns an iterator instead of a cloned Vec for efficiency.
    pub fn remove_all_memory_maps(&self) -> impl Iterator<Item = VirtualMemoryMap> {
        let mut g = self.inner.write();
        g.last_search_cache = None;
        let memmap = core::mem::take(&mut g.memmap);
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
    pub fn restore_memory_maps<I>(&self, maps: I) -> Result<(), &'static str>
    where
        I: IntoIterator<Item = VirtualMemoryMap>,
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
    pub fn search_memory_map(&self, vaddr: usize) -> Option<VirtualMemoryMap> {
        let mut g = self.inner.write();
        if let Some((cache_start, cache_end, cache_key)) = g.last_search_cache {
            if cache_start <= vaddr && vaddr <= cache_end {
                return g.memmap.get(&cache_key).cloned();
            }
        }
        if let Some((_k, map)) = g.memmap.range(..=vaddr).next_back() {
            if vaddr <= map.vmarea.end {
                let start = map.vmarea.start;
                let end = map.vmarea.end;
                let out = map.clone();
                g.last_search_cache = Some((start, end, start));
                return Some(out);
            }
        }
        None
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
    // Removed: replaced by search_memory_map with caching in write lock

    /// Searches for a memory map containing the given virtual address (mutable version).
    ///
    /// This version allows mutable access and updates the search cache.
    ///
    /// # Arguments
    /// * `vaddr` - The virtual address to search for
    ///
    /// # Returns
    /// Mutable reference to the memory map containing the given virtual address, if it exists.
    // Removed mutable accessor; use fixed operations instead

    /// Helper method that finds memory map and updates cache
    ///
    /// # Arguments
    /// * `vaddr` - Virtual address to search for
    ///
    /// # Returns
    /// The start address key of the found memory map, if any
    // Helper moved out to work with inner lock directly

    /// Adds a page table to the virtual memory manager.
    pub fn add_page_table(&self, page_table: Arc<PageTable>) {
        self.inner.write().page_tables.push(page_table);
    }

    /// Returns the root page table for the current address space.
    ///
    /// # Returns
    /// The root page table for the current address space, if it exists.
    pub fn get_root_page_table(&self) -> Option<&mut PageTable> {
        get_root_pagetable(self.get_asid())
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
    pub fn lazy_map_page(&self, vaddr: usize) -> Result<(), &'static str> {
        // Backward-compat shim: default to Load with unknown size
        let access = crate::object::capability::memory_mapping::AccessKind {
            op: crate::object::capability::memory_mapping::AccessOp::Load,
            vaddr,
            size: None,
        };
        self.lazy_map_page_with(access)
    }

    /// Lazy map with access context (instruction/load/store and optional size)
    pub fn lazy_map_page_with(
        &self,
        access: crate::object::capability::memory_mapping::AccessKind,
    ) -> Result<(), &'static str> {
        let vaddr = access.vaddr;
        let memory_map = match self.search_memory_map(vaddr) {
            Some(map) => map,
            None => {
                return self.try_extend_mapping_for_access(&access);
            }
        };

        let page_vaddr = vaddr & !(PAGE_SIZE - 1);
        let page_idx = (page_vaddr - memory_map.vm_start) / PAGE_SIZE;
        let mut perms = memory_map.permissions;

        let page_paddr = if let Some(owner) = &memory_map.owner {
            let owner_access = if !memory_map.is_shared {
                crate::object::capability::memory_mapping::AccessKind {
                    op: AccessOp::Load,
                    vaddr: access.vaddr,
                    size: access.size,
                }
            } else {
                access
            };
            match owner.resolve_fault(&owner_access, page_idx, memory_map.vm_start) {
                Ok(res) => {
                    perms = owner.fault_page_permissions(&access, perms);
                    if res.is_tail {
                        perms &= !0x1;
                        perms &= !0x2;
                    }

                    // COW: most private owner-based mappings allocate a private
                    // page on any resolved fault. Fork COW owners allow reads to
                    // share a read-only page and request a copy only on stores.
                    if !memory_map.is_shared && owner.private_fault_requires_copy(&access) {
                        if let Some(root_pagetable) = self.get_root_page_table() {
                            let new_alloc = ContiguousPages::new(1)
                                .ok_or("Failed to allocate page for private mapping COW")?;
                            let new_paddr = new_alloc.as_paddr();
                            unsafe {
                                core::ptr::copy_nonoverlapping(
                                    crate::vm::addr::phys_to_virt(res.paddr_page_base) as *const u8,
                                    crate::vm::addr::phys_to_virt(new_paddr) as *mut u8,
                                    PAGE_SIZE,
                                );
                            }
                            let cow_map = VirtualMemoryMap {
                                pmarea: MemoryArea::new(new_paddr, new_paddr + PAGE_SIZE - 1),
                                vmarea: MemoryArea::new(page_vaddr, page_vaddr + PAGE_SIZE - 1),
                                vm_start: memory_map.vm_start,
                                permissions: perms,
                                is_shared: false,
                                memory_attribute: memory_map.memory_attribute,
                                owner: None,
                            };
                            match self.add_memory_map_fixed(cow_map) {
                                Ok(removed) => {
                                    for rm in &removed {
                                        if rm.pmarea.start != 0 && rm.owner.is_none() {
                                            // SAFETY: pmarea points to a page allocated by allocate_raw_pages.
                                            unsafe {
                                                crate::mem::page::free_raw_pages(
                                                    crate::vm::addr::phys_to_virt(rm.pmarea.start)
                                                        as *mut _,
                                                    1,
                                                );
                                            }
                                        }
                                    }
                                }
                                Err(_) => {
                                    return Err("Failed to add COW mapping to VM manager");
                                }
                            }
                            self.track_private_page_allocation(new_alloc);
                            Self::sync_executable_page_for_mapping(perms, new_paddr);
                            root_pagetable.map(
                                self.get_asid(),
                                page_vaddr,
                                new_paddr,
                                perms,
                                true,
                                access.op == AccessOp::Store,
                            );
                            return Ok(());
                        } else {
                            return Err("No root page table available for COW mapping");
                        }
                    }

                    res.paddr_page_base
                }
                Err(_) => {
                    if memory_map.pmarea.start != 0 {
                        memory_map.pmarea.start + (page_vaddr - memory_map.vmarea.start)
                    } else {
                        return Err("Owner failed to resolve fault");
                    }
                }
            }
        } else {
            memory_map.pmarea.start + (page_vaddr - memory_map.vmarea.start)
        };

        if let Some(root_pagetable) = self.get_root_page_table() {
            Self::sync_executable_page_for_mapping(perms, page_paddr);
            root_pagetable.map(
                self.get_asid(),
                page_vaddr,
                page_paddr,
                perms,
                true,
                access.op == AccessOp::Store,
            );
            Ok(())
        } else {
            Err("No root page table available")
        }
    }

    /// Try to extend a mapping for an access that falls outside the current vmarea
    ///
    /// This handles the case where a SharedMemory has been resized via ftruncate
    /// but the VirtualMemoryMap.vmarea.end hasn't been updated.
    fn try_extend_mapping_for_access(
        &self,
        access: &crate::object::capability::memory_mapping::AccessKind,
    ) -> Result<(), &'static str> {
        let vaddr = access.vaddr;
        let page_vaddr = vaddr & !(PAGE_SIZE - 1);

        // Result of successful extend: (paddr_page_base, permissions)
        let extend_result: Option<(usize, usize)>;

        {
            // Lock scope
            let mut g = self.inner.write();

            // Find a mapping whose vmarea.end < vaddr but might have an owner that has grown
            let mut found = None;
            for (_, map) in g.memmap.iter_mut() {
                // Check if vaddr is just past this mapping's end
                if map.vmarea.end < vaddr {
                    // Check if there's an owner that might support extended access
                    if let Some(owner) = &map.owner {
                        // Try resolve_fault to see if owner supports this offset
                        let test_access = crate::object::capability::memory_mapping::AccessKind {
                            vaddr: page_vaddr,
                            op: access.op,
                            size: access.size,
                        };

                        let page_idx = (page_vaddr - map.vm_start) / PAGE_SIZE;
                        match owner.resolve_fault(&test_access, page_idx, map.vm_start) {
                            Ok(res) => {
                                let permissions =
                                    owner.fault_page_permissions(&test_access, map.permissions);
                                // Owner says this offset is valid - extend vmarea.end
                                let new_end = page_vaddr + PAGE_SIZE - 1;
                                crate::println!(
                                    "[VmManager] Extending mapping vmarea.end from {:#x} to {:#x} for owner={}",
                                    map.vmarea.end,
                                    new_end,
                                    owner.mmap_owner_name()
                                );
                                map.vmarea.end = new_end;

                                found = Some((res.paddr_page_base, permissions));
                                break;
                            }
                            Err(_) => {
                                // Owner doesn't support this offset, continue searching
                            }
                        }
                    }
                }
            }
            extend_result = found;
        } // Lock released here

        // Now map the page outside the lock
        if let Some((paddr_page_base, permissions)) = extend_result {
            if let Some(root_pagetable) = self.get_root_page_table() {
                root_pagetable.map(
                    self.get_asid(),
                    page_vaddr,
                    paddr_page_base,
                    permissions,
                    true,
                    access.op == AccessOp::Store,
                );
                return Ok(());
            } else {
                return Err("No root page table available");
            }
        }

        Err("No extendable memory mapping found for virtual address")
    }

    /// Unmap a virtual address range from MMU
    ///
    /// This method unmaps the specified virtual address range from the MMU.
    /// Used when memory mappings are removed.
    ///
    /// # Arguments
    /// * `vaddr_start` - Start of virtual address range
    /// * `vaddr_end` - End of virtual address range (inclusive)
    pub fn unmap_range_from_mmu(&self, vaddr_start: usize, vaddr_end: usize) {
        if let Some(root_pagetable) = self.get_root_page_table() {
            root_pagetable.unmap_range(self.get_asid(), vaddr_start, vaddr_end);
        }
    }

    pub fn translate_to_kva(&self, vaddr: usize) -> Option<usize> {
        self.translate_to_phys(vaddr).map(phys_to_virt)
    }

    pub fn translate_to_kva_for_write(&self, vaddr: usize) -> Option<usize> {
        self.translate_to_phys_with_access(vaddr, AccessOp::Store)
            .map(phys_to_virt)
    }

    pub fn translate_to_phys_with_access(&self, vaddr: usize, op: AccessOp) -> Option<usize> {
        let map = self.search_memory_map(vaddr)?;

        match op {
            AccessOp::Load => {
                if !VirtualMemoryPermission::Read.contained_in(map.permissions) {
                    return None;
                }
            }
            AccessOp::Store => {
                if !VirtualMemoryPermission::Write.contained_in(map.permissions) {
                    return None;
                }
            }
            AccessOp::Instruction => {
                if !VirtualMemoryPermission::Execute.contained_in(map.permissions) {
                    return None;
                }
            }
        }

        if let Some(owner) = &map.owner {
            if op == AccessOp::Store && !map.is_shared {
                let access = crate::object::capability::memory_mapping::AccessKind {
                    op,
                    vaddr,
                    size: Some(1),
                };
                self.lazy_map_page_with(access).ok()?;
                return self.translate_to_phys(vaddr);
            }

            let page_vaddr = vaddr & !(PAGE_SIZE - 1);
            let page_idx = (page_vaddr - map.vm_start) / PAGE_SIZE;
            let access = crate::object::capability::memory_mapping::AccessKind {
                op,
                vaddr,
                size: Some(1),
            };
            if let Ok(res) = owner.resolve_fault(&access, page_idx, map.vm_start) {
                return Some(res.paddr_page_base + (vaddr & (PAGE_SIZE - 1)));
            }
            if map.pmarea.start != 0 {
                return Some(map.pmarea.start + (vaddr - map.vmarea.start));
            }
            return None;
        }

        if map.pmarea.start != 0 {
            Some(map.pmarea.start + (vaddr - map.vmarea.start))
        } else {
            None
        }
    }

    pub fn translate_to_phys(&self, vaddr: usize) -> Option<usize> {
        let map = self.search_memory_map(vaddr)?;

        if let Some(owner) = &map.owner {
            let page_vaddr = vaddr & !(PAGE_SIZE - 1);
            let page_idx = (page_vaddr - map.vm_start) / PAGE_SIZE;
            let access = crate::object::capability::memory_mapping::AccessKind {
                op: crate::object::capability::memory_mapping::AccessOp::Load,
                vaddr,
                size: Some(1),
            };
            if let Ok(res) = owner.resolve_fault(&access, page_idx, map.vm_start) {
                return Some(res.paddr_page_base + (vaddr & (PAGE_SIZE - 1)));
            }
            if map.pmarea.start != 0 {
                return Some(map.pmarea.start + (vaddr - map.vmarea.start));
            }
            return None;
        }

        if map.pmarea.start != 0 {
            Some(map.pmarea.start + (vaddr - map.vmarea.start))
        } else {
            None
        }
    }

    pub fn translate_vaddr(&self, vaddr: usize) -> Option<usize> {
        self.translate_to_kva(vaddr)
    }

    pub fn translate_vaddr_to_phys(&self, vaddr: usize) -> Option<usize> {
        self.translate_to_phys(vaddr)
    }

    /// Gets the mmap base address
    ///
    /// # Returns
    /// The base address for mmap operations
    pub fn get_mmap_base(&self) -> usize {
        self.inner.read().mmap_base
    }

    /// Sets the mmap base address
    /// This allows dynamic adjustment of the mmap region
    ///
    /// # Arguments
    /// * `base` - New base address for mmap operations
    pub fn set_mmap_base(&self, base: usize) {
        self.inner.write().mmap_base = base;
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
        let g = self.inner.read();
        let mut search_addr = (g.mmap_base + alignment - 1) & !(alignment - 1);

        // If there is a mapping that starts before (or at) search_addr but still covers it,
        // we must skip past it. This prevents returning an address inside an existing map.
        if let Some((_, prev_map)) = g.memmap.range(..=search_addr).next_back() {
            if prev_map.vmarea.end >= search_addr {
                search_addr = prev_map.vmarea.end + 1;
                search_addr = (search_addr + alignment - 1) & !(alignment - 1);
            }
        }

        // Simple first-fit algorithm from the adjusted search address
        for (_start, memory_map) in g.memmap.range(search_addr..) {
            // Check if there's enough space before this memory map
            if search_addr + aligned_size <= memory_map.vmarea.start {
                return Some(search_addr);
            }

            // Move search point past this memory map
            if memory_map.vmarea.end >= search_addr {
                search_addr = memory_map.vmarea.end + 1;
                search_addr = (search_addr + alignment - 1) & !(alignment - 1);
            }
        }
        drop(g);
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
    pub fn add_memory_map_fixed(
        &self,
        map: VirtualMemoryMap,
    ) -> Result<Vec<VirtualMemoryMap>, &'static str> {
        // Validate alignment like the regular add_memory_map
        if map.vmarea.start % PAGE_SIZE != 0 || map.vmarea.size() % PAGE_SIZE != 0 {
            return Err("Address or size is not aligned to PAGE_SIZE");
        }
        if map.pmarea.start != 0
            && (map.pmarea.start % PAGE_SIZE != 0 || map.pmarea.size() % PAGE_SIZE != 0)
        {
            return Err("pmarea is not aligned to PAGE_SIZE");
        }

        let new_start = map.vmarea.start;
        let new_end = map.vmarea.end;
        let mut overwritten_mappings = Vec::new();
        let mut mappings_to_add = Vec::new();

        let mut g = self.inner.write();
        let overlapping_keys: alloc::vec::Vec<usize> = g
            .memmap
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

        for key in overlapping_keys {
            if let Some(existing_map) = g.memmap.remove(&key) {
                let existing_start = existing_map.vmarea.start;
                let existing_end = existing_map.vmarea.end;

                // Calculate the overwritten (intersection) part
                let overlap_start = core::cmp::max(new_start, existing_start);
                let overlap_end = core::cmp::min(new_end, existing_end);
                if overlap_start <= overlap_end {
                    // Cut out the pmarea at the same offset as the intersection
                    let overwritten_map = VirtualMemoryMap {
                        vmarea: MemoryArea {
                            start: overlap_start,
                            end: overlap_end,
                        },
                        pmarea: Self::subrange_pmarea(&existing_map, overlap_start, overlap_end),
                        vm_start: existing_map.vm_start,
                        permissions: existing_map.permissions,
                        is_shared: existing_map.is_shared,
                        memory_attribute: existing_map.memory_attribute,
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
                        pmarea: Self::subrange_pmarea(&existing_map, existing_start, new_start - 1),
                        vm_start: existing_map.vm_start,
                        permissions: existing_map.permissions,
                        is_shared: existing_map.is_shared,
                        memory_attribute: existing_map.memory_attribute,
                        owner: existing_map.owner.clone(),
                    };
                    mappings_to_add.push(before_map);
                }

                // Keep the part after the new mapping (if any)
                if existing_end > new_end {
                    let after_map = VirtualMemoryMap {
                        vmarea: MemoryArea {
                            start: new_end + 1,
                            end: existing_end,
                        },
                        pmarea: Self::subrange_pmarea(&existing_map, new_end + 1, existing_end),
                        vm_start: existing_map.vm_start,
                        permissions: existing_map.permissions,
                        is_shared: existing_map.is_shared,
                        memory_attribute: existing_map.memory_attribute,
                        owner: existing_map.owner.clone(),
                    };
                    mappings_to_add.push(after_map);
                }
            }
        }

        // Clear cache since we've modified the memory layout
        g.last_search_cache = None;

        // Remove overlapping mappings from MMU (page table) after releasing lock
        let split_vec = mappings_to_add.clone();
        for split_map in split_vec {
            g.memmap.insert(split_map.vmarea.start, split_map);
        }
        g.memmap.insert(map.vmarea.start, map);
        drop(g);
        for overwritten_map in &overwritten_mappings {
            self.unmap_range_from_mmu(overwritten_map.vmarea.start, overwritten_map.vmarea.end);
        }

        Ok(overwritten_mappings)
    }

    /// Get memory statistics and usage information
    /// This provides detailed information about memory usage patterns
    ///
    /// # Returns
    /// A tuple containing (total_maps, total_virtual_size, fragmentation_info)
    pub fn get_memory_stats(&self) -> (usize, usize, usize) {
        let g = self.inner.read();
        let total_maps = g.memmap.len();
        let total_virtual_size: usize = g
            .memmap
            .values()
            .map(|memory_map| memory_map.vmarea.end - memory_map.vmarea.start + 1)
            .sum();

        // Calculate fragmentation by finding gaps between memory maps
        let mut gaps = 0;
        let mut prev_end = None;

        for memory_map in g.memmap.values() {
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
    pub fn coalesce_memory_maps(&self) -> usize {
        let mut coalesced_count = 0;
        let mut to_remove = Vec::new();
        let mut to_add = Vec::new();
        let mut prev_start: Option<usize> = None;
        let mut prev_map: Option<VirtualMemoryMap> = None;
        let mut g = self.inner.write();
        for (&start, memory_map) in &g.memmap {
            if let (Some(prev_s), Some(prev_memory_map)) = (prev_start, &prev_map) {
                // Check if memory maps are adjacent and can be merged
                if prev_memory_map.vmarea.end + 1 == memory_map.vmarea.start
                    && Self::can_merge_memory_maps(prev_memory_map, memory_map)
                {
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
                        vm_start: prev_memory_map.vm_start,
                        permissions: prev_memory_map.permissions, // Use permissions from first map
                        is_shared: prev_memory_map.is_shared,
                        memory_attribute: prev_memory_map.memory_attribute,
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
            g.memmap.remove(&start);
        }
        for memory_map in to_add {
            g.memmap.insert(memory_map.vmarea.start, memory_map);
        }

        // Clear cache after coalescing
        if coalesced_count > 0 {
            g.last_search_cache = None;
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
        map1.permissions == map2.permissions
            && map1.is_shared == map2.is_shared
            && map1.memory_attribute == map2.memory_attribute
            && map1.pmarea.end + 1 == map2.pmarea.start
    }
}

impl Drop for VirtualMemoryManager {
    /// Drops the virtual memory manager, freeing the address space if it is still in use.
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) != 1 {
            return;
        }

        let memmap = {
            let mut inner = self.inner.write();
            core::mem::take(&mut inner.memmap)
        };
        for map in memmap.into_values() {
            if let Some(owner) = map.owner {
                owner.on_unmapped(map.vmarea.start, map.vmarea.size());
            }
        }

        let asid = self.get_asid();
        if asid != 0 && is_asid_used(asid) {
            free_virtual_address_space(asid);
        }
    }
}

fn find_memory_map_key_with_cache_update(inner: &mut InnerVmm, vaddr: usize) -> Option<usize> {
    if let Some((cache_start, cache_end, cache_key)) = inner.last_search_cache {
        if cache_start <= vaddr && vaddr <= cache_end {
            return Some(cache_key);
        }
    }
    if let Some((start_addr, map)) = inner.memmap.range(..=vaddr).next_back() {
        if map.vmarea.start <= vaddr && vaddr <= map.vmarea.end {
            inner.last_search_cache = Some((map.vmarea.start, map.vmarea.end, *start_addr));
            return Some(*start_addr);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::arch::vm::alloc_virtual_address_space;
    use crate::environment::PAGE_SIZE;
    use crate::object::capability::memory_mapping::anon_owner::AnonymousPageOwner;
    use crate::vm::VirtualMemoryMap;
    use crate::vm::get_current_direct_map_phys_range;
    use crate::vm::{manager::VirtualMemoryManager, vmem::MemoryArea};
    use alloc::sync::Arc;

    #[test_case]
    fn test_new_virtual_memory_manager() {
        let vmm = VirtualMemoryManager::new();
        assert_eq!(vmm.get_asid(), 0);
    }

    #[test_case]
    fn test_set_and_get_asid() {
        let vmm = VirtualMemoryManager::new();
        vmm.set_asid(42);
        assert_eq!(vmm.get_asid(), 42);
    }

    #[test_case]
    fn test_add_and_get_memory_map() {
        let vmm = VirtualMemoryManager::new();
        let vma = MemoryArea {
            start: 0x1000,
            end: 0x1fff,
        };
        let map = VirtualMemoryMap {
            vmarea: vma,
            pmarea: vma,
            vm_start: vma.start,
            permissions: 0,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };
        vmm.add_memory_map(map).unwrap();

        // Use non-cloning with_memmaps API for performance
        assert_eq!(vmm.memmap_len(), 1);
        let first_map_start = vmm.with_memmaps(|m| m.values().next().unwrap().vmarea.start);
        assert_eq!(first_map_start, 0x1000);

        // Test direct address-based access
        assert!(vmm.get_memory_map_by_addr(0x1000).is_some());
        assert_eq!(
            vmm.get_memory_map_by_addr(0x1000).unwrap().vmarea.start,
            0x1000
        );
    }

    #[test_case]
    fn test_remove_memory_map() {
        let vmm = VirtualMemoryManager::new();
        let vma = MemoryArea {
            start: 0x1000,
            end: 0x1fff,
        };
        let map = VirtualMemoryMap {
            vmarea: vma,
            pmarea: vma,
            vm_start: vma.start,
            permissions: 0,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };
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
        let vmm = VirtualMemoryManager::new();
        let vma1 = MemoryArea {
            start: 0x1000,
            end: 0x1fff,
        };
        let map1 = VirtualMemoryMap {
            vmarea: vma1,
            pmarea: vma1,
            vm_start: vma1.start,
            permissions: 0,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };
        let vma2 = MemoryArea {
            start: 0x3000,
            end: 0x3fff,
        };
        let map2 = VirtualMemoryMap {
            vmarea: vma2,
            pmarea: vma2,
            vm_start: vma2.start,
            permissions: 0,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };
        vmm.add_memory_map(map1).unwrap();
        vmm.add_memory_map(map2).unwrap();
        let found_map = vmm.search_memory_map(0x3500).unwrap();
        assert_eq!(found_map.vmarea.start, 0x3000);
    }

    #[test_case]
    fn test_get_root_page_table() {
        let vmm = VirtualMemoryManager::new();
        let asid = alloc_virtual_address_space();
        vmm.set_asid(asid);
        let page_table = vmm.get_root_page_table();
        assert!(page_table.is_some());
    }

    #[test_case]
    fn test_memory_optimization_features() {
        use crate::environment::PAGE_SIZE;

        // Test memory optimization features
        let manager = VirtualMemoryManager::new();

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
            crate::vm::vmem::MemoryArea {
                start: 0x80000000,
                end: 0x80000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x50000000,
                end: 0x50000fff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        manager.add_memory_map(map1).unwrap();

        // Should find space after the first mapping
        let addr2 = manager.find_unmapped_area(size, alignment);
        assert!(addr2.is_some());
        assert!(addr2.unwrap() > 0x50000fff);

        // Regression: if a mapping starts before mmap_base but overlaps it,
        // find_unmapped_area must not return an address inside that mapping.
        manager.set_mmap_base(0x60000000);
        let overlapping_base_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x90000000,
                end: 0x9001ffff,
            },
            crate::vm::vmem::MemoryArea {
                start: 0x5fff0000,
                end: 0x6000ffff,
            },
            0o644,
            false,
            None,
        );
        manager.add_memory_map(overlapping_base_map).unwrap();
        let addr3 = manager.find_unmapped_area(size, alignment).unwrap();
        assert!(addr3 >= 0x60010000);

        // Test memory statistics
        let (total_maps, total_size, gaps) = manager.get_memory_stats();
        assert_eq!(total_maps, 2);
        // map1: 1 page (0x50000000-0x50000fff)
        // overlapping_base_map: 32 pages (0x5fff0000-0x6000ffff)
        // Total: 33 pages
        assert_eq!(total_size, PAGE_SIZE * 33);
        // These two maps are not adjacent, so there is 1 gap between them
        assert_eq!(gaps, 1);

        // Add another non-adjacent map to create another gap
        let map2 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x80002000,
                end: 0x80002fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x50002000,
                end: 0x50002fff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        manager.add_memory_map(map2).unwrap();

        let (total_maps, total_size, gaps) = manager.get_memory_stats();
        assert_eq!(total_maps, 3);
        // map1: 1 page, map2: 1 page, overlapping_base_map: 32 pages = 34 pages total
        assert_eq!(total_size, PAGE_SIZE * 34);
        // Gaps: between map1 and map2 (1), between map2 and overlapping_base_map (2)
        assert_eq!(gaps, 2);

        // Test memory map coalescing (should fail due to non-adjacent physical addresses)
        let coalesced = manager.coalesce_memory_maps();
        assert_eq!(coalesced, 0); // No coalescing possible due to gap
    }

    #[test_case]
    fn test_memory_map_coalescing() {
        use crate::environment::PAGE_SIZE;

        // Test memory map coalescing with adjacent compatible maps
        let manager = VirtualMemoryManager::new();

        // Add two adjacent memory maps that can be merged
        let map1 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x80000000,
                end: 0x80000fff,
            },
            crate::vm::vmem::MemoryArea {
                start: 0x10000000,
                end: 0x10000fff,
            },
            0o644,
            false,
            None,
        );
        let map2 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x80001000,
                end: 0x80001fff,
            },
            crate::vm::vmem::MemoryArea {
                start: 0x10001000,
                end: 0x10001fff,
            },
            0o644, // Same permissions
            false, // Same sharing status
            None,
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
        let merged_map = manager.search_memory_map(0x10000000).unwrap();
        assert_eq!(merged_map.vmarea.start, 0x10000000);
        assert_eq!(merged_map.vmarea.end, 0x10001fff);
    }

    #[test_case]
    fn test_complex_overlap_detection() {
        let manager = VirtualMemoryManager::new();

        // Set up existing memory maps for comprehensive overlap testing
        // Map 1: [0x1000, 0x2000)
        let map1 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x10000000,
                end: 0x10000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x1000,
                end: 0x1fff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        manager.add_memory_map(map1).unwrap();

        // Map 2: [0x4000, 0x5000)
        let map2 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x20000000,
                end: 0x20000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x4000,
                end: 0x4fff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        manager.add_memory_map(map2).unwrap();

        // Map 3: [0x7000, 0x8000)
        let map3 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x30000000,
                end: 0x30000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x7000,
                end: 0x7fff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        manager.add_memory_map(map3).unwrap();

        // Test Case 1: Overlap with previous map (end boundary)
        // Try to add [0x1800, 0x2800) - overlaps with map1's end
        let overlap_with_prev = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x40000000,
                end: 0x40000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x1800,
                end: 0x27ff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(overlap_with_prev).is_err());

        // Test Case 2: Overlap with next map (start boundary)
        // Try to add [0x3800, 0x4800) - overlaps with map2's start
        let overlap_with_next = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x50000000,
                end: 0x50000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x3800,
                end: 0x47ff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(overlap_with_next).is_err());

        // Test Case 3: Complete containment by existing map
        // Try to add [0x1200, 0x1800) - completely inside map1
        let contained_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x60000000,
                end: 0x600005ff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x1200,
                end: 0x17ff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(contained_map).is_err());

        // Test Case 4: Containing an existing map
        // Try to add [0x800, 0x2800) - contains map1 completely
        let containing_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x70000000,
                end: 0x70001fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x800,
                end: 0x27ff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(containing_map).is_err());

        // Test Case 5: Exact boundary collision (touching exactly)
        // Try to add [0x2000, 0x3000) - starts exactly where map1 ends
        let exact_boundary = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x80000000,
                end: 0x80000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x2000,
                end: 0x2fff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(exact_boundary).is_ok()); // Should succeed (touching but not overlapping)

        // Test Case 6: Valid gap insertion
        // Add [0x5000, 0x6000) - fits perfectly between map2 and map3
        let gap_insertion = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x90000000,
                end: 0x90000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x5000,
                end: 0x5fff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(gap_insertion).is_ok());

        // Test Case 7: Edge case - inserting at the very beginning
        // Add [0x0, 0x1000) - before all existing maps
        let beginning_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0xa0000000,
                end: 0xa0000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x0,
                end: 0xfff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(beginning_map).is_ok());

        // Test Case 8: Edge case - inserting at the very end
        // Add [0x8000, 0x9000) - after all existing maps
        let end_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0xb0000000,
                end: 0xb0000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x8000,
                end: 0x8fff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(end_map).is_ok());

        // Verify final state: should have 7 maps total
        assert_eq!(manager.memmap_len(), 7);

        // Verify all maps are accessible and correctly ordered
        let starts: [usize; 7] = [0x0, 0x1000, 0x2000, 0x4000, 0x5000, 0x7000, 0x8000];
        let mut i = 0;
        manager.with_memmaps(|mm| {
            for map in mm.values() {
                assert_eq!(map.vmarea.start, starts[i]);
                i += 1;
            }
        });
        assert_eq!(i, 7);
    }

    #[test_case]
    fn test_alignment_and_edge_cases() {
        let manager = VirtualMemoryManager::new();

        // Test Case 1: Non-aligned virtual address (should fail)
        let misaligned_virtual = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x10000000,
                end: 0x10000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x1001,
                end: 0x2000,
            }, // vmarea - Not PAGE_SIZE aligned
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(misaligned_virtual).is_err());

        // Test Case 2: Non-aligned physical address (should fail)
        let misaligned_physical = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x10000001,
                end: 0x10001000,
            }, // pmarea - Not PAGE_SIZE aligned
            crate::vm::vmem::MemoryArea {
                start: 0x1000,
                end: 0x1fff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(misaligned_physical).is_err());

        // Test Case 3: Non-aligned size (should fail)
        let misaligned_size = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x10000000,
                end: 0x10000800,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x1000,
                end: 0x1800,
            }, // vmarea - Size is not PAGE_SIZE multiple
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(misaligned_size).is_err());

        // Test Case 4: Zero-size mapping (should fail)
        let zero_size = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x10000000,
                end: 0x10000000,
            }, // pmarea - Start == End
            crate::vm::vmem::MemoryArea {
                start: 0x1000,
                end: 0x1000,
            }, // vmarea - Start == End
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(zero_size).is_err());

        // Test Case 5: Single page mapping (should succeed)
        let single_page = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x10000000,
                end: 0x10000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x1000,
                end: 0x1fff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(single_page).is_ok());

        // Test Case 6: Large mapping (multiple pages)
        let large_mapping = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x20000000,
                end: 0x2000ffff,
            }, // pmarea - 64KB
            crate::vm::vmem::MemoryArea {
                start: 0x10000,
                end: 0x1ffff,
            }, // vmarea - 64KB
            0o644,
            false,
            None,
        );
        assert!(manager.add_memory_map(large_mapping).is_ok());

        assert_eq!(manager.memmap_len(), 2);
    }

    #[test_case]
    fn test_cache_invalidation_on_add() {
        let manager = VirtualMemoryManager::new();

        // Add initial mapping
        let map1 = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x10000000,
                end: 0x10000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x1000,
                end: 0x1fff,
            }, // vmarea
            0o644,
            false,
            None,
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
            crate::vm::vmem::MemoryArea {
                start: 0x20000000,
                end: 0x20000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x3000,
                end: 0x3fff,
            }, // vmarea
            0o644,
            false,
            None,
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
        let manager = VirtualMemoryManager::new();

        // Add initial mapping at [0x2000, 0x3000)
        let initial_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x10000000,
                end: 0x10000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x2000,
                end: 0x2fff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        manager.add_memory_map(initial_map).unwrap();
        assert_eq!(manager.memmap_len(), 1);

        // Add fixed mapping that completely contains the existing mapping [0x1000, 0x4000)
        let fixed_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x20000000,
                end: 0x20002fff,
            }, // pmarea - 3 pages
            crate::vm::vmem::MemoryArea {
                start: 0x1000,
                end: 0x3fff,
            }, // vmarea - 3 pages
            0o755,
            true,
            None,
        );

        let result = manager.add_memory_map_fixed(fixed_map);
        assert!(result.is_ok());

        let overwritten_mappings = result.unwrap();
        assert_eq!(overwritten_mappings.len(), 1); // Should have removed one mapping
        assert_eq!(overwritten_mappings[0].vmarea.start, 0x2000);

        // Should now have only the new fixed mapping
        assert_eq!(manager.memmap_len(), 1);
        let remaining_map = manager.search_memory_map(0x2000).unwrap();
        assert_eq!(remaining_map.vmarea.start, 0x1000);
        assert_eq!(remaining_map.vmarea.end, 0x3fff);
        assert_eq!(remaining_map.permissions, 0o755);
        assert_eq!(remaining_map.is_shared, true);
    }

    #[test_case]
    fn test_add_memory_map_fixed_partial_overlap() {
        let manager = VirtualMemoryManager::new();

        // Add initial mapping at [0x1000, 0x3000) - 2 pages
        let initial_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x10000000,
                end: 0x10001fff,
            }, // pmarea - 2 pages
            crate::vm::vmem::MemoryArea {
                start: 0x1000,
                end: 0x2fff,
            }, // vmarea - 2 pages
            0o644,
            false,
            None,
        );
        manager.add_memory_map(initial_map).unwrap();
        assert_eq!(manager.memmap_len(), 1);

        // Add fixed mapping that overlaps from middle: [0x2000, 0x4000) - 2 pages
        let fixed_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x20000000,
                end: 0x20001fff,
            }, // pmarea - 2 pages
            crate::vm::vmem::MemoryArea {
                start: 0x2000,
                end: 0x3fff,
            }, // vmarea - 2 pages
            0o755,
            true,
            None,
        );

        let result = manager.add_memory_map_fixed(fixed_map);
        assert!(result.is_ok());

        let overwritten_mappings = result.unwrap();
        assert_eq!(overwritten_mappings.len(), 1); // Should have removed the original mapping

        // Should now have 2 mappings: the split part [0x1000, 0x2000) and the new fixed [0x2000, 0x4000)
        assert_eq!(manager.memmap_len(), 2);

        // Check the remaining part of the original mapping
        let remaining_original = manager.search_memory_map(0x1500).unwrap();
        assert_eq!(remaining_original.vmarea.start, 0x1000);
        assert_eq!(remaining_original.vmarea.end, 0x1fff);
        assert_eq!(remaining_original.permissions, 0o644);

        // Check the new fixed mapping
        let new_fixed = manager.search_memory_map(0x3000).unwrap();
        assert_eq!(new_fixed.vmarea.start, 0x2000);
        assert_eq!(new_fixed.vmarea.end, 0x3fff);
        assert_eq!(new_fixed.permissions, 0o755);
        assert_eq!(new_fixed.is_shared, true);
    }

    #[test_case]
    fn test_add_memory_map_fixed_split_both_ends() {
        let manager = VirtualMemoryManager::new();

        // Add initial mapping at [0x1000, 0x5000) - 4 pages
        let initial_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x10000000,
                end: 0x10003fff,
            }, // pmarea - 4 pages
            crate::vm::vmem::MemoryArea {
                start: 0x1000,
                end: 0x4fff,
            }, // vmarea - 4 pages
            0o644,
            false,
            None,
        );
        manager.add_memory_map(initial_map).unwrap();
        assert_eq!(manager.memmap_len(), 1);

        // Add fixed mapping in the middle: [0x2000, 0x4000) - 2 pages
        let fixed_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x20000000,
                end: 0x20001fff,
            }, // pmarea - 2 pages
            crate::vm::vmem::MemoryArea {
                start: 0x2000,
                end: 0x3fff,
            }, // vmarea - 2 pages
            0o755,
            true,
            None,
        );

        let result = manager.add_memory_map_fixed(fixed_map);
        assert!(result.is_ok());

        let overwritten_mappings = result.unwrap();
        assert_eq!(overwritten_mappings.len(), 1); // Should have removed the original mapping

        // Should now have 3 mappings: before [0x1000, 0x2000), fixed [0x2000, 0x4000), after [0x4000, 0x5000)
        assert_eq!(manager.memmap_len(), 3);

        // Check the part before the fixed mapping
        let before_part = manager.search_memory_map(0x1500).unwrap();
        assert_eq!(before_part.vmarea.start, 0x1000);
        assert_eq!(before_part.vmarea.end, 0x1fff);
        assert_eq!(before_part.permissions, 0o644);

        // Check the new fixed mapping
        let fixed_part = manager.search_memory_map(0x3000).unwrap();
        assert_eq!(fixed_part.vmarea.start, 0x2000);
        assert_eq!(fixed_part.vmarea.end, 0x3fff);
        assert_eq!(fixed_part.permissions, 0o755);
        assert_eq!(fixed_part.is_shared, true);

        // Check the part after the fixed mapping
        let after_part = manager.search_memory_map(0x4500).unwrap();
        assert_eq!(after_part.vmarea.start, 0x4000);
        assert_eq!(after_part.vmarea.end, 0x4fff);
        assert_eq!(after_part.permissions, 0o644);
    }

    #[test_case]
    fn test_add_memory_map_fixed_no_overlap() {
        let manager = VirtualMemoryManager::new();

        // Add initial mapping at [0x1000, 0x2000)
        let initial_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x10000000,
                end: 0x10000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x1000,
                end: 0x1fff,
            }, // vmarea
            0o644,
            false,
            None,
        );
        manager.add_memory_map(initial_map).unwrap();

        // Add fixed mapping with no overlap at [0x3000, 0x4000)
        let fixed_map = VirtualMemoryMap::new(
            crate::vm::vmem::MemoryArea {
                start: 0x20000000,
                end: 0x20000fff,
            }, // pmarea
            crate::vm::vmem::MemoryArea {
                start: 0x3000,
                end: 0x3fff,
            }, // vmarea
            0o755,
            true,
            None,
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
        let manager = VirtualMemoryManager::new();
        let (dm_phys_start, _) = get_current_direct_map_phys_range();
        let vma = MemoryArea {
            start: 0x1000,
            end: 0x1fff,
        };
        let pma = MemoryArea {
            start: dm_phys_start,
            end: dm_phys_start + 0xfff,
        };
        let map = VirtualMemoryMap {
            vmarea: vma,
            pmarea: pma,
            vm_start: vma.start,
            permissions: 0o644,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };
        let asid = alloc_virtual_address_space();
        manager.set_asid(asid);
        manager.add_memory_map(map).unwrap();

        // Trigger lazy mapping by simulating a page fault at virtual address 0x1500
        assert!(manager.lazy_map_page(0x1500).is_ok());

        // The page should now be mapped in the MMU
        // For testing, we can't directly check MMU state, so we verify by translating the address
        let translated_addr = manager.translate_to_kva(0x1500);
        assert!(translated_addr.is_some());
        assert_eq!(translated_addr.unwrap() & (PAGE_SIZE - 1), 0x500);

        // Test unmapping functionality by removing the memory map
        // This also unmaps from MMU due to our implementation
        manager.remove_memory_map_by_addr(0x1500);

        // Translation should now fail as the memory map is removed
        let translated_addr_after_unmap = manager.translate_to_kva(0x1500);
        assert!(translated_addr_after_unmap.is_none());
    }

    #[test_case]
    fn test_translate_vaddr_returns_none_for_unbacked_map() {
        let manager = VirtualMemoryManager::new();
        let map = VirtualMemoryMap {
            vmarea: MemoryArea {
                start: 0x2000,
                end: 0x2fff,
            },
            pmarea: MemoryArea {
                start: 0,
                end: PAGE_SIZE - 1,
            },
            vm_start: 0x2000,
            permissions: 0,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };

        assert!(manager.add_memory_map(map).is_ok());
        assert!(manager.translate_vaddr(0x2000).is_none());
    }

    #[test_case]
    fn test_remove_memory_map_range_split_left_and_right_segments() {
        let manager = VirtualMemoryManager::new();
        let map = VirtualMemoryMap::new(
            MemoryArea {
                start: 0x8000_0000,
                end: 0x8000_3fff,
            },
            MemoryArea {
                start: 0x4000,
                end: 0x7fff,
            },
            0o644,
            false,
            None,
        );
        manager.add_memory_map(map).unwrap();

        let removed = manager.remove_memory_map_range(0x5000, PAGE_SIZE * 2);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].vmarea.start, 0x5000);
        assert_eq!(removed[0].vmarea.end, 0x6fff);

        assert_eq!(manager.memmap_len(), 2);
        let left = manager.search_memory_map(0x4fff).unwrap();
        assert_eq!(left.vmarea.start, 0x4000);
        assert_eq!(left.vmarea.end, 0x4fff);

        let right = manager.search_memory_map(0x7000).unwrap();
        assert_eq!(right.vmarea.start, 0x7000);
        assert_eq!(right.vmarea.end, 0x7fff);
    }

    #[test_case]
    fn test_remove_memory_map_range_rounds_unaligned_length_to_pages() {
        let manager = VirtualMemoryManager::new();
        let map = VirtualMemoryMap::new(
            MemoryArea {
                start: 0x8000_0000,
                end: 0x8000_4fff,
            },
            MemoryArea {
                start: 0x4000,
                end: 0x8fff,
            },
            0o644,
            false,
            None,
        );
        manager.add_memory_map(map).unwrap();

        let removed = manager.remove_memory_map_range(0x4000, PAGE_SIZE + 0x123);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].vmarea.start, 0x4000);
        assert_eq!(removed[0].vmarea.end, 0x5fff);

        assert!(manager.search_memory_map(0x4000).is_none());
        assert!(manager.search_memory_map(0x5fff).is_none());

        let right = manager.search_memory_map(0x6000).unwrap();
        assert_eq!(right.vmarea.start, 0x6000);
        assert_eq!(right.vmarea.end, 0x8fff);
        assert_eq!(right.pmarea.start, 0x8000_2000);
        assert_eq!(right.pmarea.end, 0x8000_4fff);
    }

    #[test_case]
    fn test_owner_backed_zero_pmarea_stays_zero_when_split() {
        let manager = VirtualMemoryManager::new();
        let owner = Arc::new(AnonymousPageOwner::new());
        let map = VirtualMemoryMap {
            vmarea: MemoryArea {
                start: 0x4000,
                end: 0x7fff,
            },
            pmarea: MemoryArea { start: 0, end: 0 },
            vm_start: 0x4000,
            permissions: 0o644,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: Some(owner),
        };
        manager.add_memory_map(map).unwrap();

        let replacement = VirtualMemoryMap {
            vmarea: MemoryArea {
                start: 0x5000,
                end: 0x5fff,
            },
            pmarea: MemoryArea {
                start: 0x8000_0000,
                end: 0x8000_0fff,
            },
            vm_start: 0x5000,
            permissions: 0o600,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };
        let removed = manager.add_memory_map_fixed(replacement).unwrap();

        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].pmarea, MemoryArea { start: 0, end: 0 });

        let left = manager.search_memory_map(0x4000).unwrap();
        assert_eq!(left.vmarea.start, 0x4000);
        assert_eq!(left.vmarea.end, 0x4fff);
        assert_eq!(left.pmarea, MemoryArea { start: 0, end: 0 });

        let right = manager.search_memory_map(0x6000).unwrap();
        assert_eq!(right.vmarea.start, 0x6000);
        assert_eq!(right.vmarea.end, 0x7fff);
        assert_eq!(right.pmarea, MemoryArea { start: 0, end: 0 });
    }
}
