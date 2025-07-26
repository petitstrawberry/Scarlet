use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi, 
    arch::Trapframe, 
    task::mytask, 
    environment::PAGE_SIZE,
    vm::vmem::{MemoryArea, VirtualMemoryMap},
    mem::page::allocate_raw_pages,
};
use alloc::boxed::Box;

pub fn sys_mmap(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    // Linux mmap constants
    const MAP_ANONYMOUS: usize = 0x20;
    #[allow(dead_code)]
    const MAP_FIXED: usize = 0x10;
    #[allow(dead_code)]
    const MAP_SHARED: usize = 0x01;
    
    // Linux protection flags
    #[allow(dead_code)]
    const PROT_READ: usize = 0x1;
    #[allow(dead_code)]
    const PROT_WRITE: usize = 0x2;
    #[allow(dead_code)]
    const PROT_EXEC: usize = 0x4;

    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let addr = trapframe.get_arg(0);
    let length = trapframe.get_arg(1);
    let prot = trapframe.get_arg(2);
    let flags = trapframe.get_arg(3);
    let fd = trapframe.get_arg(4) as isize;
    let _offset = trapframe.get_arg(5);  // Unused for anonymous mapping
    
    trapframe.increment_pc_next(task);

    // Input validation
    if length == 0 {
        return usize::MAX; // -EINVAL
    }

    // Only support anonymous mapping for now
    if flags & MAP_ANONYMOUS == 0 {
        crate::println!("sys_mmap: Only anonymous mapping is supported");
        return usize::MAX; // -ENOTSUP
    }

    if fd != -1 {
        crate::println!("sys_mmap: File descriptor mapping not supported");
        return usize::MAX; // -ENOTSUP
    }

    // Round up length to page boundary
    let aligned_length = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let num_pages = aligned_length / PAGE_SIZE;

    // Handle anonymous mapping using the same approach as scarlet's sys_memory_map
    handle_anonymous_mapping(task, addr, aligned_length, num_pages, prot, flags)
}

/// Handle anonymous memory mapping based on scarlet's implementation
fn handle_anonymous_mapping(
    task: &mut crate::task::Task,
    mut vaddr: usize,
    aligned_length: usize,
    num_pages: usize,
    prot: usize,
    flags: usize,
) -> usize {
    // Linux protection flags
    const PROT_READ: usize = 0x1;
    const PROT_WRITE: usize = 0x2;
    const PROT_EXEC: usize = 0x4;
    const MAP_SHARED: usize = 0x01;

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
    // let is_shared = (flags & MAP_SHARED) != 0;
    let is_shared = false; // Anonymous mappings are not shared
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
                let page_vaddr = vaddr + i * PAGE_SIZE;
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

pub fn sys_mprotect(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let addr = trapframe.get_arg(0);
    let length = trapframe.get_arg(1);
    let _prot = trapframe.get_arg(2);

    trapframe.increment_pc_next(task);

    // Input validation
    if length == 0 || addr % PAGE_SIZE != 0 {
        return usize::MAX; // -EINVAL
    }

    // Check if the memory region is actually mapped
    let paddr = task.vm_manager.translate_vaddr(addr);
    if paddr.is_none() {
        crate::println!("sys_mprotect: Invalid address {:#x}", addr);
        return usize::MAX; // -EINVAL
    }

    // TODO: Implement memory protection change
    // For now, we just return success as a placeholder
    // In a full implementation, this would:
    // 1. Find the memory mapping at the given address
    // 2. Update the protection flags in the page table
    // 3. Handle partial page protection changes if needed
    
    0 // Success (placeholder)
}

pub fn sys_munmap(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let addr = trapframe.get_arg(0);
    let length = trapframe.get_arg(1);

    trapframe.increment_pc_next(task);
    
    // Input validation
    if length == 0 || addr % PAGE_SIZE != 0 {
        return usize::MAX; // -EINVAL
    }

    if addr == 0 {
        crate::println!("sys_munmap: Cannot unmap null address");
        return usize::MAX; // -EINVAL
    }

    // Check if this is an anonymous mapping
    let is_anonymous = task.is_anonymous_mapping(addr);

    if is_anonymous {
        // Handle anonymous mapping unmapping
        if let Some(removed_map) = task.vm_manager.remove_memory_map_by_addr(addr) {
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
            task.remove_anonymous_mapping(addr);
            0
        } else {
            usize::MAX
        }
    } else {
        // Handle object-based mapping unmapping (not currently used in Linux ABI)
        if let Some(removed_map) = task.vm_manager.remove_memory_map_by_addr(addr) {
            // Notify the object owner if available
            if let Some(owner_weak) = &removed_map.owner {
                if let Some(owner) = owner_weak.upgrade() {
                    owner.on_unmapped(addr, length);
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