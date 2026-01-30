use crate::{
    abi::linux::riscv64::{
        LinuxRiscv64Abi,
        errno::{self, to_result},
    },
    arch::Trapframe,
    environment::PAGE_SIZE,
    mem::page::allocate_raw_pages,
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

    // crate::println!("linux-riscv64: sys_mmap called: pc={:#x} addr={:#x} length={} prot={:#x} flags={:#x} fd={} offset={:#x}",
    //     trapframe.epc, addr, length, prot, flags, fd, offset);

    trapframe.increment_pc_next(task);

    // crate::println!("sys_mmap: Step 1 - PC incremented");

    // Input validation
    if length == 0 {
        // crate::println!("linux-riscv64: sys_mmap error: length == 0");
        return usize::MAX; // -EINVAL
    }

    // crate::println!("sys_mmap: Step 2 - Length validated");

    // Round up length to page boundary
    let aligned_length = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let num_pages = aligned_length / PAGE_SIZE;

    // crate::println!("sys_mmap: Step 3 - aligned_length={:#x}, num_pages={}", aligned_length, num_pages);

    // Handle ANONYMOUS mappings specially
    if (flags & MAP_ANONYMOUS) != 0 {
        // crate::println!("linux-riscv64: sys_mmap - handling anonymous mapping (addr={:#x}, length={})", addr, aligned_length);
        if fd != -1 {
            // crate::println!("linux-riscv64: sys_mmap error: anonymous mapping with fd != -1 (fd={})", fd);
            return to_result(errno::EINVAL);
        }
        let result = handle_anonymous_mapping(task, addr, aligned_length, num_pages, prot, flags);
        // crate::println!("sys_mmap: RETURN {:#x} (anonymous mapping)", result);
        return result;
    }

    // crate::println!("sys_mmap: Step 5 - Handling file-backed mapping");

    // Handle file-backed mappings
    if fd == -1 {
        // crate::println!("sys_mmap: File-backed mapping requires valid file descriptor");
        return to_result(errno::EINVAL);
    }

    // crate::println!("sys_mmap: Step 6 - Getting handle for fd={}", fd);

    // Get handle from Linux fd
    let handle = match abi.get_handle(fd as usize) {
        Some(h) => {
            // crate::println!("linux-riscv64: sys_mmap - fd {} -> handle {}", fd, h);
            h
        }
        None => {
            crate::println!(
                "linux-riscv64: sys_mmap error - invalid file descriptor {}",
                fd
            );
            return to_result(errno::EBADF);
        }
    };

    // crate::println!("sys_mmap: Step 8 - Getting kernel object");

    // Get kernel object from handle
    let kernel_obj = match task.handle_table.get(handle) {
        Some(obj) => {
            // crate::println!("sys_mmap: Step 9 - Got kernel object");
            obj
        }
        None => {
            // crate::println!("sys_mmap: Invalid handle {}", handle);
            return to_result(errno::EBADF);
        }
    };

    // crate::println!("sys_mmap: Step 10 - Checking if object supports memory mapping");

    // Check if object supports MemoryMappingOps
    let memory_mappable = match kernel_obj.as_memory_mappable() {
        Some(mappable) => {
            // crate::println!("linux-riscv64: sys_mmap - object supports memory mapping");
            mappable
        }
        None => {
            crate::println!(
                "linux-riscv64: sys_mmap error - object doesn't support memory mapping"
            );
            return to_result(errno::ENODEV);
        }
    };

    // crate::println!("sys_mmap: Step 12 - Checking if object supports mmap");

    // Check if the object supports mmap
    if !memory_mappable.supports_mmap() {
        crate::println!("sys_mmap: Object doesn't support mmap operation");
        return to_result(errno::ENODEV);
    }

    // crate::println!("sys_mmap: Step 13 - Getting mapping info (offset={}, length={})", offset, length);
    // crate::println!("linux-riscv64: sys_mmap - requesting mapping info (offset={:#x}, length={})", offset, length);

    // Get mapping information from the object.
    // Some backends reject length that extends beyond file size. We try to clamp
    // to the largest mappable length (page down step) to avoid immediate failure.
    let mut ok_len = aligned_length;
    let (paddr, obj_permissions, _obj_is_shared) = loop {
        match memory_mappable.get_mapping_info(offset, ok_len) {
            Ok(info) => {
                // crate::println!(
                //     "linux-riscv64: sys_mmap - get_mapping_info returned paddr={:#x}, obj_perm={:#x}, is_shared={}, ok_len={}",
                //     info.0, info.1, info.2, ok_len
                // );
                break info;
            }
            Err(e) => {
                if ok_len >= PAGE_SIZE {
                    ok_len -= PAGE_SIZE;
                } else {
                    ok_len = 0;
                }
                if ok_len == 0 {
                    // crate::println!(
                    //     "linux-riscv64: sys_mmap - object rejected requested length (offset={:#x}, length={}), no mappable bytes: {:?}",
                    //     offset, length, e
                    // );
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
            Some(vaddr) => {
                // crate::println!("linux-riscv64: sys_mmap - found unmapped area at {:#x}", vaddr);
                vaddr
            }
            None => {
                crate::println!(
                    "linux-riscv64: sys_mmap error - no suitable unmapped area for length={}",
                    aligned_length
                );
                return to_result(errno::ENOMEM);
            }
        }
    } else {
        if addr % PAGE_SIZE != 0 {
            crate::println!(
                "linux-riscv64: sys_mmap error - requested addr {:#x} not page aligned",
                addr
            );
            return to_result(errno::EINVAL);
        }

        if !is_fixed {
            // addr is a hint, check if it's available
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
                    Some(vaddr) => {
                        // crate::println!("linux-riscv64: sys_mmap - hint address {:#x} overlaps, alternative {:#x}", addr, vaddr);
                        vaddr
                    }
                    None => {
                        crate::println!(
                            "linux-riscv64: sys_mmap error - no suitable unmapped area for length={}",
                            aligned_length
                        );
                        return to_result(errno::ENOMEM);
                    }
                }
            } else {
                // crate::println!("linux-riscv64: sys_mmap - using hint address {:#x} (no overlap)", addr);
                addr
            }
        } else {
            // crate::println!("linux-riscv64: sys_mmap - using fixed address {:#x} (MAP_FIXED set)", addr);
            addr
        }
    };

    // crate::println!("linux-riscv64: sys_mmap - creating mapping vaddr={:#x} paddr={:#x} length={} perms_req={:#x} obj_perm={:#x} is_shared={}",
    //     final_vaddr, paddr, aligned_length, prot, obj_permissions, is_shared);

    // crate::println!("sys_mmap: Step 19 - Creating memory areas (vaddr={:#x}, paddr={:#x})", final_vaddr, paddr);

    // Create memory areas
    let vmarea = MemoryArea::new(final_vaddr, final_vaddr + aligned_length - 1);

    // crate::println!("sys_mmap: Step 20 - Calculating permissions");

    // Convert protection flags to kernel permissions
    // For private mappings, we use the requested prot directly
    // For shared mappings, we need to respect object permissions
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
    // For shared mappings, combine with object permissions
    const MAP_PRIVATE: usize = 0x02;
    let is_map_private_flag = (flags & MAP_PRIVATE) != 0;

    let mut final_permissions = if is_map_private_flag {
        // Private mapping: use requested permissions (will copy data)
        prot_mask
    } else {
        // Shared mapping: must respect object permissions
        obj_permissions & prot_mask
    };

    if prot != 0 {
        final_permissions |= 0x08; // Access from user space (only if not PROT_NONE)
    }

    // Note: Tail-only permission adjustments (e.g., execute-only) are handled separately.

    // crate::println!("sys_mmap: Step 21 - final_permissions={:#x} (prot_mask={:#x}, obj_perm={:#x}, is_private={})",
    //     final_permissions, prot_mask, obj_permissions, is_map_private_flag);

    // Determine whether the mapping was requested as MAP_PRIVATE

    // crate::println!("sys_mmap: Step 22 - is_map_private_flag={}, is_shared={}", is_map_private_flag, is_shared);

    // If this is a file-backed private mapping, allocate private pages now and copy contents
    if is_map_private_flag && !is_shared {
        // crate::println!("sys_mmap: Step 23 - Allocating pages for private mapping");
        // Allocate pages for the private copy
        let pages = allocate_raw_pages(num_pages);
        // crate::println!("sys_mmap: Step 24 - Allocated pages at {:#x}", pages as usize);
        let pages_ptr = pages as usize;
        let private_pmarea = MemoryArea::new(pages_ptr, pages_ptr + aligned_length - 1);

        // crate::println!("sys_mmap: Step 25 - Creating VirtualMemoryMap");
        let vm_map = VirtualMemoryMap::new(private_pmarea, vmarea, final_permissions, false, None);

        // crate::println!("sys_mmap: Step 26 - Adding memory map to VM manager (is_fixed={})", is_fixed);

        // Use add_memory_map_fixed only if MAP_FIXED is set, otherwise use add_memory_map
        let map_result = if is_fixed {
            task.vm_manager
                .add_memory_map_fixed(vm_map)
                .map(|removed| Some(removed))
        } else {
            task.vm_manager.add_memory_map(vm_map).map(|_| None)
        };

        match map_result {
            Ok(removed_mappings_opt) => {
                // let removed_count = removed_mappings_opt.as_ref().map(|r| r.len()).unwrap_or(0);
                // crate::println!("sys_mmap: Step 27 - Memory map added successfully, processing {} removed mappings", removed_count);
                // For private mappings we do not notify the original object via on_mapped
                // because the new mapping uses private physical pages and the object
                // is not the owner of those pages.

                // Notify owners for any removed mappings (only shared ones)
                // crate::println!("sys_mmap: Step 28 - Notifying owners of removed mappings");
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

                // Clean up managed pages from removed mappings
                // crate::println!("sys_mmap: Step 29 - Cleaning up managed pages from removed mappings");
                if let Some(removed_mappings) = removed_mappings_opt {
                    for removed_map in removed_mappings {
                        if !removed_map.is_shared {
                            let mapping_start = removed_map.vmarea.start;
                            let mapping_end = removed_map.vmarea.end;
                            let num_removed_pages =
                                (mapping_end - mapping_start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;
                            for i in 0..num_removed_pages {
                                let page_vaddr = mapping_start + i * PAGE_SIZE;
                                if let Some(_managed_page) = task.remove_managed_page(page_vaddr) {
                                    // freed when dropped
                                }
                            }
                        }
                    }
                }

                // Zero-initialize entire region, then copy only the mappable portion (ok_len)
                unsafe {
                    core::ptr::write_bytes(pages as *mut u8, 0u8, aligned_length);
                }
                if ok_len > 0 {
                    let copy_len = core::cmp::min(ok_len, aligned_length);
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            paddr as *const u8,
                            pages as *mut u8,
                            copy_len,
                        );
                    }
                }

                // Add managed pages for the task so they are freed on task exit
                // crate::println!("sys_mmap: Step 31 - Adding {} managed pages", num_pages);
                for i in 0..num_pages {
                    let page_vaddr = final_vaddr + i * crate::environment::PAGE_SIZE;
                    let page_ptr = unsafe { (pages as *mut crate::mem::page::Page).add(i) };
                    task.add_managed_page(crate::task::ManagedPage {
                        vaddr: page_vaddr,
                        page: unsafe { Box::from_raw(page_ptr) },
                    });
                }

                // crate::println!("sys_mmap: Step 32 - Private mapping complete, returning {:#x}", final_vaddr);

                // Print current memory map for debugging
                // crate::println!("=== Memory Map After mmap ===");
                // for map in task.vm_manager.memmap_iter() {
                //     crate::println!("  VA: {:#x}-{:#x} -> PA: {:#x}-{:#x} (perm: {:#x}, shared: {})",
                //         map.vmarea.start, map.vmarea.end,
                //         map.pmarea.start, map.pmarea.end,
                //         map.permissions, map.is_shared);
                // }
                // crate::println!("=============================");

                // crate::println!("sys_mmap: RETURN {:#x} (private file-backed)", final_vaddr);
                final_vaddr
            }
            Err(_) => {
                // crate::println!("sys_mmap: Step 33 - Failed to add private mapping");
                // Free allocated pages to avoid leak
                crate::mem::page::free_raw_pages(pages, num_pages);
                // crate::println!("sys_mmap: Failed to add private mapping");
                to_result(errno::ENOMEM)
            }
        }
    } else {
        // crate::println!("sys_mmap: Step 33 - Shared or object-backed mapping path");
        // For MAP_SHARED (or object-backed) mappings, if the backend couldn't provide
        // the full requested length, only map the largest page-aligned prefix and leave
        // the tail unmapped so accesses fault (Linux would raise SIGBUS beyond EOF).
        if paddr == 0 && ok_len == 0 {
            // Nothing mappable at all (e.g., offset beyond EOF)
            return to_result(errno::EINVAL);
        }

        let ok_len_aligned = (ok_len / PAGE_SIZE) * PAGE_SIZE;
        if ok_len_aligned == 0 {
            // Partial (< PAGE_SIZE) tail only is not representable as shared mapping safely
            // without a COW-like helper; reject for now.
            crate::println!(
                "linux-riscv64: sys_mmap - only subpage tail available; rejecting shared mapping"
            );
            return to_result(errno::EINVAL);
        }

        // Shrink vm/pm areas to the mappable prefix when necessary
        let vmarea = MemoryArea::new(final_vaddr, final_vaddr + ok_len_aligned - 1);
        let pmarea = MemoryArea::new(paddr, paddr + ok_len_aligned - 1);

        // Create virtual memory map with weak reference to the object (shared/object-backed)
        let owner = kernel_obj.as_memory_mappable_weak();
        let vm_map = VirtualMemoryMap::new(pmarea, vmarea, final_permissions, is_shared, owner);

        // Add the mapping to VM manager
        // crate::println!("sys_mmap: Step 35 - Adding memory map to VM manager (is_fixed={})", is_fixed);

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

                // Then, handle managed page cleanup (MMU cleanup is already handled by VmManager.add_memory_map_fixed)
                if let Some(removed_mappings) = removed_mappings_opt {
                    for removed_map in removed_mappings {
                        // Remove managed pages only for private mappings
                        if !removed_map.is_shared {
                            let mapping_start = removed_map.vmarea.start;
                            let mapping_end = removed_map.vmarea.end;
                            let num_removed_pages =
                                (mapping_end - mapping_start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;

                            for i in 0..num_removed_pages {
                                let page_vaddr = mapping_start + i * PAGE_SIZE;
                                if let Some(_managed_page) = task.remove_managed_page(page_vaddr) {
                                    // The managed page is automatically freed when dropped
                                }
                            }
                        }
                    }
                }

                // crate::println!("sys_mmap: RETURN {:#x} (shared/object-backed)", final_vaddr);
                final_vaddr
            }
            Err(_) => {
                // crate::println!("sys_mmap: Failed to add memory mapping");
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
            Some(addr) => {
                // crate::println!("sys_mmap (anonymous): Found unmapped area at {:#x}", addr);
                addr
            }
            None => {
                // crate::println!("sys_mmap (anonymous): No suitable address found");
                return to_result(errno::ENOMEM);
            }
        }
    } else {
        // If vaddr is non-zero and MAP_FIXED is not set, treat it as a hint
        let is_fixed = (flags & MAP_FIXED) != 0;
        if !is_fixed {
            // Check if the requested range is available
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
                    Some(addr) => {
                        // crate::println!("sys_mmap (anonymous): Hint address {:#x} occupied, using {:#x}", vaddr, addr);
                        addr
                    }
                    None => {
                        // crate::println!("sys_mmap (anonymous): No suitable address found");
                        return to_result(errno::ENOMEM);
                    }
                }
            } else {
                // crate::println!("sys_mmap (anonymous): Using hint address {:#x}", vaddr);
                vaddr
            }
        } else {
            // crate::println!("sys_mmap (anonymous): Using fixed address {:#x}", vaddr);
            vaddr
        }
    };

    // For anonymous mappings, allocate physical memory directly
    let pages = allocate_raw_pages(num_pages);
    let pages_ptr = pages as usize;

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

            // Then, handle managed page cleanup (MMU cleanup is already handled by VmManager.add_memory_map_fixed)
            for removed_map in removed_mappings {
                // Remove managed pages only for private mappings
                if !removed_map.is_shared {
                    let mapping_start = removed_map.vmarea.start;
                    let mapping_end = removed_map.vmarea.end;
                    let num_removed_pages =
                        (mapping_end - mapping_start + 1 + PAGE_SIZE - 1) / PAGE_SIZE;

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
            // Free allocated pages on error to avoid leak
            crate::mem::page::free_raw_pages(pages, num_pages);
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

    // Note: Do not globally enforce execute-only here.

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

// TODO: Migrate object-backed MAP_PRIVATE mappings to delayed Copy-On-Write (COW).
// Motivation:
// - Currently MAP_PRIVATE file-backed mappings in the Linux ABI handler may allocate
//   private copies eagerly. For large mappings or read-mostly workloads this is
//   inefficient. Delayed COW copies pages only on first write, saving memory and CPU.
//
// High-level plan (implementation checklist):
// 1) Syscall layer (this file): when a user requests MAP_PRIVATE, mark the new
//    VirtualMemoryMap with a `cow` flag and avoid performing an immediate copy.
//    - Install the mapping with write permission cleared so stores will trap.
//    - Keep the owner reference so reads can still source data from the object.
//
// 2) VM representation: ensure VirtualMemoryMap has a boolean `cow` field and
//    the field is propagated to all mapping creation sites in the kernel.
//
// 3) Trap handling: modify the architecture trap/exception handler so that a
//    store (write) page fault will check the `cow` flag on the mapping and call
//    a dedicated per-page COW handler instead of the generic lazy mapping path.
//
// 4) Per-page COW handler (Task::handle_cow_page): allocate a new physical page,
//    copy the contents from the original backing paddr into it, replace only the
//    single faulting page in the mapping (e.g. via vm_manager.add_memory_map_fixed
//    with a one-page mapping), map it immediately, and register it as a managed
//    page of the task so it will be freed on exit.
//
// 5) Fork/clone semantics: preserve COW semantics across fork/clone so parent and
//    child share pages until either writes; ensure managed_pages bookkeeping is
//    adjusted so that only private copies are freed by the owner task.
//
// 6) Tests and validation:
//    - Integration tests for two tasks mapping the same file MAP_PRIVATE and
//      verifying that a write by one task creates a private copy while the other
//      retains original contents.
//    - Tests for fork/clone + MAP_PRIVATE behavior and corner cases (partial-page
//      writes, overlapping mappings, munmap after COW).
//
// 7) Documentation: update rustdoc and design documents describing the `cow`
//    flag, the runtime behavior, and which object types are eligible for COW.
//
// Acceptance criteria:
// - MAP_PRIVATE mappings are created without eager copying (cow=true and write bit
//   cleared).
// - On first write to a page, only that page is copied and the writer receives a
//   private writable page while others continue to see the original data.
// - Tests pass and there are no page leaks.
//
// Notes:
// - Some object types (device MMIO, special backing) cannot be COW'ed safely;
//   in such cases the syscall should either fall back to eager copy, reject the
//   mapping, or require explicit flags. Document these cases.
