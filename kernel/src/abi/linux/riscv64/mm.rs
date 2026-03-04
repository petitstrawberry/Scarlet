use crate::{
    abi::linux::riscv64::{
        LinuxRiscv64Abi,
        errno::{self, to_result},
    },
    arch::Trapframe,
    environment::PAGE_SIZE,
    mem::page::PageAllocation,
    task::mytask,
    vm::vmem::{MemoryArea, VirtualMemoryMap},
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
            return to_result(errno::EINVAL);
        }
        let result = handle_anonymous_mapping(task, addr, aligned_length, num_pages, prot, flags);
        return result;
    }

    // Handle file-backed mappings
    if fd == -1 {
        return to_result(errno::EINVAL);
    }

    // Get handle from Linux fd
    let handle = match abi.get_handle(fd as usize) {
        Some(h) => h,
        None => return to_result(errno::EBADF),
    };

    // Get kernel object from handle
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => obj,
        None => return to_result(errno::EBADF),
    };

    // Check if object supports MemoryMappingOps
    let memory_mappable = match kernel_obj.as_memory_mappable() {
        Some(mappable) => mappable,
        None => return to_result(errno::ENODEV),
    };

    // Check if the object supports mmap
    if !memory_mappable.supports_mmap() {
        return to_result(errno::ENODEV);
    }

    // Get mapping information from the object.
    let is_shared = (flags & MAP_SHARED) != 0;
    let owner_name = memory_mappable.mmap_owner_name();
    let should_log = owner_name.contains("xkb");
    let mut ok_len = aligned_length;
    let (paddr, obj_permissions, _obj_is_shared) = loop {
        match memory_mappable.get_mapping_info_with(offset, ok_len, is_shared) {
            Ok(info) => break info,
            Err(_e) => {
                if ok_len >= PAGE_SIZE {
                    ok_len -= PAGE_SIZE;
                } else {
                    ok_len = 0;
                }
                if ok_len == 0 {
                    break (0, 0, false);
                }
            }
        }
    };

    // Decide sharing semantics from flags (MAP_SHARED controls sharing)
    let is_shared = (flags & MAP_SHARED) != 0;
    // Determine final address
    let is_fixed = (flags & MAP_FIXED) != 0;

    let final_vaddr = if addr == 0 {
        match task
            .vm_manager
            .find_unmapped_area(aligned_length, PAGE_SIZE)
        {
            Some(vaddr) => vaddr,
            None => return to_result(errno::ENOMEM),
        }
    } else {
        if addr % PAGE_SIZE != 0 {
            return to_result(errno::EINVAL);
        }

        if !is_fixed {
            let requested_end = addr + aligned_length - 1;
            let has_overlap = task.vm_manager.with_memmaps(|mm| {
                mm.values()
                    .any(|map| !(requested_end < map.vmarea.start || addr > map.vmarea.end))
            });

            if has_overlap {
                match task
                    .vm_manager
                    .find_unmapped_area(aligned_length, PAGE_SIZE)
                {
                    Some(vaddr) => vaddr,
                    None => return to_result(errno::ENOMEM),
                }
            } else {
                addr
            }
        } else {
            addr
        }
    };

    // Create memory areas
    let vmarea = MemoryArea::new(final_vaddr, final_vaddr + aligned_length - 1);

    // Convert protection flags to kernel permissions
    let mut prot_mask = 0;
    if (prot & PROT_READ) != 0 {
        prot_mask |= 0x1;
    }
    if (prot & PROT_WRITE) != 0 {
        prot_mask |= 0x2;
    }
    if (prot & PROT_EXEC) != 0 {
        prot_mask |= 0x4;
    }

    // For private mappings, use requested permissions directly
    const MAP_PRIVATE: usize = 0x02;
    let is_map_private_flag = (flags & MAP_PRIVATE) != 0;

    let mut final_permissions = if is_map_private_flag {
        prot_mask
    } else {
        obj_permissions & prot_mask
    };

    if prot != 0 {
        final_permissions |= 0x08; // Access from user space (only if not PROT_NONE)
    }

    // If this is a file-backed private mapping, allocate private pages now and copy contents
    if is_map_private_flag && !is_shared {
        // Allocate pages for the private copy using PageAllocation
        let mut page_alloc = match PageAllocation::new(num_pages) {
            Some(pa) => pa,
            None => return to_result(errno::ENOMEM),
        };
        let pages_ptr = page_alloc.as_ptr() as usize;
        let private_pmarea = MemoryArea::new(pages_ptr, pages_ptr + aligned_length - 1);

        let vm_map = VirtualMemoryMap::new(private_pmarea, vmarea, final_permissions, false, None);

        let map_result = if is_fixed {
            task.vm_manager
                .add_memory_map_fixed(vm_map)
                .map(|removed| Some(removed))
        } else {
            task.vm_manager.add_memory_map(vm_map).map(|_| None)
        };

        match map_result {
            Ok(removed_mappings_opt) => {
                // Notify owners for any removed mappings (only shared ones)
                if let Some(removed_mappings) = &removed_mappings_opt {
                    for removed_map in removed_mappings {
                        if removed_map.is_shared {
                            if let Some(owner_weak) = &removed_map.owner {
                                if let Some(owner) = owner_weak.upgrade() {
                                    owner.on_unmapped(
                                        removed_map.vmarea.start,
                                        removed_map.vmarea.size(),
                                    );
                                }
                            }
                        }
                    }
                }

                // Clean up page allocations from removed mappings
                if let Some(removed_mappings) = removed_mappings_opt {
                    for removed_map in removed_mappings {
                        if !removed_map.is_shared {
                            let pm_start = removed_map.pmarea.start;
                            let mut allocs = task.page_allocations.write();
                            if let Some(pos) = allocs
                                .iter()
                                .position(|pa| pa.as_ptr() as usize == pm_start)
                            {
                                allocs.remove(pos);
                            }
                        }
                    }
                }

                // Zero-initialize entire region, then copy only the mappable portion (ok_len)
                unsafe {
                    core::ptr::write_bytes(page_alloc.as_ptr() as *mut u8, 0u8, aligned_length);
                }
                if ok_len > 0 {
                    let copy_len = core::cmp::min(ok_len, aligned_length);
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            paddr as *const u8,
                            page_alloc.as_ptr() as *mut u8,
                            copy_len,
                        );
                    }
                }

                // Store the allocation so it will be freed on task exit
                task.page_allocations.write().push(page_alloc);

                final_vaddr
            }
            Err(_) => {
                // Drop page_alloc to free pages
                drop(page_alloc);
                to_result(errno::ENOMEM)
            }
        }
    } else {
        // Shared or object-backed mapping path
        if paddr == 0 && ok_len == 0 {
            return to_result(errno::EINVAL);
        }

        let ok_len_aligned = (ok_len / PAGE_SIZE) * PAGE_SIZE;
        if ok_len_aligned == 0 {
            return to_result(errno::EINVAL);
        }

        // Shrink vm/pm areas to the mappable prefix when necessary
        let vmarea = MemoryArea::new(final_vaddr, final_vaddr + ok_len_aligned - 1);
        let pmarea = MemoryArea::new(paddr, paddr + ok_len_aligned - 1);

        // Create virtual memory map with weak reference to the object (shared/object-backed)
        let owner = kernel_obj.as_memory_mappable_weak();
        let vm_map = VirtualMemoryMap::new(pmarea, vmarea, final_permissions, is_shared, owner);

        let map_result = if is_fixed {
            task.vm_manager
                .add_memory_map_fixed(vm_map)
                .map(|removed| Some(removed))
        } else {
            task.vm_manager.add_memory_map(vm_map).map(|_| None)
        };

        match map_result {
            Ok(removed_mappings_opt) => {
                // Notify the object that mapping was created
                memory_mappable.on_mapped(final_vaddr, paddr, aligned_length, offset);

                // First, notify object owners about removed mappings
                if let Some(removed_mappings) = &removed_mappings_opt {
                    for removed_map in removed_mappings {
                        if removed_map.is_shared {
                            if let Some(owner_weak) = &removed_map.owner {
                                if let Some(owner) = owner_weak.upgrade() {
                                    owner.on_unmapped(
                                        removed_map.vmarea.start,
                                        removed_map.vmarea.size(),
                                    );
                                }
                            }
                        }
                    }
                }

                // Then, handle page allocation cleanup for private mappings
                if let Some(removed_mappings) = removed_mappings_opt {
                    for removed_map in removed_mappings {
                        if !removed_map.is_shared {
                            let pm_start = removed_map.pmarea.start;
                            let mut allocs = task.page_allocations.write();
                            if let Some(pos) = allocs
                                .iter()
                                .position(|pa| pa.as_ptr() as usize == pm_start)
                            {
                                allocs.remove(pos);
                            }
                        }
                    }
                }

                final_vaddr
            }
            Err(_) => to_result(errno::ENOMEM),
        }
    }
}

/// Handle anonymous memory mapping based on scarlet's implementation
fn handle_anonymous_mapping(
    task: &crate::task::Task,
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
    const MAP_FIXED: usize = 0x10;

    // For anonymous mappings, decide shareable based on flags
    const MAP_SHARED: usize = 0x01;
    let is_shared = (flags & MAP_SHARED) != 0;

    // Determine final address - if vaddr is 0, find an unmapped area
    let final_vaddr = if vaddr == 0 {
        match task
            .vm_manager
            .find_unmapped_area(aligned_length, PAGE_SIZE)
        {
            Some(addr) => addr,
            None => return to_result(errno::ENOMEM),
        }
    } else {
        let is_fixed = (flags & MAP_FIXED) != 0;
        if !is_fixed {
            let requested_end = vaddr + aligned_length - 1;
            let has_overlap = task.vm_manager.with_memmaps(|mm| {
                mm.values()
                    .any(|map| !(requested_end < map.vmarea.start || vaddr > map.vmarea.end))
            });

            if has_overlap {
                match task
                    .vm_manager
                    .find_unmapped_area(aligned_length, PAGE_SIZE)
                {
                    Some(addr) => addr,
                    None => return to_result(errno::ENOMEM),
                }
            } else {
                vaddr
            }
        } else {
            vaddr
        }
    };

    // For anonymous mappings, allocate physical memory directly using PageAllocation
    let mut page_alloc = match PageAllocation::new(num_pages) {
        Some(pa) => pa,
        None => return to_result(errno::ENOMEM),
    };
    let pages_ptr = page_alloc.as_ptr() as usize;

    // Convert protection flags to kernel permissions
    let mut permissions = 0;
    if prot != 0 {
        permissions |= 0x08; // Access from user space (only if not PROT_NONE)
        if (prot & PROT_READ) != 0 {
            permissions |= 0x1; // Readable
        }
        if (prot & PROT_WRITE) != 0 {
            permissions |= 0x2; // Writable
        }
        if (prot & PROT_EXEC) != 0 {
            permissions |= 0x4; // Executable
        }
    }

    // Create memory areas
    let vmarea = MemoryArea::new(final_vaddr, final_vaddr + aligned_length - 1);
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

            // Then, handle page allocation cleanup (MMU cleanup is already handled by VmManager.add_memory_map_fixed)
            for removed_map in removed_mappings {
                if !removed_map.is_shared {
                    let pm_start = removed_map.pmarea.start;
                    let mut allocs = task.page_allocations.write();
                    if let Some(pos) = allocs
                        .iter()
                        .position(|pa| pa.as_ptr() as usize == pm_start)
                    {
                        allocs.remove(pos);
                    }
                }
            }

            // Store the allocation so it will be freed on task exit
            task.page_allocations.write().push(page_alloc);

            final_vaddr
        }
        Err(_) => {
            // Drop allocated pages on error to avoid leak
            drop(page_alloc);
            to_result(errno::ENOMEM)
        }
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

    trapframe.increment_pc_next(task);

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
            return usize::MAX; // -ENOMEM
        }
    }

    // Get the original mapping to determine properties
    let original_mapping = match task.vm_manager.search_memory_map(addr) {
        Some(map) => map,
        None => return usize::MAX, // -ENOMEM
    };

    // Convert Linux protection flags to kernel permissions
    let mut new_permissions = 0;
    if prot != 0 {
        new_permissions |= 0x08; // Access from user space (only if not PROT_NONE)
        if (prot & PROT_READ) != 0 {
            new_permissions |= 0x1; // Readable
        }
        if (prot & PROT_WRITE) != 0 {
            new_permissions |= 0x2; // Writable
        }
        if (prot & PROT_EXEC) != 0 {
            new_permissions |= 0x4; // Executable
        }
    }

    // For file-backed mappings, check object permissions
    if let Some(owner_weak) = &original_mapping.owner {
        if let Some(owner) = owner_weak.upgrade() {
            let offset = addr - original_mapping.vmarea.start;
            if let Ok((_, obj_permissions, _)) = owner.get_mapping_info(offset, aligned_length) {
                if (new_permissions & obj_permissions) != (new_permissions & 0x7) {
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
        Ok(_removed_mappings) => 0, // Success
        Err(_) => usize::MAX,       // -EFAULT
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

        // Remove page allocations only for private mappings
        if !removed_map.is_shared {
            let pm_start = removed_map.pmarea.start;
            let mut allocs = task.page_allocations.write();
            if let Some(pos) = allocs
                .iter()
                .position(|pa| pa.as_ptr() as usize == pm_start)
            {
                allocs.remove(pos);
            }
        }

        0
    } else {
        usize::MAX // No mapping found at this address
    }
}

// TODO: Migrate object-backed MAP_PRIVATE mappings to delayed Copy-On-Write (COW).
// (omitted for brevity)
