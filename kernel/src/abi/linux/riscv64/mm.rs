use crate::{
    abi::linux::riscv64::{
        LinuxRiscv64Abi,
        errno::{self, to_result},
    },
    arch::Trapframe,
    environment::PAGE_SIZE,
    object::capability::memory_mapping::syscall::reclaim_private_removed_mapping,
    task::mytask,
    vm::addr::{is_direct_mapped, virt_to_phys},
    vm::vmem::{MemoryArea, VirtualMemoryMap},
};
use alloc::vec::Vec;

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

    // Decide sharing semantics from flags (MAP_SHARED controls sharing)
    let is_shared = (flags & MAP_SHARED) != 0;
    let is_fixed = (flags & MAP_FIXED) != 0;
    const MAP_PRIVATE: usize = 0x02;
    let is_map_private_flag = (flags & MAP_PRIVATE) != 0;

    // Determine final address
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

    let mut final_permissions = if is_map_private_flag {
        prot_mask
    } else {
        prot_mask
    };

    if prot != 0 {
        final_permissions |= 0x08;
    }

    if is_map_private_flag && !is_shared {
        let owner = match kernel_obj.as_memory_mappable_arc() {
            Some(owner) => owner,
            None => return to_result(errno::ENODEV),
        };
        let vm_map = VirtualMemoryMap {
            pmarea: MemoryArea { start: 0, end: 0 },
            vmarea: MemoryArea::new(final_vaddr, final_vaddr + aligned_length - 1),
            vm_start: final_vaddr,
            permissions: final_permissions,
            is_shared: false,
            owner: Some(owner),
        };

        let removed_mappings = if is_fixed {
            task.vm_manager
                .add_memory_map_fixed(vm_map)
                .map_err(|_| to_result(errno::ENOMEM))
        } else {
            task.vm_manager
                .add_memory_map(vm_map)
                .map(|_| Vec::new())
                .map_err(|_| to_result(errno::ENOMEM))
        };

        let removed_mappings = match removed_mappings {
            Ok(rm) => rm,
            Err(e) => return e,
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
    let file_obj = kernel_obj.as_file();
    let mut ok_len = aligned_length;
    let (mapping_base, obj_permissions, _obj_is_shared) = loop {
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
    let paddr = if mapping_base != 0 && is_direct_mapped(mapping_base) {
        virt_to_phys(mapping_base)
    } else {
        mapping_base
    };

    final_permissions = obj_permissions & prot_mask;
    if prot != 0 {
        final_permissions |= 0x08;
    }

    if paddr == 0 && ok_len == 0 {
        return to_result(errno::EINVAL);
    }

    let ok_len_aligned = (ok_len / PAGE_SIZE) * PAGE_SIZE;
    if ok_len_aligned == 0 {
        return to_result(errno::EINVAL);
    }

    let vmarea = MemoryArea::new(final_vaddr, final_vaddr + ok_len_aligned - 1);
    let pmarea = MemoryArea::new(paddr, paddr + ok_len_aligned - 1);

    let owner = kernel_obj.as_memory_mappable_arc();
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
            memory_mappable.on_mapped(final_vaddr, paddr, aligned_length, offset);

            if let Some(removed_mappings) = &removed_mappings_opt {
                for removed_map in removed_mappings {
                    if removed_map.is_shared {
                        if let Some(owner) = &removed_map.owner {
                            owner.on_unmapped(removed_map.vmarea.start, removed_map.vmarea.size());
                        }
                    }
                }
            }

            if let Some(removed_mappings) = removed_mappings_opt {
                for removed_map in removed_mappings {
                    reclaim_private_removed_mapping(task, &removed_map);
                }
            }

            final_vaddr
        }
        Err(_) => to_result(errno::ENOMEM),
    }
}

/// Handle anonymous memory mapping based on scarlet's implementation
fn handle_anonymous_mapping(
    task: &crate::task::Task,
    vaddr: usize,
    aligned_length: usize,
    _num_pages: usize,
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

    // Convert protection flags to kernel permissions
    let mut permissions = 0;
    if prot != 0 {
        permissions |= 0x08;
        if (prot & PROT_READ) != 0 {
            permissions |= 0x1;
        }
        if (prot & PROT_WRITE) != 0 {
            permissions |= 0x2;
        }
        if (prot & PROT_EXEC) != 0 {
            permissions |= 0x4;
        }
    }

    let owner: alloc::sync::Arc<dyn crate::object::capability::memory_mapping::MemoryMappingOps> =
        alloc::sync::Arc::new(
            crate::object::capability::memory_mapping::anon_owner::AnonymousPageOwner::new(),
        );

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
        Err(_) => return to_result(errno::ENOMEM),
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
        if task.vm_manager.translate_to_kva(page_addr).is_none() {
            return usize::MAX; // -ENOMEM
        }
    }

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

    for i in 0..num_pages {
        let page_addr = addr + i * PAGE_SIZE;
        let original_mapping = match task.vm_manager.search_memory_map(page_addr) {
            Some(map) => map,
            None => return usize::MAX,
        };

        if let Some(owner) = &original_mapping.owner {
            let offset = page_addr - original_mapping.vmarea.start;
            if let Ok((_, obj_permissions, _)) = owner.get_mapping_info(offset, PAGE_SIZE) {
                if (new_permissions & obj_permissions) != (new_permissions & 0x7) {
                    return usize::MAX;
                }
            }
        }

        let offset_in_mapping = page_addr - original_mapping.vmarea.start;
        let new_paddr = original_mapping.pmarea.start + offset_in_mapping;
        let new_map = VirtualMemoryMap::new(
            MemoryArea::new(new_paddr, new_paddr + PAGE_SIZE - 1),
            MemoryArea::new(page_addr, page_addr + PAGE_SIZE - 1),
            new_permissions,
            original_mapping.is_shared,
            original_mapping.owner.clone(),
        );

        if task.vm_manager.add_memory_map_fixed(new_map).is_err() {
            return usize::MAX;
        }
    }

    0
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
// (omitted for brevity)
