//! System calls for MemoryMappingOps capability
//! 
//! This module implements system calls for memory mapping operations.
//! ANONYMOUS mappings are handled directly in the syscall for efficiency,
//! while all other mappings (including FIXED) are delegated to KernelObjects
//! with MemoryMappingOps capability.

use crate::arch::Trapframe;
use crate::task::mytask;
use crate::vm::vmem::{MemoryArea, VirtualMemoryMap};
use crate::environment::PAGE_SIZE;
use crate::mem::page::allocate_raw_pages;
use alloc::boxed::Box;

// Memory mapping flags (MAP_*)
#[allow(dead_code)]
const MAP_SHARED: usize = 0x01;
const MAP_ANONYMOUS: usize = 0x20;
const MAP_PRIVATE: usize = 0x02;

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

    // Get mapping information from the object
    let (paddr, obj_permissions, _obj_is_shared) = match memory_mappable.get_mapping_info(offset, length) {
        Ok(info) => info,
        Err(_) => return usize::MAX,
    };

    // Determine is_shared from flags (MAP_SHARED controls sharing semantics)
    let is_shared = (flags & MAP_SHARED) != 0;

    // Determine final address
    let final_vaddr = if vaddr == 0 {
        match task.vm_manager.find_unmapped_area(aligned_length, PAGE_SIZE) {
            Some(addr) => addr,
            None => return usize::MAX,
        }
    } else {
        if vaddr % PAGE_SIZE != 0 {
            return usize::MAX;
        }
        vaddr
    };

    // Create memory areas
    let vmarea = MemoryArea::new(final_vaddr, final_vaddr + aligned_length - 1);
    let pmarea = MemoryArea::new(paddr, paddr + aligned_length - 1);

    // Combine object permissions with requested permissions
    let final_permissions = obj_permissions & {
        let mut perm = 0;
        if (prot & PROT_READ) != 0 { perm |= 0x1; }
        if (prot & PROT_WRITE) != 0 { perm |= 0x2; }
        if (prot & PROT_EXEC) != 0 { perm |= 0x4; }
        perm
    } | 0x08; // Access from user space

    // If this is a private file-backed mapping, allocate private pages and copy
    let is_map_private_flag = (flags & MAP_PRIVATE) != 0;
    if is_map_private_flag && !is_shared {
        // Allocate private pages
        let pages = allocate_raw_pages(num_pages);
        let pages_ptr = pages as usize;
        let private_pmarea = MemoryArea::new(pages_ptr, pages_ptr + aligned_length - 1);

        let vm_map = VirtualMemoryMap::new(private_pmarea, vmarea, final_permissions, false, None);

        match task.vm_manager.add_memory_map_fixed(vm_map) {
            Ok(removed_mappings) => {
                // For private mappings the new mapping uses private pages and the object
                // does not own those pages; avoid calling on_mapped for the object.

                // Notify owners of removed maps (only for shared mappings)
                for removed_map in &removed_mappings {
                    if removed_map.is_shared {
                        if let Some(owner_weak) = &removed_map.owner {
                            if let Some(owner) = owner_weak.upgrade() {
                                owner.on_unmapped(removed_map.vmarea.start, removed_map.vmarea.size());
                            }
                        }
                    }
                }

                // Clean up removed managed pages
                for removed_map in removed_mappings {
                    if !removed_map.is_shared {
                        let mapping_start = removed_map.vmarea.start;
                        let mapping_end = removed_map.vmarea.end;
                        let num_removed_pages = (mapping_end - mapping_start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;
                        for i in 0..num_removed_pages {
                            let page_vaddr = mapping_start + i * PAGE_SIZE;
                            if let Some(_managed_page) = task.remove_managed_page(page_vaddr) {
                                // freed when dropped
                            }
                        }
                    }
                }

                // Copy contents from object paddr to private pages
                for i in 0..num_pages {
                    let src = (paddr + i * PAGE_SIZE) as *const u8;
                    let dst_page = unsafe { (pages as *mut crate::mem::page::Page).add(i) } as *mut u8;
                    unsafe { core::ptr::copy_nonoverlapping(src, dst_page, PAGE_SIZE); }
                }

                // Add managed pages to task
                for i in 0..num_pages {
                    let page_vaddr = final_vaddr + i * crate::environment::PAGE_SIZE;
                    let page_ptr = unsafe { (pages as *mut crate::mem::page::Page).add(i) };
                    task.add_managed_page(crate::task::ManagedPage {
                        vaddr: page_vaddr,
                        page: unsafe { Box::from_raw(page_ptr) },
                    });
                }

                return final_vaddr;
            }
            Err(_) => {
                // Free allocated pages
                crate::mem::page::free_raw_pages(pages, num_pages);
                return usize::MAX;
            }
        }
    }

    // Create virtual memory map with weak reference to the object
    let owner = kernel_obj.as_memory_mappable_weak();
    let vm_map = VirtualMemoryMap::new(pmarea, vmarea, final_permissions, is_shared, owner);

    // Add the mapping to VM manager
    match task.vm_manager.add_memory_map_fixed(vm_map) {
        Ok(removed_mappings) => {
            // Notify the object that mapping was created
            memory_mappable.on_mapped(final_vaddr, paddr, aligned_length, offset);

            // First, notify object owners about removed mappings
            for removed_map in &removed_mappings {
                if removed_map.is_shared {
                    if let Some(owner_weak) = &removed_map.owner {
                        if let Some(owner) = owner_weak.upgrade() {
                            owner.on_unmapped(removed_map.vmarea.start, removed_map.vmarea.size());
                        }
                    }
                }
            }

            // Then, handle managed page cleanup (MMU cleanup is already handled by VmManager.add_memory_map_fixed)
            for removed_map in removed_mappings {
                // Remove managed pages only for private mappings
                if !removed_map.is_shared {
                    let mapping_start = removed_map.vmarea.start;
                    let mapping_end = removed_map.vmarea.end;
                    let num_pages = (mapping_end - mapping_start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;
                    
                    for i in 0..num_pages {
                        let page_vaddr = mapping_start + i * PAGE_SIZE;
                        if let Some(_managed_page) = task.remove_managed_page(page_vaddr) {
                            // The managed page is automatically freed when dropped
                        }
                    }
                }
            }

            final_vaddr
        }
        Err(_) => usize::MAX,
    }
}

/// Handle anonymous memory mapping
fn handle_anonymous_mapping(
    task: &mut crate::task::Task,
    vaddr: usize,
    aligned_length: usize,
    num_pages: usize,
    prot: usize,
    flags: usize,
) -> usize {
    // For anonymous mappings, decide shared/private based on flags
    let is_shared = (flags & MAP_SHARED) != 0;

    // For anonymous mappings, allocate physical memory directly
    let pages = allocate_raw_pages(num_pages);
    let pages_ptr = pages as usize;

    // Convert protection flags to kernel permissions
    let mut permissions = 0x08; // Access from user space
    if (prot & PROT_READ) != 0 {
        permissions |= 0x1; // Readable
    }
    if (prot & PROT_WRITE) != 0 {
        permissions |= 0x2; // Writable
    }
    if (prot & PROT_EXEC) != 0 {
        permissions |= 0x4; // Executable
    }

    // Create memory areas
    let vmarea = MemoryArea::new(vaddr, vaddr + aligned_length - 1);
    let pmarea = MemoryArea::new(pages_ptr, pages_ptr + aligned_length - 1);
    
    // Create virtual memory map  
    let vm_map = VirtualMemoryMap::new(pmarea, vmarea, permissions, is_shared, None); // Anonymous mappings have no owner

    // Use add_memory_map_fixed for both FIXED and non-FIXED mappings to handle overlaps consistently
    match task.vm_manager.add_memory_map_fixed(vm_map) {
        Ok(removed_mappings) => {
            // First, process notifications for object owners
            for removed_map in &removed_mappings {
                if removed_map.is_shared {
                    if let Some(owner_weak) = &removed_map.owner {
                        if let Some(owner) = owner_weak.upgrade() {
                            owner.on_unmapped(removed_map.vmarea.start, removed_map.vmarea.size());
                        }
                    }
                }
            }
            
            // Then, handle managed page cleanup (MMU cleanup is already handled by VmManager.add_memory_map_fixed)
            for removed_map in removed_mappings {
                // Remove managed pages only for private mappings
                if !removed_map.is_shared {
                    let mapping_start = removed_map.vmarea.start;
                    let mapping_end = removed_map.vmarea.end;
                    let num_removed_pages = (mapping_end - mapping_start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;
                    
                    for i in 0..num_removed_pages {
                        let page_vaddr = mapping_start + i * PAGE_SIZE;
                        if let Some(_managed_page) = task.remove_managed_page(page_vaddr) {
                            // The managed page is automatically freed when dropped
                        }
                    }
                }
            }
            
            // Add managed pages for the new anonymous mapping
            for i in 0..num_pages {
                let page_vaddr = vaddr + i * crate::environment::PAGE_SIZE;
                let page_ptr = unsafe { (pages as *mut crate::mem::page::Page).add(i) };
                task.add_managed_page(crate::task::ManagedPage {
                    vaddr: page_vaddr,
                    page: unsafe { Box::from_raw(page_ptr) },
                });
            }
            
            vaddr
        }
        Err(_) => usize::MAX,
    }
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

    // Remove the mapping regardless of whether it's anonymous or object-based
    if let Some(removed_map) = task.vm_manager.remove_memory_map_by_addr(vaddr) {
        // Notify the object owner if available (for object-based mappings)
        if let Some(owner_weak) = &removed_map.owner {
            if removed_map.is_shared {
                if let Some(owner) = owner_weak.upgrade() {
                    owner.on_unmapped(vaddr, length);
                }
            }
        }
        
        // Remove managed pages only for private mappings
        // Shared mappings should not have their physical pages freed here
        // as they might be used by other processes
        // (MMU cleanup is already handled by VmManager.remove_memory_map_by_addr)
        if !removed_map.is_shared {
            let mapping_start = removed_map.vmarea.start;
            let mapping_end = removed_map.vmarea.end;
            let num_pages = (mapping_end - mapping_start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;
            
            for i in 0..num_pages {
                let page_vaddr = mapping_start + i * PAGE_SIZE;
                if let Some(_managed_page) = task.remove_managed_page(page_vaddr) {
                    // The managed page is automatically freed when dropped
                }
            }
        }
        
        0
    } else {
        usize::MAX // No mapping found at this address
    }
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