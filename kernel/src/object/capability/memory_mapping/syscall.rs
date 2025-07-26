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
use alloc::collections::BTreeSet;
use core::sync::atomic::{AtomicUsize, Ordering};

// Memory mapping flags (MAP_*)
const MAP_SHARED: usize = 0x01;
const MAP_PRIVATE: usize = 0x02;
const MAP_FIXED: usize = 0x10;
const MAP_ANONYMOUS: usize = 0x20;

// Protection flags (PROT_*)
const PROT_READ: usize = 0x1;
const PROT_WRITE: usize = 0x2;
const PROT_EXEC: usize = 0x4;
const PROT_NONE: usize = 0x0;

// Global storage for anonymous mappings tracking
// This is a simple approach for now - in a more robust implementation,
// this would be per-task or in the task structure itself
static ANONYMOUS_MAPPINGS: spin::Mutex<BTreeSet<usize>> = spin::Mutex::new(BTreeSet::new());
static NEXT_ANONYMOUS_VADDR: AtomicUsize = AtomicUsize::new(0x40000000); // Start at 1GB

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
    let mut vaddr = trapframe.get_arg(1) as usize;
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

    // All other mappings (including FIXED) are delegated to KernelObject
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return usize::MAX, // Invalid handle
    };

    // Check if object supports MemoryMappingOps
    let memory_mappable = match kernel_obj.as_memory_mappable() {
        Some(mappable) => mappable,
        None => return usize::MAX, // Object doesn't support memory mapping operations
    };

    // Perform mmap operation - let the object handle all aspects including FIXED mappings
    match memory_mappable.mmap(vaddr, length, prot, flags, offset) {
        Ok(mapped_addr) => mapped_addr,
        Err(_) => usize::MAX, // Mmap error
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
        // Find a suitable virtual address
        vaddr = NEXT_ANONYMOUS_VADDR.fetch_add(aligned_length, Ordering::SeqCst);
        // Align to page boundary
        vaddr = (vaddr + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
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
    let vm_map = VirtualMemoryMap::new(pmarea, vmarea, permissions, is_shared);

    // Add the mapping to the task's VM manager
    match task.vm_manager.add_memory_map(vm_map) {
        Ok(()) => {
            // Track this anonymous mapping
            ANONYMOUS_MAPPINGS.lock().insert(vaddr);
            
            // Note: we're not freeing the pages here, they'll remain allocated
            // until explicitly unmapped. In a more complete implementation,
            // we should track these allocations for proper cleanup.
            
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

    // Round up length to page boundary
    let aligned_length = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    // Check if this is an anonymous mapping we need to track
    let is_anonymous = {
        let anonymous_mappings = ANONYMOUS_MAPPINGS.lock();
        anonymous_mappings.contains(&vaddr)
    };

    if is_anonymous {
        // Handle anonymous mapping unmapping
        if let Some(_removed_map) = task.vm_manager.remove_memory_map_by_addr(vaddr) {
            // Remove from our tracking
            ANONYMOUS_MAPPINGS.lock().remove(&vaddr);
            0
        } else {
            usize::MAX
        }
    } else {
        // Find the memory mapping that contains this address
        if let Some(_memory_map) = task.vm_manager.search_memory_map(vaddr) {
            // For object-based mappings, we might want to call munmap on the object
            // For now, we'll just unmap from the VM manager
            if let Some(_removed_map) = task.vm_manager.remove_memory_map_by_addr(vaddr) {
                0
            } else {
                usize::MAX
            }
        } else {
            usize::MAX // No mapping found at this address
        }
    }
}