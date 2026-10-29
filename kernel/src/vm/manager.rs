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
use crate::sync::{IrqRwSpinLock, Mutex};
use alloc::collections::btree_map::Values;
use alloc::{collections::BTreeMap, sync::Arc, vec::Vec};
use core::{ops::Bound, sync::atomic::AtomicUsize};

use crate::mem::page::{ContiguousPages, TaskPages};
use crate::object::capability::memory_mapping::AccessOp;
use crate::{
    arch::vm::{free_virtual_address_space, get_root_pagetable, is_asid_used, mmu::PageTable},
    environment::PAGE_SIZE,
};

use super::addr::{phys_to_virt, validate_direct_map_alias};
use super::vmem::{
    MemoryArea, MemoryAttribute, PhysicalMemoryArea, VirtualMemoryMap, VirtualMemoryPermission,
};

const WRITE_SITE_OWNER_TASK: u64 = 0x4f54;
const WRITE_SITE_SET_ASID: u64 = 0x5341;
const WRITE_SITE_CLONE_MEMMAP: u64 = 0x434d;
const WRITE_SITE_ADD_MAP: u64 = 0x414d;
const WRITE_SITE_REMOVE_MAP: u64 = 0x524d;
const WRITE_SITE_REMOVE_RANGE: u64 = 0x5252;
const WRITE_SITE_REMOVE_ALL: u64 = 0x5241;
const WRITE_SITE_ADD_PAGE_TABLE: u64 = 0x4150;
const WRITE_SITE_COW_COMMIT: u64 = 0x4357;
const WRITE_SITE_EXTEND: u64 = 0x4558;
const WRITE_SITE_MMAP_BASE: u64 = 0x424d;
const WRITE_SITE_ADD_FIXED: u64 = 0x4146;
const WRITE_SITE_COALESCE: u64 = 0x434f;
const WRITE_SITE_DROP: u64 = 0x4452;
const WRITE_SITE_RETAG: u64 = 0x5254;
const WRITE_SITE_RELOCATE: u64 = 0x524c;
const WRITE_SITE_PROTECT: u64 = 0x5052;
const DEBUG_VM_MAPPING_EXTEND_LOGGING: bool = false;
use crate::environment::{DEFAULT_USER_MMAP_BASE, USER_LOWER_CANONICAL_END};

fn checked_align_up(value: usize, alignment: usize) -> Option<usize> {
    if !alignment.is_power_of_two() {
        return None;
    }
    value
        .checked_add(alignment - 1)
        .map(|aligned| aligned & !(alignment - 1))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelocationError {
    InvalidRange,
    UnmappedSource,
    UnsupportedMapping,
    MissingPageTable,
    NoSpace,
}

/// Immutable process-entry stack identity, shared with the address space.
/// The bounds describe the fixed reservation installed by the ELF loader;
/// ordinary thread stacks and later mmap/mprotect calls do not replace it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessEntryStack {
    pub base: usize,
    pub top: usize,
    pub initial_sp: usize,
}

#[derive(Clone)]
pub struct VirtualMemoryManager {
    inner: Arc<IrqRwSpinLock<InnerVmm>>, // shared, internally synchronized
    // Physical backing, brk state, and data accounting belong to the address
    // space, not to an individual Task. CLONE_VM tasks and non-Task VMM owners
    // must keep these alive for exactly as long as they keep the shared
    // mappings alive.
    page_allocations: Arc<IrqRwSpinLock<Vec<ContiguousPages>>>,
    task_pages: Arc<IrqRwSpinLock<Vec<TaskPages>>>,
    brk: Arc<AtomicUsize>,
    data_size: Arc<AtomicUsize>,
    brk_transaction: Arc<Mutex<()>>,
}

impl core::fmt::Debug for VirtualMemoryManager {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Do not lock the backing registries while formatting. Some page
        // owners are intentionally not Debug, and diagnostics may run while
        // one of these locks is already held.
        f.debug_struct("VirtualMemoryManager")
            .field("address_space_owners", &Arc::strong_count(&self.inner))
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct InnerVmm {
    memmap: BTreeMap<usize, VirtualMemoryMap>,
    asid: u16,
    mmap_base: usize,
    // [mmap_base, mmap_occupied_until) is known to contain no holes. Readers
    // may advance this hint under the VM read lock; writers lower it whenever
    // they remove coverage. It never skips a gap too small for an earlier ask.
    mmap_occupied_until: AtomicUsize,
    page_tables: Vec<Arc<PageTable>>,
    last_search_cache: Option<(usize, usize, usize)>,
    owner_task_id: Option<usize>,
    process_entry_stack: Option<ProcessEntryStack>,
}

impl VirtualMemoryManager {
    pub(crate) fn is_exclusive(&self) -> bool {
        Arc::strong_count(&self.inner) == 1
    }

    /// Exchange already prepared images. The caller excludes shared-VM
    /// execution and performs no fallible work after this commit.
    pub(crate) fn exchange_exec_image(&self, other: &Self) {
        assert!(self.is_exclusive() && other.is_exclusive());
        core::mem::swap(&mut *self.inner.write(), &mut *other.inner.write());
        core::mem::swap(
            &mut *self.page_allocations.write(),
            &mut *other.page_allocations.write(),
        );
        core::mem::swap(
            &mut *self.task_pages.write(),
            &mut *other.task_pages.write(),
        );
        use core::sync::atomic::Ordering;
        let old_brk = self
            .brk
            .swap(other.brk.load(Ordering::Relaxed), Ordering::Relaxed);
        other.brk.store(old_brk, Ordering::Relaxed);
        let old_size = self
            .data_size
            .swap(other.data_size.load(Ordering::Relaxed), Ordering::Relaxed);
        other.data_size.store(old_size, Ordering::Relaxed);
    }

    #[inline(always)]
    fn record_inner_writer(&self, site: u64) {
        crate::breadcrumb::drop(
            crate::breadcrumb::VMM_WRITE_HELD,
            site,
            Arc::as_ptr(&self.inner).addr() as u64,
        );
    }

    fn subrange_pmarea(
        existing_map: &VirtualMemoryMap,
        sub_start: usize,
        sub_end: usize,
    ) -> PhysicalMemoryArea {
        if existing_map.pmarea.start == 0 && existing_map.pmarea.end == 0 {
            return PhysicalMemoryArea { start: 0, end: 0 };
        }

        let pm_offset = (sub_start - existing_map.vmarea.start) as u64;
        PhysicalMemoryArea {
            start: existing_map.pmarea.start + pm_offset,
            end: existing_map.pmarea.start + pm_offset + (sub_end - sub_start) as u64,
        }
    }

    fn validate_mapping_direct_map_alias(map: &VirtualMemoryMap) -> Result<(), &'static str> {
        if map.pmarea.start == 0 && map.pmarea.end == 0 {
            return Ok(());
        }

        validate_direct_map_alias(map.pmarea, map.memory_attribute)
    }

    /// Creates a new virtual memory manager.
    ///
    /// # Returns
    /// A new virtual memory manager with default values.
    pub fn new() -> Self {
        let inner = InnerVmm {
            memmap: BTreeMap::new(),
            asid: 0,
            mmap_base: DEFAULT_USER_MMAP_BASE,
            mmap_occupied_until: AtomicUsize::new(DEFAULT_USER_MMAP_BASE),
            page_tables: Vec::new(),
            last_search_cache: None,
            owner_task_id: None,
            process_entry_stack: None,
        };
        VirtualMemoryManager {
            inner: Arc::new(IrqRwSpinLock::new(inner)),
            page_allocations: Arc::new(IrqRwSpinLock::new(Vec::new())),
            task_pages: Arc::new(IrqRwSpinLock::new(Vec::new())),
            brk: Arc::new(AtomicUsize::new(usize::MAX)),
            data_size: Arc::new(AtomicUsize::new(0)),
            brk_transaction: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) fn page_allocations_handle(&self) -> Arc<IrqRwSpinLock<Vec<ContiguousPages>>> {
        Arc::clone(&self.page_allocations)
    }

    /// Called only while constructing an unpublished executable image.
    pub(crate) fn record_process_entry_stack(&self, initial_sp: usize) -> Result<(), &'static str> {
        let (base, top) = self
            .user_mapping_bounds(initial_sp)
            .ok_or("Process entry stack is not user-mapped")?;
        self.inner.write().process_entry_stack = Some(ProcessEntryStack {
            base,
            top,
            initial_sp,
        });
        Ok(())
    }

    pub(crate) fn process_entry_stack(&self) -> Option<ProcessEntryStack> {
        self.inner.read().process_entry_stack
    }

    pub(crate) fn inherit_process_entry_stack(&self, parent: &Self) {
        let stack = parent.process_entry_stack();
        self.inner.write().process_entry_stack = stack;
    }

    pub(crate) fn task_pages_handle(&self) -> Arc<IrqRwSpinLock<Vec<TaskPages>>> {
        Arc::clone(&self.task_pages)
    }

    pub(crate) fn brk_handle(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.brk)
    }

    pub(crate) fn data_size_handle(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.data_size)
    }

    pub(crate) fn brk_transaction_handle(&self) -> Arc<Mutex<()>> {
        Arc::clone(&self.brk_transaction)
    }

    /// Check whether this manager is the only owner of its address space.
    ///
    /// # Returns
    ///
    /// `true` when no other `VirtualMemoryManager` shares the same inner state.
    pub fn is_sole_owner(&self) -> bool {
        Arc::strong_count(&self.inner) == 1
    }

    /// Identify shared address spaces without relying on reusable ASIDs.
    pub(crate) fn shares_address_space(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Count manager instances, including non-Task owners and unpublished clones.
    pub(crate) fn address_space_owner_count(&self) -> usize {
        Arc::strong_count(&self.inner)
    }

    /// Set the owner task ID if this manager does not already have one.
    ///
    /// # Arguments
    /// * `task_id` - Task ID owning this virtual address space.
    pub fn set_owner_task_id_if_unset(&self, task_id: usize) {
        let mut g = self.inner.write();
        self.record_inner_writer(WRITE_SITE_OWNER_TASK);
        if g.owner_task_id.is_none() {
            g.owner_task_id = Some(task_id);
        }
    }

    fn track_private_page_allocation(&self, alloc: ContiguousPages) {
        self.page_allocations.write().push(alloc);
    }

    fn sync_executable_page_for_mapping(permissions: usize, paddr: u64) {
        if VirtualMemoryPermission::Execute.contained_in(permissions) {
            crate::arch::sync_icache_for_execution(phys_to_virt(paddr), PAGE_SIZE);
        }
    }

    /// Sets the ASID (Address Space ID) for the virtual memory manager.
    ///
    /// # Arguments
    /// * `asid` - The ASID to set
    pub fn set_asid(&self, asid: u16) {
        // Capture the old ASID to free and update inner, then release the lock
        // BEFORE touching PMM/page-tables. Holding inner.write() across
        // free_virtual_address_space() forms a lock cycle with PMM and can
        // deadlock against COW/exec paths that take PMM then inner.
        let old_asid_to_free: Option<u16> = {
            let mut g = self.inner.write();
            self.record_inner_writer(WRITE_SITE_SET_ASID);
            if g.asid == asid {
                None
            } else {
                let old = g.asid;
                g.asid = asid;
                if old != 0 && is_asid_used(old) {
                    Some(old)
                } else {
                    None
                }
            }
        };

        if let Some(old) = old_asid_to_free {
            free_virtual_address_space(old);
        }
    }

    /// Returns the ASID (Address Space ID) for the virtual memory manager.
    ///
    /// # Returns
    /// The ASID for the virtual memory manager.
    pub fn get_asid(&self) -> u16 {
        if let Some(inner) = self.inner.try_read() {
            return inner.asid;
        }

        crate::breadcrumb::drop(
            crate::breadcrumb::VMM_READ_WAIT,
            self.inner.writer_count() as u64,
            Arc::as_ptr(&self.inner).addr() as u64,
        );
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
        self.record_inner_writer(WRITE_SITE_CLONE_MEMMAP);
        g.mmap_occupied_until
            .store(g.mmap_base, core::sync::atomic::Ordering::Relaxed);
        g.last_search_cache = None;
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
        Self::validate_memory_map(&map)?;

        let mut g = self.inner.write();
        self.record_inner_writer(WRITE_SITE_ADD_MAP);
        Self::insert_memory_map(&mut g.memmap, map)?;

        g.last_search_cache = None;
        Ok(())
    }

    fn validate_memory_map(map: &VirtualMemoryMap) -> Result<(), &'static str> {
        if map.permissions & !map.maximum_permissions != 0 {
            return Err("Mapping permission ceiling exceeded");
        }
        if map.vmarea.start % PAGE_SIZE != 0 || map.vmarea.size() % PAGE_SIZE != 0 {
            return Err("Address or size is not aligned to PAGE_SIZE");
        }
        if map.pmarea.start != 0
            && (map.pmarea.start % PAGE_SIZE as u64 != 0 || map.pmarea.size() % PAGE_SIZE != 0)
        {
            return Err("pmarea is not aligned to PAGE_SIZE");
        }
        Self::validate_mapping_direct_map_alias(map)
    }

    fn insert_memory_map(
        maps: &mut BTreeMap<usize, VirtualMemoryMap>,
        map: VirtualMemoryMap,
    ) -> Result<(), &'static str> {
        if let Some((_, prev_map)) = maps.range(..map.vmarea.start).next_back() {
            if prev_map.vmarea.end > map.vmarea.start {
                return Err("Memory mapping overlaps with a preceding map");
            }
        }
        if let Some((_, next_map)) = maps.range(map.vmarea.start..).next() {
            if next_map.vmarea.start < map.vmarea.end {
                return Err("Memory mapping overlaps with a succeeding map");
            }
        }

        maps.insert(map.vmarea.start, map);
        Ok(())
    }

    /// Change permissions on a fully mapped native user range, preserving its
    /// backing, owner page indices, and memory attributes through VMA splits.
    /// Old PTEs are revoked before return. Subsequent faults install the new
    /// permissions and synchronize executable pages via
    /// `sync_executable_page_for_mapping` on AArch64 and RISC-V.
    ///
    /// Permission increases are supported for private normal memory backed by
    /// the task or an anonymous/COW owner. Object/device mappings may only lose
    /// permissions because their original maximum is not recorded in the VMA.
    pub(crate) fn protect_memory_map_range(
        &self,
        vmarea: MemoryArea,
        prot: usize,
    ) -> Result<(), &'static str> {
        self.protect_memory_map_range_inner(vmarea, prot, false)
    }

    /// Linux permits restoring access to private anonymous and file-backed
    /// reservations after PROT_NONE. Apply the protection change as one VMA
    /// transaction rather than splitting a large reservation page by page.
    pub(crate) fn protect_linux_memory_map_range(
        &self,
        vmarea: MemoryArea,
        prot: usize,
    ) -> Result<(), &'static str> {
        self.protect_memory_map_range_inner(vmarea, prot, true)
    }

    fn protect_memory_map_range_inner(
        &self,
        vmarea: MemoryArea,
        prot: usize,
        linux_private: bool,
    ) -> Result<(), &'static str> {
        if vmarea.start > vmarea.end
            || vmarea.end >= USER_LOWER_CANONICAL_END
            || !vmarea.start.is_multiple_of(PAGE_SIZE)
            || vmarea.end % PAGE_SIZE != PAGE_SIZE - 1
            || prot & !0x7 != 0
        {
            return Err("Invalid memory protection range or flags");
        }
        let permissions = prot | VirtualMemoryPermission::User as usize;
        let mut g = self.inner.write();
        self.record_inner_writer(WRITE_SITE_PROTECT);
        let keys: Vec<usize> = g
            .memmap
            .iter()
            .filter_map(|(key, map)| {
                (map.vmarea.start <= vmarea.end && vmarea.start <= map.vmarea.end).then_some(*key)
            })
            .collect();
        let mut covered_until = vmarea.start;
        for key in &keys {
            let map = &g.memmap[key];
            let start = map.vmarea.start.max(vmarea.start);
            let end = map.vmarea.end.min(vmarea.end);
            if start != covered_until
                || (!VirtualMemoryPermission::User.contained_in(map.permissions)
                    && !(linux_private && map.permissions == 0))
            {
                return Err("Memory protection range is not fully user-mapped");
            }
            if prot & !map.maximum_permissions != 0 {
                return Err("Mapping permission ceiling exceeded");
            }
            if prot & !map.permissions != 0
                && (map.is_shared
                    || map.memory_attribute != MemoryAttribute::Normal
                    || (!linux_private
                        && map
                            .owner
                            .as_ref()
                            .is_some_and(|owner| !owner.supports_permission_changes())))
            {
                return Err("Mapping does not support increasing permissions");
            }
            covered_until = end + 1;
        }
        if covered_until != vmarea.end + 1 {
            return Err("Memory protection range has an unmapped gap");
        }
        for key in keys {
            let map = g.memmap.remove(&key).unwrap();
            let start = map.vmarea.start.max(vmarea.start);
            let end = map.vmarea.end.min(vmarea.end);
            if map.vmarea.start < start {
                g.memmap.insert(
                    map.vmarea.start,
                    VirtualMemoryMap {
                        vmarea: MemoryArea::new(map.vmarea.start, start - 1),
                        pmarea: Self::subrange_pmarea(&map, map.vmarea.start, start - 1),
                        ..map.clone()
                    },
                );
            }
            if end < map.vmarea.end {
                g.memmap.insert(
                    end + 1,
                    VirtualMemoryMap {
                        vmarea: MemoryArea::new(end + 1, map.vmarea.end),
                        pmarea: Self::subrange_pmarea(&map, end + 1, map.vmarea.end),
                        ..map.clone()
                    },
                );
            }
            g.memmap.insert(
                start,
                VirtualMemoryMap {
                    vmarea: MemoryArea::new(start, end),
                    pmarea: Self::subrange_pmarea(&map, start, end),
                    permissions,
                    ..map
                },
            );
        }
        g.last_search_cache = None;
        drop(g);
        // This is a permission change, not an unmap: owners and physical pages
        // remain alive, so do not send on_unmapped or reclaim backing storage.
        self.unmap_range_from_mmu(vmarea.start, vmarea.end);
        Ok(())
    }

    /// Retags a fully covered range in existing VM metadata without changing page tables.
    ///
    /// This is internal plumbing for a serialized live direct-map retag. The
    /// caller must update the corresponding page-table leaves before publishing
    /// the replacement direct-map metadata, and must pass the attribute currently
    /// recorded for every covered mapping.
    ///
    /// # Arguments
    ///
    /// * `vmarea` - Inclusive page-aligned virtual range to retag.
    /// * `expected_attribute` - Attribute that every covered map must have.
    /// * `memory_attribute` - Attribute to record for the covered range.
    ///
    /// # Returns
    ///
    /// `Ok(())` after replacing only the covered portions, or an error when the
    /// range is invalid, has a gap, or does not have the expected attribute.
    pub(crate) fn retag_memory_map_range(
        &self,
        vmarea: MemoryArea,
        expected_attribute: MemoryAttribute,
        memory_attribute: MemoryAttribute,
    ) -> Result<(), &'static str> {
        if vmarea.start > vmarea.end
            || !vmarea.start.is_multiple_of(PAGE_SIZE)
            || vmarea.end % PAGE_SIZE != PAGE_SIZE - 1
        {
            return Err("VM retag range must be page-aligned and non-empty");
        }

        let mut g = self.inner.write();
        self.record_inner_writer(WRITE_SITE_RETAG);
        let keys: Vec<usize> = g
            .memmap
            .iter()
            .filter_map(|(start, map)| {
                if map.vmarea.start <= vmarea.end && vmarea.start <= map.vmarea.end {
                    Some(*start)
                } else {
                    None
                }
            })
            .collect();

        let mut next_covered = vmarea.start;
        for key in &keys {
            let map = g
                .memmap
                .get(key)
                .expect("retag key must reference an existing map");
            let overlap_start = map.vmarea.start.max(vmarea.start);
            let overlap_end = map.vmarea.end.min(vmarea.end);
            if overlap_start != next_covered {
                return Err("VM retag range is not fully covered");
            }
            if map.memory_attribute != expected_attribute {
                return Err("VM retag range has an unexpected memory attribute");
            }
            if overlap_end == vmarea.end {
                next_covered = vmarea.end;
                break;
            }
            next_covered = overlap_end
                .checked_add(1)
                .ok_or("VM retag range coverage overflows")?;
        }

        if keys.is_empty() || next_covered != vmarea.end {
            return Err("VM retag range is not fully covered");
        }

        let mut replacement = Vec::new();
        for key in keys {
            let map = g
                .memmap
                .remove(&key)
                .expect("retag key must reference an existing map");
            let overlap_start = map.vmarea.start.max(vmarea.start);
            let overlap_end = map.vmarea.end.min(vmarea.end);

            if map.vmarea.start < overlap_start {
                replacement.push(VirtualMemoryMap {
                    vmarea: MemoryArea::new(map.vmarea.start, overlap_start - 1),
                    pmarea: Self::subrange_pmarea(&map, map.vmarea.start, overlap_start - 1),
                    vm_start: map.vm_start,
                    mapping_origin: map.mapping_origin,
                    maximum_permissions: map.maximum_permissions,
                    permissions: map.permissions,
                    is_shared: map.is_shared,
                    memory_attribute: map.memory_attribute,
                    owner: map.owner.clone(),
                });
            }

            replacement.push(VirtualMemoryMap {
                vmarea: MemoryArea::new(overlap_start, overlap_end),
                pmarea: Self::subrange_pmarea(&map, overlap_start, overlap_end),
                vm_start: map.vm_start,
                mapping_origin: map.mapping_origin,
                maximum_permissions: map.maximum_permissions,
                permissions: map.permissions,
                is_shared: map.is_shared,
                memory_attribute,
                owner: map.owner.clone(),
            });

            if overlap_end < map.vmarea.end {
                let after_start = overlap_end
                    .checked_add(1)
                    .expect("overlap end precedes the mapped range end");
                replacement.push(VirtualMemoryMap {
                    vmarea: MemoryArea::new(after_start, map.vmarea.end),
                    pmarea: Self::subrange_pmarea(&map, after_start, map.vmarea.end),
                    vm_start: map.vm_start,
                    mapping_origin: map.mapping_origin,
                    maximum_permissions: map.maximum_permissions,
                    permissions: map.permissions,
                    is_shared: map.is_shared,
                    memory_attribute: map.memory_attribute,
                    owner: map.owner,
                });
            }
        }

        for map in replacement {
            g.memmap.insert(map.vmarea.start, map);
        }
        g.last_search_cache = None;
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
        self.record_inner_writer(WRITE_SITE_REMOVE_MAP);
        let start_addr = find_memory_map_key_with_cache_update(&mut *g, vaddr)?;
        if let Some((_, _, cache_key)) = g.last_search_cache {
            if cache_key == start_addr {
                g.last_search_cache = None;
            }
        }
        let removed_map = g.memmap.remove(&start_addr);
        if let Some(map) = &removed_map {
            g.mmap_occupied_until.fetch_min(
                map.vmarea.start.max(g.mmap_base),
                core::sync::atomic::Ordering::Relaxed,
            );
        }
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
        let mut g = self.inner.write();
        self.record_inner_writer(WRITE_SITE_REMOVE_RANGE);
        let removed_maps = Self::remove_memory_map_range_locked(&mut g, remove_start, remove_end);
        drop(g);
        self.unmap_range_from_mmu(remove_start, remove_end);
        removed_maps
    }

    /// Shrink a covered portion of one logical mapping, including its COW splits.
    /// Validation and metadata removal share one lock. Physical backing remains
    /// owned by the returned descriptors until the caller reclaims it after the
    /// page-table invalidation performed here.
    pub(crate) fn shrink_memory_map_range(
        &self,
        vaddr: usize,
        old_len: usize,
        new_len: usize,
    ) -> Result<Vec<VirtualMemoryMap>, &'static str> {
        if !vaddr.is_multiple_of(PAGE_SIZE)
            || !old_len.is_multiple_of(PAGE_SIZE)
            || !new_len.is_multiple_of(PAGE_SIZE)
            || new_len == 0
            || new_len > old_len
        {
            return Err("Invalid mapping shrink range");
        }
        let old_end = vaddr.checked_add(old_len - 1).ok_or("Range overflow")?;
        let new_end = vaddr.checked_add(new_len - 1).ok_or("Range overflow")?;
        let mut g = self.inner.write();
        self.record_inner_writer(WRITE_SITE_REMOVE_RANGE);
        let first = g
            .memmap
            .range(..=vaddr)
            .next_back()
            .map(|(_, map)| map)
            .filter(|map| vaddr <= map.vmarea.end)
            .ok_or("Unmapped source range")?;
        let (origin, mapping_origin, permissions, shared, attribute) = (
            first.vm_start,
            first.mapping_origin,
            first.permissions,
            first.is_shared,
            first.memory_attribute,
        );
        let mut next = vaddr;
        let mut covered = false;
        for (_, map) in g.memmap.range(first.vmarea.start..) {
            if map.vmarea.start > next
                || map.vm_start != origin
                || map.mapping_origin != mapping_origin
                || map.permissions != permissions
                || map.is_shared != shared
                || map.memory_attribute != attribute
            {
                return Err("Source range has a gap or different mapping types");
            }
            if map.vmarea.end >= old_end {
                covered = true;
                break;
            }
            next = map.vmarea.end + 1;
        }
        if !covered {
            return Err("Unmapped source range");
        }
        if new_len == old_len {
            return Ok(Vec::new());
        }
        let removed = Self::remove_memory_map_range_locked(&mut g, new_end + 1, old_end);
        drop(g);
        self.unmap_range_from_mmu(new_end + 1, old_end);
        Ok(removed)
    }

    fn movable_private(map: &VirtualMemoryMap) -> bool {
        !map.is_shared
            && map.memory_attribute == MemoryAttribute::Normal
            && (map.permissions == 0
                || map.permissions & (VirtualMemoryPermission::User as usize) != 0)
            && map.owner.as_ref().map_or(map.pmarea.start != 0, |owner| {
                owner.supports_private_relocation()
            })
    }

    /// Validate one logical allocation, including resident and COW fragments.
    /// The caller holds the VM lock through its subsequent metadata commit.
    fn private_range(
        g: &InnerVmm,
        source: usize,
        source_end: usize,
    ) -> Result<VirtualMemoryMap, RelocationError> {
        let first = g
            .memmap
            .range(..=source)
            .next_back()
            .map(|(_, m)| m)
            .filter(|m| source <= m.vmarea.end)
            .ok_or(RelocationError::UnmappedSource)?;
        let mut next = source;
        for (_, map) in g.memmap.range(first.vmarea.start..=source_end) {
            if map.vmarea.start > next {
                return Err(RelocationError::UnmappedSource);
            }
            if map.mapping_origin != first.mapping_origin
                || map.vm_start != first.vm_start
                || map.maximum_permissions != first.maximum_permissions
                || map.permissions != first.permissions
                || !Self::movable_private(map)
            {
                return Err(RelocationError::UnsupportedMapping);
            }
            next = map.vmarea.end.min(source_end) + 1;
        }
        if next != source_end + 1 {
            return Err(RelocationError::UnmappedSource);
        }
        Ok(first.clone())
    }

    /// Grow private anonymous memory, optionally moving to a free range.
    /// Select the destination, validate and commit under one VM lock. Existing
    /// backing is never copied/reclaimed here; new pages have fresh zero backing
    /// rather than extending a finite COW owner or reviving a discarded page.
    pub(crate) fn grow_private_anonymous_range(
        &self,
        source: usize,
        old_len: usize,
        new_len: usize,
        may_move: bool,
    ) -> Result<usize, RelocationError> {
        if source == 0
            || !source.is_multiple_of(PAGE_SIZE)
            || old_len == 0
            || !old_len.is_multiple_of(PAGE_SIZE)
            || new_len <= old_len
            || !new_len.is_multiple_of(PAGE_SIZE)
            || source
                .checked_add(old_len - 1)
                .is_none_or(|end| end > USER_LOWER_CANONICAL_END)
        {
            return Err(RelocationError::InvalidRange);
        }
        let source_end = source + old_len - 1;
        let asid = self.get_asid();
        let mut page_table = if asid != 0 && is_asid_used(asid) {
            Some(get_root_pagetable(asid).ok_or(RelocationError::MissingPageTable)?)
        } else {
            None
        };
        let mut g = self.inner.write();
        self.record_inner_writer(WRITE_SITE_EXTEND);
        if g.asid != asid {
            return Err(RelocationError::MissingPageTable);
        }
        let first = Self::private_range(&g, source, source_end)?;
        if !first.mapping_origin.is_anonymous() {
            return Err(RelocationError::UnsupportedMapping);
        }
        let fits_in_place = source.checked_add(new_len - 1).is_some_and(|end| {
            end <= USER_LOWER_CANONICAL_END
                && !g
                    .memmap
                    .range(..=end)
                    .next_back()
                    .is_some_and(|(_, map)| map.vmarea.end > source_end)
        });
        let destination = if fits_in_place {
            source
        } else if may_move {
            Self::find_unmapped_area_in(&g, new_len, PAGE_SIZE).ok_or(RelocationError::NoSpace)?
        } else {
            return Err(RelocationError::NoSpace);
        };
        let offset = source
            .checked_sub(first.vm_start)
            .ok_or(RelocationError::UnsupportedMapping)?;
        let new_vm_start = destination
            .checked_sub(offset)
            .ok_or(RelocationError::UnsupportedMapping)?;
        let destination_end = destination + new_len - 1;
        let tail = VirtualMemoryMap {
            vmarea: MemoryArea::new(destination + old_len, destination_end),
            pmarea: PhysicalMemoryArea::new(0, 0),
            vm_start: new_vm_start,
            mapping_origin: first.mapping_origin,
            maximum_permissions: first.maximum_permissions,
            permissions: first.permissions,
            is_shared: false,
            memory_attribute: MemoryAttribute::Normal,
            owner: Some(Arc::new(
                crate::object::capability::memory_mapping::anon_owner::AnonymousZeroPageOwner::new(
                ),
            )),
        };
        // No fallible operation remains. Preserve page indices for existing
        // owners, and make both old and new fault snapshots stale on a move.
        if destination != source {
            let moved = Self::remove_memory_map_range_locked(&mut g, source, source_end);
            for mut map in moved {
                map.vmarea = MemoryArea::new(
                    destination + (map.vmarea.start - source),
                    destination + (map.vmarea.end - source),
                );
                map.vm_start = new_vm_start;
                g.memmap.insert(map.vmarea.start, map);
            }
        }
        g.memmap.insert(tail.vmarea.start, tail);
        g.last_search_cache = None;
        if let Some(table) = page_table.as_mut() {
            if destination != source {
                table.unmap_range(source, source_end);
                table.unmap_range(destination, destination_end);
            } else {
                table.unmap_range(source + old_len, destination_end);
            }
        }
        Ok(destination)
    }

    /// Move one covered private allocation to a disjoint user range of the
    /// same size. The destination may replace private normal-memory mappings.
    /// Resident physical pages and lazy/COW owners retain their identity; only
    /// virtual metadata moves. Returned destination backing may be reclaimed
    /// after both old page-table ranges have been invalidated.
    pub(crate) fn relocate_private_memory_map_range(
        &self,
        source: usize,
        destination: usize,
        length: usize,
    ) -> Result<Vec<VirtualMemoryMap>, RelocationError> {
        let valid = |start: usize| {
            start != 0
                && start.is_multiple_of(PAGE_SIZE)
                && length != 0
                && length.is_multiple_of(PAGE_SIZE)
                && start
                    .checked_add(length - 1)
                    .is_some_and(|end| end <= USER_LOWER_CANONICAL_END)
        };
        if !valid(source) || !valid(destination) {
            return Err(RelocationError::InvalidRange);
        }
        let source_end = source + length - 1;
        let destination_end = destination + length - 1;
        if source <= destination_end && destination <= source_end {
            return Err(RelocationError::InvalidRange);
        }
        // Page-table lock precedes VM metadata, as in fault publication below.
        // This also excludes stale source/destination PTE installations until
        // the metadata update and both TLB invalidations have completed.
        let asid = self.get_asid();
        let mut page_table = if asid != 0 && is_asid_used(asid) {
            Some(get_root_pagetable(asid).ok_or(RelocationError::MissingPageTable)?)
        } else {
            None
        };
        let mut g = self.inner.write();
        self.record_inner_writer(WRITE_SITE_RELOCATE);
        if g.asid != asid {
            return Err(RelocationError::MissingPageTable);
        }
        let first = Self::private_range(&g, source, source_end)?;
        let offset = source
            .checked_sub(first.vm_start)
            .ok_or(RelocationError::UnsupportedMapping)?;
        let new_vm_start = destination
            .checked_sub(offset)
            .ok_or(RelocationError::UnsupportedMapping)?;
        // Device and object callbacks can depend on original virtual addresses.
        // Reject those targets before changing either range.
        let first_target = g
            .memmap
            .range(..=destination)
            .next_back()
            .map_or(destination, |(key, _)| *key);
        if g.memmap
            .range(first_target..=destination_end)
            .any(|(_, m)| m.vmarea.end >= destination && !Self::movable_private(m))
        {
            return Err(RelocationError::UnsupportedMapping);
        }
        let mut moved = Self::remove_memory_map_range_locked(&mut g, source, source_end);
        let replaced = Self::remove_memory_map_range_locked(&mut g, destination, destination_end);
        for map in &mut moved {
            map.vmarea = MemoryArea::new(
                destination + (map.vmarea.start - source),
                destination + (map.vmarea.end - source),
            );
            map.vm_start = new_vm_start;
            g.memmap.insert(map.vmarea.start, map.clone());
        }
        g.last_search_cache = None;
        if let Some(table) = page_table.as_mut() {
            table.unmap_range(source, source_end);
            table.unmap_range(destination, destination_end);
        }
        Ok(replaced)
    }

    fn remove_memory_map_range_locked(
        g: &mut InnerVmm,
        remove_start: usize,
        remove_end: usize,
    ) -> Vec<VirtualMemoryMap> {
        let mut removed_maps = Vec::new();
        let mut mappings_to_add = Vec::new();

        // Find all mappings that overlap with the removal range
        // A sorted, non-overlapping map can intersect the range only at its
        // predecessor or at keys inside the range. Avoid scanning every VMA
        // for each small allocator munmap in a shader-heavy process.
        let first_key = g
            .memmap
            .range(..=remove_start)
            .next_back()
            .map(|(key, _)| *key)
            .unwrap_or(remove_start);
        let overlapping_keys: alloc::vec::Vec<usize> = g
            .memmap
            .range(first_key..=remove_end)
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
                        mapping_origin: existing_map.mapping_origin,
                        maximum_permissions: existing_map.maximum_permissions,
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
                        mapping_origin: existing_map.mapping_origin,
                        maximum_permissions: existing_map.maximum_permissions,
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
                        mapping_origin: existing_map.mapping_origin,
                        maximum_permissions: existing_map.maximum_permissions,
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

        if !removed_maps.is_empty() {
            g.mmap_occupied_until.fetch_min(
                remove_start.max(g.mmap_base),
                core::sync::atomic::Ordering::Relaxed,
            );
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
        let memmap = {
            let mut g = self.inner.write();
            self.record_inner_writer(WRITE_SITE_REMOVE_ALL);
            g.last_search_cache = None;
            g.mmap_occupied_until
                .store(g.mmap_base, core::sync::atomic::Ordering::Relaxed);
            core::mem::take(&mut g.memmap)
        };

        self.unmap_all_from_mmu();
        memmap.into_values()
    }

    /// Replaces the current memory maps with a validated restored set.
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
        let mut restored_maps = BTreeMap::new();
        for map in maps {
            Self::validate_memory_map(&map)?;
            Self::insert_memory_map(&mut restored_maps, map)?;
        }

        let replaced_maps = {
            let mut g = self.inner.write();
            self.record_inner_writer(WRITE_SITE_REMOVE_ALL);
            g.last_search_cache = None;
            g.mmap_occupied_until
                .store(g.mmap_base, core::sync::atomic::Ordering::Relaxed);
            core::mem::replace(&mut g.memmap, restored_maps)
        };

        self.unmap_all_from_mmu();
        drop(replaced_maps);
        Ok(())
    }

    /// Contiguous user region with one mapping origin and protection, including
    /// private COW fragments. A touched page is not a whole stack boundary.
    /// Hold one read lock so a concurrent split/unmap cannot produce mixed bounds.
    pub fn user_mapping_bounds(&self, address: usize) -> Option<(usize, usize)> {
        let g = self.inner.read();
        let (_, first) = g.memmap.range(..=address).next_back()?;
        if address > first.vmarea.end || first.permissions & 0x08 == 0 {
            return None;
        }
        let same = |map: &VirtualMemoryMap| {
            map.mapping_origin == first.mapping_origin
                && map.vm_start == first.vm_start
                && map.maximum_permissions == first.maximum_permissions
                && map.permissions == first.permissions
                && map.is_shared == first.is_shared
                && map.memory_attribute == first.memory_attribute
        };
        let mut start = first.vmarea.start;
        let mut end = first.vmarea.end;
        for (_, map) in g.memmap.range(..start).rev() {
            if map.vmarea.end.checked_add(1) != Some(start) || !same(map) {
                break;
            }
            start = map.vmarea.start;
        }
        for (_, map) in g.memmap.range((
            core::ops::Bound::Excluded(first.vmarea.start),
            core::ops::Bound::Unbounded,
        )) {
            if end.checked_add(1) != Some(map.vmarea.start) || !same(map) {
                break;
            }
            end = map.vmarea.end;
        }
        Some((start, end.checked_add(1)?))
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
        // Read lock only. This used to take inner.write() to update a last-find
        // cache, which serialized every COW fault on the same vm_manager (e.g.
        // stemd + its CLONE_VM IPC thread) and could re-enter the lock via
        // owner.resolve_fault() paths (framebuffer). The cache is intentionally
        // not updated here; the BTreeMap range lookup below stays correct.
        let g = self.inner.read();
        if let Some((cache_start, cache_end, cache_key)) = g.last_search_cache {
            if cache_start <= vaddr && vaddr <= cache_end {
                if let Some(map) = g.memmap.get(&cache_key) {
                    if vaddr <= map.vmarea.end {
                        return Some(map.clone());
                    }
                }
            }
        }
        g.memmap.range(..=vaddr).next_back().and_then(|(_, map)| {
            if vaddr <= map.vmarea.end {
                Some(map.clone())
            } else {
                None
            }
        })
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
        let mut inner = self.inner.write();
        self.record_inner_writer(WRITE_SITE_ADD_PAGE_TABLE);
        inner.page_tables.push(page_table);
    }

    /// Returns the root page table for the current address space.
    ///
    /// # Returns
    /// The root page table for the current address space, if it exists.
    pub fn get_root_page_table(&self) -> Option<crate::arch::vm::RootPageTableGuard> {
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

    fn page_backing_at(map: &VirtualMemoryMap, page_vaddr: usize) -> Option<u64> {
        if map.pmarea.start == 0 {
            return Some(0);
        }

        let offset = page_vaddr.checked_sub(map.vmarea.start)?;
        map.pmarea.start.checked_add(offset as u64)
    }

    fn commit_private_cow_page(
        &self,
        expected: &VirtualMemoryMap,
        replacement: VirtualMemoryMap,
    ) -> Result<bool, &'static str> {
        let page_vaddr = replacement.vmarea.start;
        let page_end = page_vaddr
            .checked_add(PAGE_SIZE - 1)
            .ok_or("COW page virtual address overflow")?;
        if page_vaddr % PAGE_SIZE != 0 || replacement.vmarea.end != page_end {
            return Err("COW replacement is not one aligned virtual page");
        }
        let physical_page_end = replacement
            .pmarea
            .start
            .checked_add((PAGE_SIZE - 1) as u64)
            .ok_or("COW page physical address overflow")?;
        if replacement.pmarea.start == 0
            || replacement.pmarea.start % PAGE_SIZE as u64 != 0
            || replacement.pmarea.end != physical_page_end
        {
            return Err("COW replacement is not one aligned physical page");
        }

        let expected_owner = match expected.owner.as_ref() {
            Some(owner) => owner,
            None => return Ok(false),
        };
        if expected.is_shared
            || replacement.mapping_origin != expected.mapping_origin
            || replacement.maximum_permissions != expected.maximum_permissions
            || page_vaddr < expected.vmarea.start
            || page_end > expected.vmarea.end
        {
            return Ok(false);
        }

        let mut inner = self.inner.write();
        self.record_inner_writer(WRITE_SITE_COW_COMMIT);
        let current_key = match inner.memmap.range(..=page_vaddr).next_back() {
            Some((key, current)) if page_end <= current.vmarea.end => *key,
            _ => return Ok(false),
        };
        let source_matches = match inner.memmap.get(&current_key) {
            Some(current) => {
                let owner_matches = current
                    .owner
                    .as_ref()
                    .is_some_and(|owner| Arc::ptr_eq(owner, expected_owner));
                owner_matches
                    && !current.is_shared
                    && current.mapping_origin == expected.mapping_origin
                    && current.vm_start == expected.vm_start
                    && current.maximum_permissions == expected.maximum_permissions
                    && current.permissions == expected.permissions
                    && current.memory_attribute == expected.memory_attribute
                    && Self::page_backing_at(current, page_vaddr)
                        == Self::page_backing_at(expected, page_vaddr)
            }
            None => false,
        };
        if !source_matches {
            return Ok(false);
        }

        let current = inner
            .memmap
            .remove(&current_key)
            .ok_or("COW source mapping disappeared during commit")?;
        if current.vmarea.start < page_vaddr {
            let left = VirtualMemoryMap {
                vmarea: MemoryArea::new(current.vmarea.start, page_vaddr - 1),
                pmarea: Self::subrange_pmarea(&current, current.vmarea.start, page_vaddr - 1),
                vm_start: current.vm_start,
                mapping_origin: current.mapping_origin,
                maximum_permissions: current.maximum_permissions,
                permissions: current.permissions,
                is_shared: current.is_shared,
                memory_attribute: current.memory_attribute,
                owner: current.owner.clone(),
            };
            inner.memmap.insert(left.vmarea.start, left);
        }
        if page_end < current.vmarea.end {
            let right = VirtualMemoryMap {
                vmarea: MemoryArea::new(page_end + 1, current.vmarea.end),
                pmarea: Self::subrange_pmarea(&current, page_end + 1, current.vmarea.end),
                vm_start: current.vm_start,
                mapping_origin: current.mapping_origin,
                maximum_permissions: current.maximum_permissions,
                permissions: current.permissions,
                is_shared: current.is_shared,
                memory_attribute: current.memory_attribute,
                owner: current.owner.clone(),
            };
            inner.memmap.insert(right.vmarea.start, right);
        }
        inner.memmap.insert(page_vaddr, replacement);
        inner.last_search_cache = None;
        drop(inner);
        drop(current);
        Ok(true)
    }

    fn fault_snapshot_matches(inner: &InnerVmm, expected: &VirtualMemoryMap, page: usize) -> bool {
        inner
            .memmap
            .range(..=page)
            .next_back()
            .is_some_and(|(_, current)| {
                page + PAGE_SIZE - 1 <= current.vmarea.end
                    && current.mapping_origin == expected.mapping_origin
                    && current.vm_start == expected.vm_start
                    && current.maximum_permissions == expected.maximum_permissions
                    && current.permissions == expected.permissions
                    && current.is_shared == expected.is_shared
                    && current.memory_attribute == expected.memory_attribute
                    && Self::page_backing_at(current, page) == Self::page_backing_at(expected, page)
                    && match (&current.owner, &expected.owner) {
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                        (None, None) => true,
                        _ => false,
                    }
            })
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

        crate::breadcrumb::drop(crate::breadcrumb::LAZY_FOUND, vaddr as u64, 0);

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
                    crate::breadcrumb::drop(
                        crate::breadcrumb::LAZY_RESOLVED,
                        res.paddr_page_base as u64,
                        0,
                    );
                    perms = owner.fault_page_permissions(&access, perms) & memory_map.maximum_permissions;
                    if res.is_tail {
                        perms &= !0x1;
                        perms &= !0x2;
                    }

                    // COW: most private owner-based mappings allocate a private
                    // page on any resolved fault. Fork COW owners allow reads to
                    // share a read-only page and request a copy only on stores.
                    if !memory_map.is_shared && owner.private_fault_requires_copy(&access) {
                        crate::breadcrumb::drop(
                            crate::breadcrumb::COW_COPY_REQUIRED,
                            page_vaddr as u64,
                            0,
                        );
                        crate::breadcrumb::drop(
                            crate::breadcrumb::PMM_ALLOC_BEGIN,
                            page_vaddr as u64,
                            0,
                        );
                        let new_alloc = ContiguousPages::new(1)
                            .ok_or("Failed to allocate page for private mapping COW")?;
                        let new_paddr = new_alloc.as_paddr();
                        crate::breadcrumb::drop(
                            crate::breadcrumb::PMM_ALLOC_DONE,
                            new_paddr as u64,
                            0,
                        );
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                crate::vm::addr::phys_to_virt(res.paddr_page_base) as *const u8,
                                crate::vm::addr::phys_to_virt(new_paddr) as *mut u8,
                                PAGE_SIZE,
                            );
                        }
                        let cow_map = VirtualMemoryMap {
                            pmarea: crate::vm::vmem::PhysicalMemoryArea::new(
                                new_paddr,
                                new_paddr + PAGE_SIZE as u64 - 1,
                            ),
                            vmarea: MemoryArea::new(page_vaddr, page_vaddr + PAGE_SIZE - 1),
                            vm_start: memory_map.vm_start,
                            mapping_origin: memory_map.mapping_origin,
                            maximum_permissions: memory_map.maximum_permissions,
                            permissions: perms,
                            is_shared: false,
                            memory_attribute: memory_map.memory_attribute,
                            owner: None,
                        };
                        if !self.commit_private_cow_page(&memory_map, cow_map.clone())? {
                            drop(new_alloc);
                            return self.lazy_map_page_with(access);
                        }
                        self.track_private_page_allocation(new_alloc);
                        crate::breadcrumb::drop(
                            crate::breadcrumb::LAZY_COWDONE,
                            page_vaddr as u64,
                            new_paddr as u64,
                        );
                        Self::sync_executable_page_for_mapping(perms, new_paddr);
                        let asid = self.get_asid();
                        crate::breadcrumb::drop(
                            crate::breadcrumb::PT_LOCK_WAIT,
                            asid as u64,
                            page_vaddr as u64,
                        );
                        let Some(mut root_pagetable) = self.get_root_page_table() else {
                            return Err("No root page table available for COW mapping");
                        };
                        let current = self.inner.read();
                        if !Self::fault_snapshot_matches(&current, &cow_map, page_vaddr) {
                            drop(current);
                            drop(root_pagetable);
                            return self.lazy_map_page_with(access);
                        }
                        crate::breadcrumb::drop(
                            crate::breadcrumb::PT_LOCK_DONE,
                            asid as u64,
                            page_vaddr as u64,
                        );
                        crate::breadcrumb::drop(
                            crate::breadcrumb::PT_MAP_BEGIN,
                            page_vaddr as u64,
                            new_paddr as u64,
                        );
                        root_pagetable.map(
                            page_vaddr,
                            new_paddr,
                            perms,
                            memory_map.memory_attribute,
                            true,
                            access.op == AccessOp::Store,
                        );
                        crate::breadcrumb::drop(
                            crate::breadcrumb::PT_MAP_DONE,
                            page_vaddr as u64,
                            new_paddr as u64,
                        );
                        return Ok(());
                    }

                    res.paddr_page_base
                }
                Err(_) => {
                    if memory_map.pmarea.start != 0 {
                        memory_map.pmarea.start + (page_vaddr - memory_map.vmarea.start) as u64
                    } else {
                        return Err("Owner failed to resolve fault");
                    }
                }
            }
        } else {
            memory_map.pmarea.start + (page_vaddr - memory_map.vmarea.start) as u64
        };

        let asid = self.get_asid();
        crate::breadcrumb::drop(
            crate::breadcrumb::PT_LOCK_WAIT,
            asid as u64,
            page_vaddr as u64,
        );
        if let Some(mut root_pagetable) = self.get_root_page_table() {
            let current = self.inner.read();
            if !Self::fault_snapshot_matches(&current, &memory_map, page_vaddr) {
                drop(current);
                drop(root_pagetable);
                return self.lazy_map_page_with(access);
            }
            crate::breadcrumb::drop(
                crate::breadcrumb::PT_LOCK_DONE,
                asid as u64,
                page_vaddr as u64,
            );
            Self::sync_executable_page_for_mapping(perms, page_paddr);
            crate::breadcrumb::drop(
                crate::breadcrumb::PT_MAP_BEGIN,
                page_vaddr as u64,
                page_paddr as u64,
            );
            root_pagetable.map(
                page_vaddr,
                page_paddr,
                perms,
                memory_map.memory_attribute,
                true,
                access.op == AccessOp::Store,
            );
            crate::breadcrumb::drop(
                crate::breadcrumb::PT_MAP_DONE,
                page_vaddr as u64,
                page_paddr as u64,
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

        // Result of successful extend: (paddr_page_base, permissions, memory_attribute)
        let extend_result: Option<(u64, usize, MemoryAttribute)>;

        {
            // Lock scope
            let mut g = self.inner.write();
            self.record_inner_writer(WRITE_SITE_EXTEND);

            // Find a mapping whose vmarea.end < vaddr and whose owner explicitly
            // supports VMA growth after its backing object was resized.
            let mut found = None;
            let candidate_keys: Vec<usize> = g.memmap.keys().copied().collect();
            for key in candidate_keys {
                let Some((old_end, vm_start, map_permissions, memory_attribute, owner)) =
                    g.memmap.get(&key).and_then(|map| {
                        if map.vmarea.end >= vaddr {
                            return None;
                        }

                        let owner = map.owner.as_ref()?;
                        if !owner.can_extend_vma_on_fault() {
                            return None;
                        }

                        Some((
                            map.vmarea.end,
                            map.vm_start,
                            map.permissions,
                            map.memory_attribute,
                            owner.clone(),
                        ))
                    })
                else {
                    continue;
                };

                let Some(offset) = page_vaddr.checked_sub(vm_start) else {
                    continue;
                };
                let Some(new_end) = page_vaddr.checked_add(PAGE_SIZE - 1) else {
                    continue;
                };
                let overlaps_next = match g
                    .memmap
                    .range((Bound::Excluded(key), Bound::Unbounded))
                    .next()
                {
                    Some((_, next_map)) => new_end >= next_map.vmarea.start,
                    None => false,
                };
                if overlaps_next {
                    continue;
                }

                // Try resolve_fault to see if owner supports this offset.
                let test_access = crate::object::capability::memory_mapping::AccessKind {
                    vaddr: page_vaddr,
                    op: access.op,
                    size: access.size,
                };

                let page_idx = offset / PAGE_SIZE;
                match owner.resolve_fault(&test_access, page_idx, vm_start) {
                    Ok(res) => {
                        let permissions =
                            owner.fault_page_permissions(&test_access, map_permissions) & map_permissions;
                        if let Some(map) = g.memmap.get_mut(&key) {
                            if DEBUG_VM_MAPPING_EXTEND_LOGGING {
                                crate::println!(
                                    "[VmManager] Extending mapping vmarea.end from {:#x} to {:#x} for owner={}",
                                    old_end,
                                    new_end,
                                    owner.mmap_owner_name()
                                );
                            }
                            map.vmarea.end = new_end;
                            g.last_search_cache = None;
                        }

                        found = Some((res.paddr_page_base, permissions, memory_attribute));
                        break;
                    }
                    Err(_) => {
                        // Owner doesn't support this offset, continue searching.
                    }
                }
            }
            extend_result = found;
        } // Lock released here

        // Now map the page outside the lock
        if let Some((paddr_page_base, permissions, memory_attribute)) = extend_result {
            if let Some(mut root_pagetable) = self.get_root_page_table() {
                root_pagetable.map(
                    page_vaddr,
                    paddr_page_base,
                    permissions,
                    memory_attribute,
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
        let asid = self.get_asid();
        if asid == 0 || !is_asid_used(asid) {
            return;
        }

        let Some(mut root_pagetable) = self.get_root_page_table() else {
            panic!(
                "Cannot unmap {:#x}-{:#x}: live ASID {} has no root page table",
                vaddr_start, vaddr_end, asid
            );
        };
        root_pagetable.unmap_range(vaddr_start, vaddr_end);
    }

    fn unmap_all_from_mmu(&self) {
        let asid = self.get_asid();
        if asid == 0 || !is_asid_used(asid) {
            return;
        }

        let Some(mut root_pagetable) = self.get_root_page_table() else {
            panic!(
                "Cannot unmap all mappings: live ASID {} has no root page table",
                asid
            );
        };
        root_pagetable.unmap_all();
    }

    pub fn translate_to_kva(&self, vaddr: usize) -> Option<usize> {
        self.translate_to_phys(vaddr).map(phys_to_virt)
    }

    pub fn translate_to_kva_for_write(&self, vaddr: usize) -> Option<usize> {
        self.translate_to_phys_with_access(vaddr, AccessOp::Store)
            .map(phys_to_virt)
    }

    pub fn translate_to_phys_with_access(&self, vaddr: usize, op: AccessOp) -> Option<u64> {
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
                return Some(res.paddr_page_base + (vaddr & (PAGE_SIZE - 1)) as u64);
            }
            if map.pmarea.start != 0 {
                return Some(map.pmarea.start + (vaddr - map.vmarea.start) as u64);
            }
            return None;
        }

        if map.pmarea.start != 0 {
            Some(map.pmarea.start + (vaddr - map.vmarea.start) as u64)
        } else {
            None
        }
    }

    pub fn translate_to_phys(&self, vaddr: usize) -> Option<u64> {
        let map = self.search_memory_map(vaddr)?;

        if let Some(owner) = &map.owner {
            let page_vaddr = vaddr & !(PAGE_SIZE - 1);
            let page_idx = (page_vaddr - map.vm_start) / PAGE_SIZE;
            let access = crate::object::capability::memory_mapping::AccessKind {
                op: crate::object::capability::memory_mapping::AccessOp::Load,
                vaddr,
                size: Some(1),
            };
            // Lazy private backing must materialize the same private page a
            // userspace load would fault in. Legacy raw-alias output writers
            // must never receive the shared zero source. Eager object mappings
            // keep their physical fallback; fork-COW reads still remain shared.
            if !map.is_shared && map.pmarea.start == 0 && owner.private_fault_requires_copy(&access)
            {
                self.lazy_map_page_with(access).ok()?;
                return self.translate_to_phys(vaddr);
            }
            if let Ok(res) = owner.resolve_fault(&access, page_idx, map.vm_start) {
                return Some(res.paddr_page_base + (vaddr & (PAGE_SIZE - 1)) as u64);
            }
            if map.pmarea.start != 0 {
                return Some(map.pmarea.start + (vaddr - map.vmarea.start) as u64);
            }
            return None;
        }

        if map.pmarea.start != 0 {
            Some(map.pmarea.start + (vaddr - map.vmarea.start) as u64)
        } else {
            None
        }
    }

    pub fn translate_vaddr(&self, vaddr: usize) -> Option<usize> {
        self.translate_to_kva(vaddr)
    }

    pub fn translate_vaddr_to_phys(&self, vaddr: usize) -> Option<u64> {
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
        let mut inner = self.inner.write();
        self.record_inner_writer(WRITE_SITE_MMAP_BASE);
        inner.mmap_base = base;
        inner
            .mmap_occupied_until
            .store(base, core::sync::atomic::Ordering::Relaxed);
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
        let g = self.inner.read();
        Self::find_unmapped_area_in(&g, size, alignment)
    }

    /// Select and insert a free user mapping while holding the same VM lock.
    /// Concurrent mmap callers must not all reserve the same first-fit gap.
    pub fn add_memory_map_anywhere(
        &self,
        mut map: VirtualMemoryMap,
    ) -> Result<usize, &'static str> {
        Self::validate_memory_map(&map)?;
        let size = map.vmarea.size();
        let mut g = self.inner.write();
        let addr = Self::find_unmapped_area_in(&g, size, PAGE_SIZE)
            .ok_or("No unmapped user address range")?;
        map.vmarea = MemoryArea::new(addr, addr + size - 1);
        map.vm_start = addr;
        self.record_inner_writer(WRITE_SITE_ADD_MAP);
        Self::insert_memory_map(&mut g.memmap, map)?;
        g.last_search_cache = None;
        Ok(addr)
    }

    fn find_unmapped_area_in(g: &InnerVmm, size: usize, alignment: usize) -> Option<usize> {
        use core::sync::atomic::Ordering;
        let aligned_size = checked_align_up(size, alignment)?;
        if aligned_size == 0 {
            return None;
        }

        let mut occupied_until = g
            .mmap_occupied_until
            .load(Ordering::Relaxed)
            .max(g.mmap_base);
        let result = (|| {
            let mut search_addr = checked_align_up(occupied_until, alignment)?;

            // Alignment may place the candidate inside a map that began before
            // it. Only extend the hint if there is no gap before that map.
            if let Some((_, prev_map)) = g.memmap.range(..=search_addr).next_back() {
                if prev_map.vmarea.end >= search_addr {
                    let end = prev_map.vmarea.end.checked_add(1)?;
                    if prev_map.vmarea.start <= occupied_until {
                        occupied_until = end;
                    }
                    search_addr = checked_align_up(end, alignment)?;
                }
            }

            for (_, memory_map) in g.memmap.range(search_addr..) {
                let candidate_end = search_addr.checked_add(aligned_size)?;
                if candidate_end <= memory_map.vmarea.start {
                    return (candidate_end <= USER_LOWER_CANONICAL_END).then_some(search_addr);
                }

                if memory_map.vmarea.end >= search_addr {
                    let end = memory_map.vmarea.end.checked_add(1)?;
                    if memory_map.vmarea.start <= occupied_until {
                        occupied_until = end;
                    }
                    search_addr = checked_align_up(end, alignment)?;
                }
            }
            search_addr
                .checked_add(aligned_size)
                .filter(|end| *end <= USER_LOWER_CANONICAL_END)
                .map(|_| search_addr)
        })();
        // Shared readers can only publish covered prefixes while their read
        // guards exclude removals. A slower reader must not regress the hint.
        g.mmap_occupied_until
            .fetch_max(occupied_until, Ordering::Relaxed);
        result
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
        if map.permissions & !map.maximum_permissions != 0 {
            return Err("Mapping permission ceiling exceeded");
        }
        if map.vmarea.start % PAGE_SIZE != 0 || map.vmarea.size() % PAGE_SIZE != 0 {
            return Err("Address or size is not aligned to PAGE_SIZE");
        }
        if map.pmarea.start != 0
            && (map.pmarea.start % PAGE_SIZE as u64 != 0 || map.pmarea.size() % PAGE_SIZE != 0)
        {
            return Err("pmarea is not aligned to PAGE_SIZE");
        }
        Self::validate_mapping_direct_map_alias(&map)?;

        let new_start = map.vmarea.start;
        let new_end = map.vmarea.end;
        let mut overwritten_mappings = Vec::new();
        let mut mappings_to_add = Vec::new();

        let mut g = self.inner.write();
        self.record_inner_writer(WRITE_SITE_ADD_FIXED);
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
                        mapping_origin: existing_map.mapping_origin,
                        maximum_permissions: existing_map.maximum_permissions,
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
                        mapping_origin: existing_map.mapping_origin,
                        maximum_permissions: existing_map.maximum_permissions,
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
                        mapping_origin: existing_map.mapping_origin,
                        maximum_permissions: existing_map.maximum_permissions,
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
        self.record_inner_writer(WRITE_SITE_COALESCE);
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
                        pmarea: PhysicalMemoryArea {
                            start: prev_memory_map.pmarea.start,
                            end: memory_map.pmarea.end,
                        },
                        vm_start: prev_memory_map.vm_start,
                        mapping_origin: prev_memory_map.mapping_origin,
                        maximum_permissions: prev_memory_map.maximum_permissions,
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
        let same_owner = match (&map1.owner, &map2.owner) {
            (None, None) => true,
            (Some(first), Some(second)) => Arc::ptr_eq(first, second),
            _ => false,
        };
        // Coalescing descriptors must retain both allocation identity and
        // owner page indexing, even when distinct mappings are adjacent.
        map1.mapping_origin == map2.mapping_origin
            && map1.vm_start == map2.vm_start
            && same_owner
            && map1.maximum_permissions == map2.maximum_permissions
            && map1.permissions == map2.permissions
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

        let asid = self.get_asid();
        crate::breadcrumb::drop(crate::breadcrumb::VMM_DROP_ENTER, asid as u64, 0);

        let memmap = {
            let mut inner = self.inner.write();
            self.record_inner_writer(WRITE_SITE_DROP);
            core::mem::take(&mut inner.memmap)
        };
        for map in memmap.into_values() {
            if let Some(owner) = map.owner {
                owner.on_unmapped(map.vmarea.start, map.vmarea.size());
            }
        }
        crate::breadcrumb::drop(crate::breadcrumb::VMM_DROP_MAPS_DONE, asid as u64, 0);

        if asid != 0 && is_asid_used(asid) {
            crate::breadcrumb::drop(crate::breadcrumb::VMM_DROP_ASID_BEGIN, asid as u64, 0);
            free_virtual_address_space(asid);
        }
        crate::breadcrumb::drop(crate::breadcrumb::VMM_DROP_DONE, asid as u64, 0);
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
    use crate::object::capability::memory_mapping::{AccessKind, AccessOp};
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
            pmarea: crate::vm::vmem::PhysicalMemoryArea::new(vma.start as u64, vma.end as u64),
            vm_start: vma.start,
            mapping_origin: crate::vm::vmem::MappingOrigin::new(),
            maximum_permissions: usize::MAX,
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
    fn restore_memory_maps_replaces_partial_exec_state_transactionally() {
        let vmm = VirtualMemoryManager::new();
        let vmarea = MemoryArea::new(0x2000, 0x2fff);
        let partial_map = VirtualMemoryMap::new(
            crate::vm::vmem::PhysicalMemoryArea::new(0x3000, 0x3fff),
            vmarea,
            0,
            false,
            None,
        );
        vmm.add_memory_map(partial_map).unwrap();

        let original_map = VirtualMemoryMap::new(
            crate::vm::vmem::PhysicalMemoryArea::new(0x1000, 0x1fff),
            vmarea,
            0,
            false,
            None,
        );
        vmm.restore_memory_maps([original_map]).unwrap();

        assert_eq!(vmm.memmap_len(), 1);
        assert_eq!(vmm.search_memory_map(0x2000).unwrap().pmarea.start, 0x1000);

        let invalid_map = VirtualMemoryMap::new(
            crate::vm::vmem::PhysicalMemoryArea::new(0x5000, 0x5fff),
            MemoryArea::new(0x2800, 0x37ff),
            0,
            false,
            None,
        );
        assert!(vmm.restore_memory_maps([invalid_map]).is_err());
        assert_eq!(vmm.memmap_len(), 1);
        assert_eq!(vmm.search_memory_map(0x2000).unwrap().pmarea.start, 0x1000);
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
            pmarea: crate::vm::vmem::PhysicalMemoryArea::new(vma.start as u64, vma.end as u64),
            vm_start: vma.start,
            mapping_origin: crate::vm::vmem::MappingOrigin::new(),
            maximum_permissions: usize::MAX,
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
            pmarea: crate::vm::vmem::PhysicalMemoryArea::new(vma1.start as u64, vma1.end as u64),
            vm_start: vma1.start,
            mapping_origin: crate::vm::vmem::MappingOrigin::new(),
            maximum_permissions: usize::MAX,
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
            pmarea: crate::vm::vmem::PhysicalMemoryArea::new(vma2.start as u64, vma2.end as u64),
            vm_start: vma2.start,
            mapping_origin: crate::vm::vmem::MappingOrigin::new(),
            maximum_permissions: usize::MAX,
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
        assert_eq!(
            manager.get_mmap_base(),
            crate::environment::DEFAULT_USER_MMAP_BASE
        );
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
    fn test_find_unmapped_area_supports_native_user_addresses() {
        let manager = VirtualMemoryManager::new();

        assert_eq!(
            manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE),
            Some(crate::environment::DEFAULT_USER_MMAP_BASE)
        );

        #[cfg(target_pointer_width = "64")]
        let relocated_base = 0x2_0000_0000;
        #[cfg(target_pointer_width = "32")]
        let relocated_base = 0x6000_0000;
        manager.set_mmap_base(relocated_base);
        assert_eq!(
            manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE),
            Some(relocated_base)
        );
        assert!(manager.find_unmapped_area(0, PAGE_SIZE).is_none());
        assert!(manager.find_unmapped_area(PAGE_SIZE, 0).is_none());
        assert!(manager.find_unmapped_area(PAGE_SIZE, 3).is_none());

        manager.set_mmap_base(usize::MAX - PAGE_SIZE + 1);
        assert!(manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE).is_none());
    }

    fn free_search_test_map(start: usize, pages: usize) -> VirtualMemoryMap {
        VirtualMemoryMap::new(
            crate::vm::vmem::PhysicalMemoryArea { start: 0, end: 0 },
            MemoryArea::new(start, start + pages * PAGE_SIZE - 1),
            0o644,
            false,
            None,
        )
    }

    #[test_case]
    fn free_area_search_reuses_holes_after_long_contiguous_allocation() {
        let manager = VirtualMemoryManager::new();
        let base = 0x50000000;
        manager.set_mmap_base(base);
        for page in 0..1024 {
            assert_eq!(
                manager.add_memory_map_anywhere(free_search_test_map(0, 1)),
                Ok(base + page * PAGE_SIZE)
            );
        }
        assert_eq!(
            manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE),
            Some(base + 1024 * PAGE_SIZE)
        );
        manager
            .remove_memory_map_by_addr(base + 3 * PAGE_SIZE)
            .unwrap();
        assert_eq!(
            manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE),
            Some(base + 3 * PAGE_SIZE)
        );
        // A large request skips this one-page hole, but must not hide it from
        // a subsequent smaller request or change first-fit placement.
        assert_eq!(
            manager.find_unmapped_area(2 * PAGE_SIZE, PAGE_SIZE),
            Some(base + 1024 * PAGE_SIZE)
        );
        assert_eq!(
            manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE),
            Some(base + 3 * PAGE_SIZE)
        );
        manager.remove_memory_map_range(base + 8 * PAGE_SIZE, 3 * PAGE_SIZE);
        assert_eq!(
            manager.find_unmapped_area(2 * PAGE_SIZE, PAGE_SIZE),
            Some(base + 8 * PAGE_SIZE)
        );
    }

    #[test_case]
    fn free_area_search_matches_page_occupancy_after_fixed_maps_and_unmaps() {
        let manager = VirtualMemoryManager::new();
        let base = 0x50000000;
        manager.set_mmap_base(base);
        let mut occupied = [false; 64];
        let mut seed = 0x12345678u32;
        for step in 0..128 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let page = (seed as usize >> 8) % occupied.len();
            let pages = (1 + (seed as usize >> 24) % 4).min(occupied.len() - page);
            let present = step % 3 != 0;
            if present {
                manager
                    .add_memory_map_fixed(free_search_test_map(base + page * PAGE_SIZE, pages))
                    .unwrap();
            } else {
                manager.remove_memory_map_range(base + page * PAGE_SIZE, pages * PAGE_SIZE);
            }
            occupied[page..page + pages].fill(present);
            for size in [1, 3, 7] {
                for alignment in [1, 4, 16] {
                    let reserved = (size + alignment - 1) / alignment * alignment;
                    let expected = (0..128)
                        .find(|candidate| {
                            candidate % alignment == 0
                                && (*candidate..*candidate + reserved)
                                    .all(|index| !occupied.get(index).copied().unwrap_or(false))
                        })
                        .unwrap();
                    assert_eq!(
                        manager.find_unmapped_area(size * PAGE_SIZE, alignment * PAGE_SIZE),
                        Some(base + expected * PAGE_SIZE),
                        "step={} size={} alignment={}",
                        step,
                        size,
                        alignment
                    );
                }
            }
        }
    }

    #[test_case]
    fn free_area_search_tracks_shrink_restore_clear_and_base_changes() {
        let manager = VirtualMemoryManager::new();
        let base = 0x50000000;
        manager.set_mmap_base(base);
        manager
            .add_memory_map(free_search_test_map(base, 8))
            .unwrap();
        assert_eq!(
            manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE),
            Some(base + 8 * PAGE_SIZE)
        );
        manager
            .shrink_memory_map_range(base, 8 * PAGE_SIZE, 4 * PAGE_SIZE)
            .unwrap();
        assert_eq!(
            manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE),
            Some(base + 4 * PAGE_SIZE)
        );
        manager
            .restore_memory_maps([free_search_test_map(base, 2)])
            .unwrap();
        assert_eq!(
            manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE),
            Some(base + 2 * PAGE_SIZE)
        );
        manager.with_memmaps_mut(|maps| maps.clear());
        assert_eq!(manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE), Some(base));
        manager
            .add_memory_map(free_search_test_map(base, 4))
            .unwrap();
        assert_eq!(
            manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE),
            Some(base + 4 * PAGE_SIZE)
        );
        manager.remove_all_memory_maps().for_each(drop);
        assert_eq!(manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE), Some(base));
        manager.set_mmap_base(base - PAGE_SIZE);
        assert_eq!(
            manager.find_unmapped_area(PAGE_SIZE, PAGE_SIZE),
            Some(base - PAGE_SIZE)
        );
    }

    #[test_case]
    fn test_memory_map_coalescing() {
        use crate::environment::PAGE_SIZE;

        // Test memory map coalescing with adjacent compatible maps
        let manager = VirtualMemoryManager::new();

        // Add two adjacent descriptors of the same logical mapping.
        let map1 = VirtualMemoryMap::new(
            crate::vm::vmem::PhysicalMemoryArea {
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
        let mut map2 = VirtualMemoryMap::new(
            crate::vm::vmem::PhysicalMemoryArea {
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

        map2.mapping_origin = map1.mapping_origin;
        map2.vm_start = map1.vm_start;

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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
        let pma = crate::vm::vmem::PhysicalMemoryArea {
            start: dm_phys_start,
            end: dm_phys_start + 0xfff,
        };
        let map = VirtualMemoryMap {
            vmarea: vma,
            pmarea: pma,
            vm_start: vma.start,
            mapping_origin: crate::vm::vmem::MappingOrigin::new(),
            maximum_permissions: usize::MAX,
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
            pmarea: crate::vm::vmem::PhysicalMemoryArea {
                start: 0,
                end: PAGE_SIZE as u64 - 1,
            },
            vm_start: 0x2000,
            mapping_origin: crate::vm::vmem::MappingOrigin::new(),
            maximum_permissions: usize::MAX,
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            crate::vm::vmem::PhysicalMemoryArea {
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
            pmarea: crate::vm::vmem::PhysicalMemoryArea { start: 0, end: 0 },
            vm_start: 0x4000,
            mapping_origin: crate::vm::vmem::MappingOrigin::new(),
            maximum_permissions: usize::MAX,
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
            pmarea: crate::vm::vmem::PhysicalMemoryArea {
                start: 0x8000_0000,
                end: 0x8000_0fff,
            },
            vm_start: 0x5000,
            mapping_origin: crate::vm::vmem::MappingOrigin::new(),
            maximum_permissions: usize::MAX,
            permissions: 0o600,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };
        let removed = manager.add_memory_map_fixed(replacement).unwrap();

        assert_eq!(removed.len(), 1);
        assert_eq!(
            removed[0].pmarea,
            crate::vm::vmem::PhysicalMemoryArea { start: 0, end: 0 }
        );

        let left = manager.search_memory_map(0x4000).unwrap();
        assert_eq!(left.vmarea.start, 0x4000);
        assert_eq!(left.vmarea.end, 0x4fff);
        assert_eq!(
            left.pmarea,
            crate::vm::vmem::PhysicalMemoryArea { start: 0, end: 0 }
        );

        let right = manager.search_memory_map(0x6000).unwrap();
        assert_eq!(right.vmarea.start, 0x6000);
        assert_eq!(right.vmarea.end, 0x7fff);
        assert_eq!(
            right.pmarea,
            crate::vm::vmem::PhysicalMemoryArea { start: 0, end: 0 }
        );
    }

    #[test_case]
    fn test_private_cow_stale_commit_preserves_winning_page() {
        // Given: two fault handlers captured the same owner-backed mapping.
        let manager = VirtualMemoryManager::new();
        let owner: Arc<dyn crate::object::capability::memory_mapping::MemoryMappingOps> =
            Arc::new(AnonymousPageOwner::new());
        let source = VirtualMemoryMap {
            vmarea: MemoryArea {
                start: 0x4000,
                end: 0x6fff,
            },
            pmarea: crate::vm::vmem::PhysicalMemoryArea { start: 0, end: 0 },
            vm_start: 0x4000,
            mapping_origin: crate::vm::vmem::MappingOrigin::new(),
            maximum_permissions: usize::MAX,
            permissions: 0o603,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: Some(Arc::clone(&owner)),
        };
        assert!(manager.add_memory_map(source).is_ok());
        let first_snapshot = manager.search_memory_map(0x5000).unwrap();
        let stale_snapshot = first_snapshot.clone();

        let winning_page = VirtualMemoryMap {
            vmarea: MemoryArea {
                start: 0x5000,
                end: 0x5fff,
            },
            pmarea: crate::vm::vmem::PhysicalMemoryArea {
                start: 0x8000_0000,
                end: 0x8000_0fff,
            },
            vm_start: 0x4000,
            mapping_origin: first_snapshot.mapping_origin,
            maximum_permissions: first_snapshot.maximum_permissions,
            permissions: 0o603,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };

        // When: the first fault commits and the stale fault attempts the same page.
        assert_eq!(
            manager.commit_private_cow_page(&first_snapshot, winning_page),
            Ok(true)
        );
        let losing_page = VirtualMemoryMap {
            vmarea: MemoryArea {
                start: 0x5000,
                end: 0x5fff,
            },
            pmarea: crate::vm::vmem::PhysicalMemoryArea {
                start: 0x9000_0000,
                end: 0x9000_0fff,
            },
            vm_start: 0x4000,
            mapping_origin: stale_snapshot.mapping_origin,
            maximum_permissions: stale_snapshot.maximum_permissions,
            permissions: 0o603,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: None,
        };
        assert_eq!(
            manager.commit_private_cow_page(&stale_snapshot, losing_page),
            Ok(false)
        );

        // Then: the winner remains installed and the source mapping stays split around it.
        let installed = manager.search_memory_map(0x5000).unwrap();
        assert_eq!(installed.pmarea.start, 0x8000_0000);
        assert!(installed.owner.is_none());

        let left = manager.search_memory_map(0x4000).unwrap();
        let right = manager.search_memory_map(0x6000).unwrap();
        assert!(
            left.owner
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &owner))
        );
        assert!(
            right
                .owner
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &owner))
        );
    }

    fn query_test_mapping() -> VirtualMemoryMap {
        VirtualMemoryMap::new(
            crate::vm::vmem::PhysicalMemoryArea { start: 0, end: 0 },
            MemoryArea::new(0x4000, 0x6fff),
            0x0b,
            false,
            Some(Arc::new(AnonymousPageOwner::new())),
        )
    }

    #[test_case]
    fn raw_translation_keeps_eager_owner_physical_fallback() {
        use crate::object::capability::memory_mapping::{MemoryMappingInfo, MemoryMappingOps};
        struct EagerOwner;
        impl MemoryMappingOps for EagerOwner {
            fn get_mapping_info(
                &self,
                _: usize,
                _: usize,
            ) -> Result<MemoryMappingInfo, &'static str> {
                Err("eager backing only")
            }
        }
        let manager = VirtualMemoryManager::new();
        manager
            .add_memory_map(VirtualMemoryMap::new(
                crate::vm::vmem::PhysicalMemoryArea::new(0x8000_0000, 0x8000_0fff),
                MemoryArea::new(0x4000, 0x4fff),
                0x0b,
                false,
                Some(Arc::new(EagerOwner)),
            ))
            .unwrap();
        // A failed owner resolver must not enter an endless fault/translation
        // retry when a directly supplied physical page is already available.
        assert_eq!(manager.translate_to_phys(0x4017), Some(0x8000_0017));
    }

    #[test_case]
    fn mapping_query_stops_at_reused_origin_address() {
        let manager = VirtualMemoryManager::new();
        let original = query_test_mapping();
        let mut prefix = query_test_mapping();
        prefix.vmarea.end = 0x4fff;
        // Reusing the same backing owner must not make this a fragment.
        prefix.owner = original.owner.clone();
        assert_ne!(prefix.mapping_origin, original.mapping_origin);
        manager.add_memory_map(original).unwrap();
        manager.remove_memory_map_range(0x4000, PAGE_SIZE);
        manager.add_memory_map(prefix).unwrap();
        assert_eq!(manager.user_mapping_bounds(0x4100), Some((0x4000, 0x5000)));
        assert_eq!(manager.user_mapping_bounds(0x5100), Some((0x5000, 0x7000)));
        assert_eq!(manager.coalesce_memory_maps(), 0);
    }

    #[test_case]
    fn mapping_query_stops_at_fixed_replacement() {
        let manager = VirtualMemoryManager::new();
        let original = query_test_mapping();
        let mut prefix = query_test_mapping();
        prefix.vmarea.end = 0x4fff;
        manager.add_memory_map(original.clone()).unwrap();
        let removed = manager.add_memory_map_fixed(prefix).unwrap();
        assert_eq!(removed[0].mapping_origin, original.mapping_origin);
        assert_eq!(manager.user_mapping_bounds(0x4100), Some((0x4000, 0x5000)));
        assert_eq!(manager.user_mapping_bounds(0x6100), Some((0x5000, 0x7000)));
    }

    #[test_case]
    fn mapping_query_preserves_full_cow_stack_bounds() {
        let manager = VirtualMemoryManager::new();
        let source = query_test_mapping();
        manager.add_memory_map(source.clone()).unwrap();
        let replacement = VirtualMemoryMap {
            vmarea: MemoryArea::new(0x5000, 0x5fff),
            pmarea: crate::vm::vmem::PhysicalMemoryArea::new(0x8000_0000, 0x8000_0fff),
            owner: None,
            ..source.clone()
        };
        assert_eq!(
            manager.commit_private_cow_page(&source, replacement),
            Ok(true)
        );
        for address in [0x4100, 0x5100, 0x6100] {
            assert_eq!(manager.user_mapping_bounds(address), Some((0x4000, 0x7000)));
            assert_eq!(
                manager.search_memory_map(address).unwrap().mapping_origin,
                source.mapping_origin
            );
        }
    }

    #[test_case]
    fn mapping_query_respects_protection_boundaries() {
        let manager = VirtualMemoryManager::new();
        let source = query_test_mapping();
        manager.add_memory_map(source.clone()).unwrap();
        manager
            .protect_memory_map_range(MemoryArea::new(0x5000, 0x5fff), 1)
            .unwrap();
        assert_eq!(manager.user_mapping_bounds(0x4100), Some((0x4000, 0x5000)));
        assert_eq!(manager.user_mapping_bounds(0x5100), Some((0x5000, 0x6000)));
        assert_eq!(manager.user_mapping_bounds(0x6100), Some((0x6000, 0x7000)));
        manager
            .protect_memory_map_range(MemoryArea::new(0x5000, 0x5fff), 3)
            .unwrap();
        assert_eq!(manager.user_mapping_bounds(0x5100), Some((0x4000, 0x7000)));
        assert_eq!(
            manager.search_memory_map(0x5100).unwrap().mapping_origin,
            source.mapping_origin
        );
    }

    #[test_case]
    fn coalescer_keeps_distinct_mapping_origins() {
        let manager = VirtualMemoryManager::new();
        let first = VirtualMemoryMap::new(
            crate::vm::vmem::PhysicalMemoryArea::new(0x8000_0000, 0x8000_0fff),
            MemoryArea::new(0x4000, 0x4fff),
            0x0b,
            false,
            None,
        );
        let mut second = VirtualMemoryMap::new(
            crate::vm::vmem::PhysicalMemoryArea::new(0x8000_1000, 0x8000_1fff),
            MemoryArea::new(0x5000, 0x5fff),
            0x0b,
            false,
            None,
        );
        // Model an old tail adjacent to a new allocation at its old base.
        second.vm_start = first.vm_start;
        manager.add_memory_map(first).unwrap();
        manager.add_memory_map(second).unwrap();
        assert_eq!(manager.coalesce_memory_maps(), 0);
        assert_eq!(manager.memmap_len(), 2);
        assert_eq!(manager.user_mapping_bounds(0x4100), Some((0x4000, 0x5000)));
    }

    #[test_case]
    fn cow_commit_rejects_recreated_mapping_with_same_owner_and_address() {
        let manager = VirtualMemoryManager::new();
        let original = query_test_mapping();
        manager.add_memory_map(original.clone()).unwrap();
        let mut recreated = query_test_mapping();
        recreated.owner = original.owner.clone();
        manager.add_memory_map_fixed(recreated.clone()).unwrap();
        let replacement = VirtualMemoryMap {
            vmarea: MemoryArea::new(0x5000, 0x5fff),
            pmarea: crate::vm::vmem::PhysicalMemoryArea::new(0x8000_0000, 0x8000_0fff),
            owner: None,
            ..original.clone()
        };
        assert_eq!(
            manager.commit_private_cow_page(&original, replacement),
            Ok(false)
        );
        assert_eq!(
            manager.search_memory_map(0x5000).unwrap().mapping_origin,
            recreated.mapping_origin
        );
    }

    #[test_case]
    fn test_anonymous_owner_fault_past_vma_does_not_extend_mapping() {
        let manager = VirtualMemoryManager::new();
        let owner = Arc::new(AnonymousPageOwner::new());
        let map = VirtualMemoryMap {
            vmarea: MemoryArea {
                start: 0x4000,
                end: 0x4fff,
            },
            pmarea: crate::vm::vmem::PhysicalMemoryArea { start: 0, end: 0 },
            vm_start: 0x4000,
            mapping_origin: crate::vm::vmem::MappingOrigin::new(),
            maximum_permissions: usize::MAX,
            permissions: 0x0b,
            is_shared: false,
            memory_attribute: crate::vm::vmem::MemoryAttribute::Normal,
            owner: Some(owner),
        };
        manager.add_memory_map(map).unwrap();

        let fault_addr = usize::MAX - 7;
        let result = manager.lazy_map_page_with(AccessKind {
            op: AccessOp::Load,
            vaddr: fault_addr,
            size: None,
        });

        assert_eq!(
            result,
            Err("No extendable memory mapping found for virtual address")
        );
        let map = manager.get_memory_map_by_addr(0x4000).unwrap();
        assert_eq!(map.vmarea.end, 0x4fff);
        assert!(manager.search_memory_map(fault_addr).is_none());
    }
    #[test_case]
    fn permission_ceiling_survives_relocation_growth_and_prevents_coalescing() {
        use crate::object::capability::memory_mapping::anon_owner::AnonymousZeroPageOwner;
        let manager = VirtualMemoryManager::new();
        let base = 0x50000000;
        let mut map = VirtualMemoryMap::new(crate::vm::vmem::PhysicalMemoryArea::new(0, 0),
            MemoryArea::new(base, base + PAGE_SIZE - 1), 11, false,
            Some(Arc::new(AnonymousZeroPageOwner::new())));
        map.mapping_origin = crate::vm::vmem::MappingOrigin::new_anonymous();
        map.maximum_permissions = !4;
        manager.add_memory_map(map).unwrap();
        let moved = base + 4 * PAGE_SIZE;
        manager.relocate_private_memory_map_range(base, moved, PAGE_SIZE).unwrap();
        assert_eq!(manager.grow_private_anonymous_range(moved, PAGE_SIZE, 2 * PAGE_SIZE, false), Ok(moved));
        assert!(manager.protect_linux_memory_map_range(MemoryArea::new(moved, moved + 2 * PAGE_SIZE - 1), 5).is_err());
        for page in 0..2 { assert_eq!(manager.search_memory_map(moved + page * PAGE_SIZE).unwrap().maximum_permissions & 4, 0); }
        let first = manager.search_memory_map(moved).unwrap();
        let mut second = first.clone();
        second.maximum_permissions = usize::MAX;
        assert!(!VirtualMemoryManager::can_merge_memory_maps(&first, &second));
    }

}
