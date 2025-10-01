use crate::{
    abi::linux::riscv64::{LinuxRiscv64Abi, errno::{self, to_result}}, arch::Trapframe, environment::PAGE_SIZE, mem::page::allocate_raw_pages, task::mytask, vm::vmem::{MemoryArea, VirtualMemoryMap}
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

    // crate::println!("sys_mmap: addr={:#x}, length={}, prot={:#x}, flags={:#x}, fd={}, offset={}", 
    //     addr, length, prot, flags, fd, offset);
    
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
            return to_result(errno::EINVAL);
        }
        return handle_anonymous_mapping(task, addr, aligned_length, num_pages, prot, flags);
    }

    // Handle file-backed mappings
    if fd == -1 {
        crate::println!("sys_mmap: File-backed mapping requires valid file descriptor");
        return to_result(errno::EINVAL);
    }

    // Get handle from Linux fd
    let handle = match abi.get_handle(fd as usize) {
        Some(h) => h,
        None => {
            crate::println!("sys_mmap: Invalid file descriptor {}", fd);
            return to_result(errno::EBADF);
        }
    };

    // Get kernel object from handle
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => {
            crate::println!("sys_mmap: Invalid handle {}", handle);
            return to_result(errno::EBADF);
        }
    };

    // Check if object supports MemoryMappingOps
    let memory_mappable = match kernel_obj.as_memory_mappable() {
        Some(mappable) => mappable,
        None => {
            crate::println!("sys_mmap: Object doesn't support memory mapping");
            return to_result(errno::ENODEV);
        }
    };

    // Check if the object supports mmap
    if !memory_mappable.supports_mmap() {
        crate::println!("sys_mmap: Object doesn't support mmap operation");
        return to_result(errno::ENODEV);
    }

    // Get mapping information from the object
    let (paddr, obj_permissions, _obj_is_shared) = match memory_mappable.get_mapping_info(offset, length) {
        Ok(info) => info,
        Err(_) => {
            crate::println!("sys_mmap: Failed to get mapping info");
            return to_result(errno::EINVAL);
        }
    };

    // Decide sharing semantics from flags (MAP_SHARED controls sharing)
    let is_shared = (flags & MAP_SHARED) != 0;

    // Determine final address
    let final_vaddr = if addr == 0 {
        match task.vm_manager.find_unmapped_area(aligned_length, PAGE_SIZE) {
            Some(vaddr) => vaddr,
            None => {
                crate::println!("sys_mmap: No suitable address found");
                return to_result(errno::ENOMEM);
            }
        }
    } else {
        if addr % PAGE_SIZE != 0 {
            crate::println!("sys_mmap: Address not page-aligned");
            return to_result(errno::EINVAL);
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

    // Determine whether the mapping was requested as MAP_PRIVATE
    const MAP_PRIVATE: usize = 0x02;
    let is_map_private_flag = (flags & MAP_PRIVATE) != 0;

    // If this is a file-backed private mapping, allocate private pages now and copy contents
    if is_map_private_flag && !is_shared {
        // Allocate pages for the private copy
        let pages = allocate_raw_pages(num_pages);
        let pages_ptr = pages as usize;
        let private_pmarea = MemoryArea::new(pages_ptr, pages_ptr + aligned_length - 1);

        let vm_map = VirtualMemoryMap::new(private_pmarea, vmarea, final_permissions, false, None);

        match task.vm_manager.add_memory_map_fixed(vm_map) {
            Ok(removed_mappings) => {
                // For private mappings we do not notify the original object via on_mapped
                // because the new mapping uses private physical pages and the object
                // is not the owner of those pages.

                // Notify owners for any removed mappings (only shared ones)
                for removed_map in &removed_mappings {
                    if removed_map.is_shared {
                        if let Some(owner_weak) = &removed_map.owner {
                            if let Some(owner) = owner_weak.upgrade() {
                                owner.on_unmapped(removed_map.vmarea.start, removed_map.vmarea.size());
                            }
                        }
                    }
                }

                // Clean up managed pages from removed mappings
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

                // Copy contents from the original object paddr into our private pages
                for i in 0..num_pages {
                    let src = (paddr + i * PAGE_SIZE) as *const u8;
                    let dst_page = unsafe { (pages as *mut crate::mem::page::Page).add(i) } as *mut u8;
                    unsafe { core::ptr::copy_nonoverlapping(src, dst_page, PAGE_SIZE); }
                }

                // Add managed pages for the task so they are freed on task exit
                for i in 0..num_pages {
                    let page_vaddr = final_vaddr + i * crate::environment::PAGE_SIZE;
                    let page_ptr = unsafe { (pages as *mut crate::mem::page::Page).add(i) };
                    task.add_managed_page(crate::task::ManagedPage {
                        vaddr: page_vaddr,
                        page: unsafe { Box::from_raw(page_ptr) },
                    });
                }

                final_vaddr
            }
            Err(_) => {
                // Free allocated pages to avoid leak
                crate::mem::page::free_raw_pages(pages, num_pages);
                crate::println!("sys_mmap: Failed to add private mapping");
                to_result(errno::ENOMEM)
            }
        }
    } else {
        // Create virtual memory map with weak reference to the object (shared or private backed by object)
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
                        let num_removed_pages = (mapping_end - mapping_start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;
                        
                        for i in 0..num_removed_pages {
                            let page_vaddr = mapping_start + i * PAGE_SIZE;
                            if let Some(_managed_page) = task.remove_managed_page(page_vaddr) {
                                // The managed page is automatically freed when dropped
                            }
                        }
                    }
                }

                final_vaddr
            }
            Err(_) => {
                crate::println!("sys_mmap: Failed to add memory mapping");
                to_result(errno::ENOMEM)
            }
        }
    }
}

/// Handle anonymous memory mapping based on scarlet's implementation
fn handle_anonymous_mapping(
    task: &mut crate::task::Task,
    vaddr: usize,
    aligned_length: usize,
    num_pages: usize,
    prot: usize,
    flags: usize,
) -> usize {
    // Linux protection flags
    const PROT_READ: usize = 0x1;
    const PROT_WRITE: usize = 0x2;
    const PROT_EXEC: usize = 0x4;

    // For anonymous mappings, decide shareable based on flags
    const MAP_SHARED: usize = 0x01;
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

pub fn sys_mprotect(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
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

    // crate::println!("sys_mprotect: addr={:#x}, length={}, prot={:#x}", addr, length, prot);

    trapframe.increment_pc_next(task);
    // return 0;

    // Input validation
    if length == 0 || addr % PAGE_SIZE != 0 {
        return usize::MAX; // -EINVAL
    }

    // Round up length to page boundary
    let aligned_length = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let num_pages = aligned_length / PAGE_SIZE;

    // Check if all pages in the range are mapped
    for i in 0..num_pages {
        let page_addr = addr + i * PAGE_SIZE;
        if task.vm_manager.translate_vaddr(page_addr).is_none() {
            // crate::println!("sys_mprotect: Unmapped page at address {:#x}", page_addr);
            return usize::MAX; // -ENOMEM
        }
    }

    // Get the original mapping to determine properties
    let original_mapping = match task.vm_manager.search_memory_map(addr) {
        Some(map) => map,
        None => {
            // crate::println!("sys_mprotect: No memory mapping found at address {:#x}", addr);
            return usize::MAX; // -ENOMEM
        }
    };

    // Convert Linux protection flags to kernel permissions
    let mut new_permissions = 0;
    if (prot & PROT_READ) != 0 {
        new_permissions |= 0x1; // Readable
    }
    if (prot & PROT_WRITE) != 0 {
        new_permissions |= 0x2; // Writable
    }
    if (prot & PROT_EXEC) != 0 {
        new_permissions |= 0x4; // Executable
    }
    new_permissions |= 0x08; // Access from user space

    // For file-backed mappings, check object permissions
    if let Some(owner_weak) = &original_mapping.owner {
        if let Some(owner) = owner_weak.upgrade() {
            let offset = addr - original_mapping.vmarea.start;
            if let Ok((_, obj_permissions, _)) = owner.get_mapping_info(offset, aligned_length) {
                if (new_permissions & obj_permissions) != (new_permissions & 0x7) {
                    // crate::println!("sys_mprotect: Requested permissions exceed object permissions");
                    return usize::MAX; // -EACCES
                }
            }
        }
    }

    // Calculate physical address for the new mapping
    let offset_in_mapping = addr - original_mapping.vmarea.start;
    let new_paddr = original_mapping.pmarea.start + offset_in_mapping;

    // Create the new memory mapping with updated permissions
    let new_map = VirtualMemoryMap::new(
        MemoryArea::new(new_paddr, new_paddr + aligned_length - 1),
        MemoryArea::new(addr, addr + aligned_length - 1),
        new_permissions,
        original_mapping.is_shared,
        original_mapping.owner.clone(),
    );

    // Use add_memory_map_fixed to handle splitting and overlaps automatically
    match task.vm_manager.add_memory_map_fixed(new_map) {
        Ok(_removed_mappings) => {

            // crate::println!("sys_mprotect: Successfully updated permissions for {:#x}-{:#x}", 
                        //    addr, addr + aligned_length - 1);
            
            0 // Success
        }
        Err(_) => {
            // crate::println!("sys_mprotect: Failed to update memory mapping: {}", e);
            usize::MAX // -EFAULT
        }
    }
}

pub fn sys_munmap(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = match mytask() {
        Some(task) => task,
        None => return usize::MAX,
    };

    let vaddr = trapframe.get_arg(0);
    let length = trapframe.get_arg(1);

    trapframe.increment_pc_next(task);
    
    // Input validation
    if length == 0 || vaddr % PAGE_SIZE != 0 {
        return usize::MAX; // -EINVAL
    }

    if vaddr == 0 {
        crate::println!("sys_munmap: Cannot unmap null address");
        return usize::MAX; // -EINVAL
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