use crate::{
    abi::linux::riscv64::LinuxRiscv64Abi, 
    arch::Trapframe, 
    task::mytask, 
    environment::PAGE_SIZE,
    vm::vmem::{MemoryArea, VirtualMemoryMap},
    mem::page::allocate_raw_pages,
};
use alloc::boxed::Box;

pub fn sys_mmap(abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    // Linux mmap constants
    const MAP_ANONYMOUS: usize = 0x20;
    #[allow(dead_code)]
    const MAP_FIXED: usize = 0x10;
    #[allow(dead_code)]
    const MAP_SHARED: usize = 0x01;
    
    // Linux protection flags
    const PROT_READ: usize = 0x1;
    const PROT_WRITE: usize = 0x2;
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
    let offset = trapframe.get_arg(5);
    
    trapframe.increment_pc_next(task);

    // Input validation
    if length == 0 {
        return usize::MAX; // -EINVAL
    }

    // Round up length to page boundary
    let aligned_length = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let num_pages = aligned_length / PAGE_SIZE;

    // Handle ANONYMOUS mappings specially
    if (flags & MAP_ANONYMOUS) != 0 {
        if fd != -1 {
            crate::println!("sys_mmap: Anonymous mapping should not have file descriptor");
            return usize::MAX; // -EINVAL
        }
        return handle_anonymous_mapping(task, addr, aligned_length, num_pages, prot, flags);
    }

    // Handle file-backed mappings
    if fd == -1 {
        crate::println!("sys_mmap: File-backed mapping requires valid file descriptor");
        return usize::MAX; // -EINVAL
    }

    // Get handle from Linux fd
    let handle = match abi.get_handle(fd as usize) {
        Some(h) => h,
        None => {
            crate::println!("sys_mmap: Invalid file descriptor {}", fd);
            return usize::MAX; // -EBADF
        }
    };

    // Get kernel object from handle
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => {
            crate::println!("sys_mmap: Invalid handle {}", handle);
            return usize::MAX; // -EBADF
        }
    };

    // Check if object supports MemoryMappingOps
    let memory_mappable = match kernel_obj.as_memory_mappable() {
        Some(mappable) => mappable,
        None => {
            crate::println!("sys_mmap: Object doesn't support memory mapping");
            return usize::MAX; // -ENODEV
        }
    };

    // Check if the object supports mmap
    if !memory_mappable.supports_mmap() {
        crate::println!("sys_mmap: Object doesn't support mmap operation");
        return usize::MAX; // -ENODEV
    }

    // Get mapping information from the object
    let (paddr, obj_permissions, is_shared) = match memory_mappable.get_mapping_info(offset, length) {
        Ok(info) => info,
        Err(_) => {
            crate::println!("sys_mmap: Failed to get mapping info");
            return usize::MAX; // -EINVAL
        }
    };

    // Determine final address
    let final_vaddr = if addr == 0 {
        match task.vm_manager.find_unmapped_area(aligned_length, PAGE_SIZE) {
            Some(vaddr) => vaddr,
            None => {
                crate::println!("sys_mmap: No suitable address found");
                return usize::MAX; // -ENOMEM
            }
        }
    } else {
        if addr % PAGE_SIZE != 0 {
            crate::println!("sys_mmap: Address not page-aligned");
            return usize::MAX; // -EINVAL
        }
        addr
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
        Err(_) => {
            crate::println!("sys_mmap: Failed to add memory mapping");
            usize::MAX // -ENOMEM
        }
    }
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
        // Handle object-based mapping unmapping (file-backed mappings)
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