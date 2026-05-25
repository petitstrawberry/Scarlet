//! System calls for MemoryMappingOps capability
//!
//! This module implements system calls for memory mapping operations.
//! ANONYMOUS mappings are handled directly in the syscall for efficiency,
//! while all other mappings (including FIXED) are delegated to KernelObjects
//! with MemoryMappingOps capability.

use crate::arch::Trapframe;
use crate::environment::PAGE_SIZE;
use crate::object::capability::memory_mapping::anon_owner::AnonymousPageOwner;
use crate::task::mytask;
use crate::vm::vmem::{MemoryArea, VirtualMemoryMap};
use alloc::sync::Arc;
use alloc::vec::Vec;

pub(crate) fn reclaim_private_removed_mapping(
    task: &crate::task::Task,
    removed_map: &VirtualMemoryMap,
) {
    if removed_map.is_shared {
        return;
    }

    if let Some(owner) = &removed_map.owner {
        let start_page_idx = (removed_map.vmarea.start - removed_map.vm_start) / PAGE_SIZE;
        let page_count =
            (removed_map.vmarea.end - removed_map.vmarea.start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;
        owner.release_pages(start_page_idx, page_count);
        return;
    }

    let pm_start = removed_map.pmarea.start;
    let pm_end = removed_map.pmarea.end;
    if pm_start == 0 && pm_end == 0 {
        return;
    }

    {
        let mut allocs = task.page_allocations.write();
        let mut retained = Vec::new();
        for alloc in allocs.drain(..) {
            let alloc_start = alloc.as_paddr();
            let alloc_end = alloc_start + alloc.len() * PAGE_SIZE - 1;

            if alloc_start >= pm_start && alloc_end <= pm_end {
                drop(alloc);
            } else {
                retained.push(alloc);
            }
        }
        *allocs = retained;
    }

    {
        let mut task_pages_allocs = task.task_pages.write();
        for alloc in task_pages_allocs.iter_mut() {
            let _ = alloc.reclaim_paddr_range(pm_start, pm_end);
        }
        task_pages_allocs.retain(|alloc| !alloc.is_empty());
    }
}

// Memory mapping flags (MAP_*)
#[allow(dead_code)]
const MAP_SHARED: usize = 0x01;
const MAP_ANONYMOUS: usize = 0x20;
const MAP_PRIVATE: usize = 0x02;
const MAP_FIXED: usize = 0x10;

// Protection flags (PROT_*)
const PROT_READ: usize = 0x1;
const PROT_WRITE: usize = 0x2;
const PROT_EXEC: usize = 0x4;

/// System call for memory mapping a KernelObject with MemoryMappingOps capability
/// or creating anonymous mappings
///
/// # Arguments
/// - handle: Handle to the KernelObject (must support MemoryMappingOps) - ignored for ANONYMOUS
/// - vaddr: Virtual address where to map (0 means kernel chooses)
/// - length: Length of the mapping in bytes
/// - prot: Protection flags (PROT_READ, PROT_WRITE, PROT_EXEC)
/// - flags: Mapping flags (MAP_SHARED, MAP_PRIVATE, MAP_FIXED, MAP_ANONYMOUS, etc.)
/// - offset: Offset within the object to start mapping from (ignored for ANONYMOUS)
///
/// # Returns
/// - On success: virtual address of the mapping
/// - On error: usize::MAX
///
/// # Design
/// - ANONYMOUS mappings are handled entirely within this syscall
/// - All other mappings (including FIXED) are delegated to the KernelObject's MemoryMappingOps
pub fn sys_memory_map(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let handle = trapframe.get_arg(0) as u32;
    let vaddr = trapframe.get_arg(1) as usize;
    let length = trapframe.get_arg(2) as usize;
    let prot = trapframe.get_arg(3) as usize;
    let flags = trapframe.get_arg(4) as usize;
    let offset = trapframe.get_arg(5) as usize;

    // Increment PC to avoid infinite loop if mmap fails
    trapframe.increment_pc_next(task);

    // Input validation
    if length == 0 {
        return usize::MAX;
    }

    // Round up length to page boundary
    let aligned_length = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let num_pages = aligned_length / PAGE_SIZE;

    // Handle ANONYMOUS mappings specially - these are handled entirely in the syscall
    if (flags & MAP_ANONYMOUS) != 0 {
        return handle_anonymous_mapping(task, vaddr, aligned_length, num_pages, prot, flags);
    }

    // All other mappings are handled through the new MemoryMappingOps design
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid handle
    };

    // Check if object supports MemoryMappingOps
    let memory_mappable = match kernel_obj.as_memory_mappable() {
        Some(mappable) => mappable,
        None => return usize::MAX, // Object doesn't support memory mapping operations
    };

    // Check if the object supports mmap
    if !memory_mappable.supports_mmap() {
        return usize::MAX;
    }

    let is_shared = (flags & MAP_SHARED) != 0;
    let is_map_fixed = (flags & MAP_FIXED) != 0;
    let is_map_private_flag = (flags & MAP_PRIVATE) != 0;

    // Determine final address
    let final_vaddr = if vaddr == 0 {
        match task
            .vm_manager
            .find_unmapped_area(aligned_length, PAGE_SIZE)
        {
            Some(addr) => addr,
            None => return usize::MAX,
        }
    } else {
        if vaddr % PAGE_SIZE != 0 {
            return usize::MAX;
        }
        vaddr
    };

    let mut prot_perm = 0;
    if (prot & PROT_READ) != 0 {
        prot_perm |= 0x1;
    }
    if (prot & PROT_WRITE) != 0 {
        prot_perm |= 0x2;
    }
    if (prot & PROT_EXEC) != 0 {
        prot_perm |= 0x4;
    }
    prot_perm |= 0x08;

    if is_map_private_flag && !is_shared {
        let owner = match task
            .handle_table
            .get_arc_clone(handle)
            .and_then(|obj| obj.as_memory_mappable_arc())
        {
            Some(owner) => owner,
            None => return usize::MAX,
        };
        let vm_map = VirtualMemoryMap {
            pmarea: MemoryArea { start: 0, end: 0 },
            vmarea: MemoryArea::new(final_vaddr, final_vaddr + aligned_length - 1),
            vm_start: final_vaddr,
            permissions: prot_perm,
            is_shared: false,
            owner: Some(owner),
        };

        let removed_mappings = if is_map_fixed {
            task.vm_manager.add_memory_map_fixed(vm_map)
        } else {
            task.vm_manager.add_memory_map(vm_map).map(|_| Vec::new())
        };

        let removed_mappings = match removed_mappings {
            Ok(rm) => rm,
            Err(_) => return usize::MAX,
        };

        for removed_map in &removed_mappings {
            if let Some(owner) = &removed_map.owner {
                owner.on_unmapped(removed_map.vmarea.start, removed_map.vmarea.size());
            }
        }
        for removed_map in removed_mappings {
            reclaim_private_removed_mapping(task, &removed_map);
        }

        memory_mappable.on_mapped(final_vaddr, 0, aligned_length, offset);
        return final_vaddr;
    }

    // Shared path: need get_mapping_info_with for pmarea and permissions
    let (paddr, obj_permissions, _obj_is_shared) =
        match memory_mappable.get_mapping_info_with(offset, aligned_length, is_shared) {
            Ok(info) => info,
            Err(_) => return usize::MAX,
        };

    let vmarea = MemoryArea::new(final_vaddr, final_vaddr + aligned_length - 1);
    let pmarea = MemoryArea::new(paddr, paddr + aligned_length - 1);

    let final_permissions = obj_permissions & {
        let mut perm = 0;
        if (prot & PROT_READ) != 0 {
            perm |= 0x1;
        }
        if (prot & PROT_WRITE) != 0 {
            perm |= 0x2;
        }
        if (prot & PROT_EXEC) != 0 {
            perm |= 0x4;
        }
        perm
    } | 0x08;

    let owner = task
        .handle_table
        .get_arc_clone(handle)
        .and_then(|obj| obj.as_memory_mappable_arc());
    let vm_map = VirtualMemoryMap::new(pmarea, vmarea, final_permissions, is_shared, owner);

    if !is_map_fixed {
        if task.vm_manager.add_memory_map(vm_map.clone()).is_err() {
            let chosen_vaddr = match task
                .vm_manager
                .find_unmapped_area(aligned_length, PAGE_SIZE)
            {
                Some(addr) => addr,
                None => return usize::MAX,
            };
            let vmarea = MemoryArea::new(chosen_vaddr, chosen_vaddr + aligned_length - 1);
            let pmarea = MemoryArea::new(paddr, paddr + aligned_length - 1);
            let owner = task
                .handle_table
                .get_arc_clone(handle)
                .and_then(|obj| obj.as_memory_mappable_arc());
            let retry_map =
                VirtualMemoryMap::new(pmarea, vmarea, final_permissions, is_shared, owner);
            if task.vm_manager.add_memory_map(retry_map).is_err() {
                return usize::MAX;
            }

            memory_mappable.on_mapped(chosen_vaddr, paddr, aligned_length, offset);
            return chosen_vaddr;
        }

        memory_mappable.on_mapped(final_vaddr, paddr, aligned_length, offset);
        return final_vaddr;
    }

    match task.vm_manager.add_memory_map_fixed(vm_map) {
        Ok(removed_mappings) => {
            memory_mappable.on_mapped(final_vaddr, paddr, aligned_length, offset);

            for removed_map in &removed_mappings {
                if removed_map.is_shared {
                    if let Some(owner) = &removed_map.owner {
                        owner.on_unmapped(removed_map.vmarea.start, removed_map.vmarea.size());
                    }
                }
            }

            for removed_map in removed_mappings {
                reclaim_private_removed_mapping(task, &removed_map);
            }

            final_vaddr
        }
        Err(_) => usize::MAX,
    }
}

fn handle_anonymous_mapping(
    task: &crate::task::Task,
    vaddr: usize,
    aligned_length: usize,
    num_pages: usize,
    prot: usize,
    flags: usize,
) -> usize {
    let is_shared = (flags & MAP_SHARED) != 0;
    let is_map_fixed = (flags & MAP_FIXED) != 0;

    // Determine the final virtual address for the mapping.
    // If vaddr is 0, find an unmapped area; otherwise use the hint or fixed address.
    let final_vaddr = if vaddr == 0 {
        match task
            .vm_manager
            .find_unmapped_area(aligned_length, PAGE_SIZE)
        {
            Some(addr) => addr,
            None => return usize::MAX,
        }
    } else if vaddr % PAGE_SIZE != 0 {
        return usize::MAX;
    } else if !is_map_fixed {
        // Non-fixed: treat vaddr as a hint; pick a fresh area if it overlaps.
        let vaddr_end = vaddr + aligned_length - 1;
        let has_overlap = task.vm_manager.with_memmaps(|mm| {
            mm.values()
                .any(|map| !(vaddr_end < map.vmarea.start || vaddr > map.vmarea.end))
        });
        if has_overlap {
            match task
                .vm_manager
                .find_unmapped_area(aligned_length, PAGE_SIZE)
            {
                Some(addr) => addr,
                None => return usize::MAX,
            }
        } else {
            vaddr
        }
    } else {
        vaddr
    };

    let mut permissions = 0x08;
    if (prot & PROT_READ) != 0 {
        permissions |= 0x1;
    }
    if (prot & PROT_WRITE) != 0 {
        permissions |= 0x2;
    }
    if (prot & PROT_EXEC) != 0 {
        permissions |= 0x4;
    }

    let owner: Arc<dyn crate::object::capability::memory_mapping::MemoryMappingOps> =
        Arc::new(AnonymousPageOwner::new());

    let vmarea = MemoryArea::new(final_vaddr, final_vaddr + aligned_length - 1);
    let vm_map = VirtualMemoryMap {
        pmarea: MemoryArea { start: 0, end: 0 },
        vmarea,
        vm_start: final_vaddr,
        permissions,
        is_shared,
        owner: Some(owner),
    };

    let removed_mappings = match task.vm_manager.add_memory_map_fixed(vm_map) {
        Ok(removed) => removed,
        Err(_) => return usize::MAX,
    };

    for removed_map in &removed_mappings {
        if removed_map.is_shared {
            if let Some(owner) = &removed_map.owner {
                owner.on_unmapped(removed_map.vmarea.start, removed_map.vmarea.size());
            }
        }
    }
    for removed_map in removed_mappings {
        reclaim_private_removed_mapping(task, &removed_map);
    }
    final_vaddr
}

/// System call for unmapping memory from a KernelObject or anonymous mapping
///
/// # Arguments
/// - vaddr: Virtual address of the mapping to unmap
/// - length: Length of the mapping to unmap
///
/// # Returns
/// - On success: 0
/// - On error: usize::MAX
pub fn sys_memory_unmap(trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let vaddr = trapframe.get_arg(0) as usize;
    let length = trapframe.get_arg(1) as usize;

    // Increment PC to avoid infinite loop if munmap fails
    trapframe.increment_pc_next(task);

    // Input validation
    if length == 0 || vaddr % PAGE_SIZE != 0 {
        return usize::MAX;
    }

    // Remove the mapping range, splitting existing mappings if necessary
    let removed_maps = task.vm_manager.remove_memory_map_range(vaddr, length);

    if removed_maps.is_empty() {
        return usize::MAX; // No mappings found in the specified range
    }

    // Notify the object owners and clean up page allocations
    for removed_map in &removed_maps {
        if let Some(owner) = &removed_map.owner {
            owner.on_unmapped(removed_map.vmarea.start, removed_map.vmarea.size());
        }

        reclaim_private_removed_mapping(task, removed_map);
    }

    0
}

// TODO: Migrate object-backed MAP_PRIVATE mappings to delayed Copy-On-Write (COW).
// Motivation:
// - Currently MAP_PRIVATE file-backed mappings perform an eager (immediate) copy of
//   the mapped region at mmap time. This can be wasteful for large mappings or when
//   the mapping is only read by the process. Delayed COW preserves memory and CPU
//   by copying only on the first write to a page.
//
// High-level plan (implementation checklist):
// 1) Syscall layer: when a user requests MAP_PRIVATE for an object-backed mapping,
//    set a `cow` flag on the VirtualMemoryMap and do NOT perform an immediate copy.
//    - Ensure the mapping is installed with write permission cleared so stores trap.
//    - Preserve the mapping owner (object) for read access until pages are copied.
//
// 2) VM representation: add/ensure a boolean `cow` field on VirtualMemoryMap to mark
//    that the mapping uses copy-on-write semantics.
//
// 3) Exception/Trap handling: on store-page-faults, detect whether the faulting
//    virtual address belongs to a mapping with cow == true. If so, invoke a per-page
//    COW handler instead of the generic lazy mapping logic.
//
// 4) Task::handle_cow_page: implement a handler that:
//    - Allocates a new physical page for the faulting virtual page.
//    - Copies the contents from the original backing paddr (via the owner object
//      or pmarea) to the newly allocated page.
//    - Replaces only the single faulting page in the VM map by inserting a fixed
//      one-page VirtualMemoryMap (owner = None) for that vaddr and maps it immediately
//      (e.g., vm_manager.add_memory_map_fixed + vm_manager.lazy_map_page).
//    - Registers the new page as a managed page of the current Task (so it will be
//      freed on exit).
//
// 5) Fork/clone semantics: ensure that when a Task is cloned/forked, the child and parent
//    share the same physical pages (do not eagerly copy) and the `cow` flag is preserved
//    on the mapping entries so that subsequent writes by either side trigger COW.
//    - Ensure managed_pages bookkeeping remains correct (only private copies are managed
//      by the process that holds them).
//
// 6) Tests and validation:
//    - Add unit/integration tests that map the same file in two tasks with MAP_PRIVATE,
//      then write from one task and assert the other still sees original content.
//    - Add tests for fork/clone + MAP_PRIVATE behavior.
//    - Add tests for corner cases (partial-page offsets, overlapping mappings, munmap
//      of pages that have been COW'ed).
//
// 7) Documentation: update rustdoc and design documentation to describe the COW
//    semantics, the role of the `cow` flag, and the guarantees provided (ownership,
//    notification behavior, and lifecycle of managed pages).
//
// Acceptance criteria:
// - MAP_PRIVATE mappings are created without eager copying (vm_manager installs mapping
//   with cow=true and write cleared).
// - On first write to a page, only that page is copied and the writer gets a private
//   writable page while others continue sharing the original page.
// - All added tests pass in the dev environment (cargo make test) and resource leaks
//   (pages) are not introduced.
//
// Notes & constraints:
// - Some object types (e.g., device MMIO) cannot be safely COW'ed; sys_memory_map must
//   detect such objects via supports_mmap / get_mapping_info and either fall back to
//   eager-copy, reject the mapping, or require special flags. Document these cases.
// - This change requires careful updates to trap handling and the Task-managed page
//   bookkeeping; perform the work incrementally and add tests at each step.
