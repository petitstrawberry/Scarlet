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
const MAP_SHARED: usize = 0x01;
const MAP_ANONYMOUS: usize = 0x20;

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
    let (paddr, obj_permissions, is_shared) = match memory_mappable.get_mapping_info(offset, length) {
        Ok(info) => info,
        Err(_) => return usize::MAX,
    };

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

    // Create virtual memory map with weak reference to the object
    let owner = kernel_obj.as_memory_mappable_weak();
    let vm_map = VirtualMemoryMap::new(pmarea, vmarea, final_permissions, is_shared, owner);

    // Add the mapping to VM manager
    match task.vm_manager.add_memory_map_fixed(vm_map) {
        Ok(_removed_mappings) => {
            // Notify the object that mapping was created
            memory_mappable.on_mapped(final_vaddr, paddr, aligned_length, offset);
            final_vaddr
        }
        Err(_) => usize::MAX,
    }
}

/// Handle anonymous memory mapping
fn handle_anonymous_mapping(
    task: &mut crate::task::Task,
    mut vaddr: usize,
    aligned_length: usize,
    num_pages: usize,
    prot: usize,
    flags: usize,
) -> usize {
    // For anonymous mappings, allocate physical memory directly
    let pages = allocate_raw_pages(num_pages);
    let pages_ptr = pages as usize;

    // If vaddr is 0, kernel chooses the address
    if vaddr == 0 {
        // Use VMManager's find_unmapped_area for consistent virtual address allocation
        match task.vm_manager.find_unmapped_area(aligned_length, PAGE_SIZE) {
            Some(addr) => vaddr = addr,
            None => return usize::MAX, // No suitable address found
        }
    } else {
        // Validate the requested address is page-aligned
        if vaddr % PAGE_SIZE != 0 {
            return usize::MAX;
        }
    }

    // Convert protection flags to kernel permissions
    let mut permissions = 0;
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
    let is_shared = (flags & MAP_SHARED) != 0;
    let vm_map = VirtualMemoryMap::new(pmarea, vmarea, permissions, is_shared, None); // Anonymous mappings have no owner

    // Use add_memory_map_fixed for both FIXED and non-FIXED mappings to handle overlaps consistently
    match task.vm_manager.add_memory_map_fixed(vm_map) {
        Ok(removed_mappings) => {
            // Process removed mappings and free their managed pages
            for removed_map in removed_mappings {
                // Remove from anonymous mappings tracking
                task.remove_anonymous_mapping(removed_map.vmarea.start);
                
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
            
            // Track this new anonymous mapping
            task.add_anonymous_mapping(vaddr);
            
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

    // Check if this is an anonymous mapping
    let is_anonymous = task.is_anonymous_mapping(vaddr);

    if is_anonymous {
        // Handle anonymous mapping unmapping
        if let Some(removed_map) = task.vm_manager.remove_memory_map_by_addr(vaddr) {
            // Remove managed pages only for private mappings
            // Shared mappings should not have their physical pages freed here
            // as they might be used by other processes
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
            
            // Remove from our tracking
            task.remove_anonymous_mapping(vaddr);
            0
        } else {
            usize::MAX
        }
    } else {
        // Handle object-based mapping unmapping
        if let Some(removed_map) = task.vm_manager.remove_memory_map_by_addr(vaddr) {
            // Notify the object owner if available
            if let Some(owner_weak) = &removed_map.owner {
                if let Some(owner) = owner_weak.upgrade() {
                    owner.on_unmapped(vaddr, length);
                }
                // If the object is no longer available, we just proceed with VM cleanup
            }
            
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
            
            0
        } else {
            usize::MAX // No mapping found at this address
        }
    }
}