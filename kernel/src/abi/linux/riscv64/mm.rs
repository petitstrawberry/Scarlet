use crate::{abi::linux::riscv64::LinuxRiscv64Abi, arch::Trapframe, task::mytask, environment::PAGE_SIZE};

pub fn sys_mmap(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    // Linux mmap constants
    const MAP_ANONYMOUS: usize = 0x20;
    const MAP_FIXED: usize = 0x10;
    
    // Linux protection flags (for future use)
    #[allow(dead_code)]
    const PROT_READ: usize = 0x1;
    #[allow(dead_code)]
    const PROT_WRITE: usize = 0x2;
    #[allow(dead_code)]
    const PROT_EXEC: usize = 0x4;

    let task = mytask().unwrap();
    let addr = trapframe.get_arg(0);
    let length = trapframe.get_arg(1);
    let prot = trapframe.get_arg(2);
    let flags = trapframe.get_arg(3);
    let fd = trapframe.get_arg(4) as isize;
    let _offset = trapframe.get_arg(5);  // Unused for anonymous mapping
    
    trapframe.increment_pc_next(task);

    // Basic validation
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

    // Calculate pages needed
    let aligned_length = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let num_pages = aligned_length / PAGE_SIZE;

    // crate::println!("sys_mmap: addr={:#x}, length={}, prot={:#x}, flags={:#x}", 
    //     addr, length, prot, flags);
    // crate::println!("sys_mmap: Allocating {} pages ({} bytes)", num_pages, aligned_length);

    // For now, ignore the requested address and let the kernel choose
    // TODO: Support MAP_FIXED and address hints
    if addr != 0 && (flags & MAP_FIXED) != 0 {
        crate::println!("sys_mmap: MAP_FIXED not fully supported yet");
    }

    // Find a suitable virtual address in user space
    // Simple strategy: start from a high address and work down
    let mut vaddr = 0x40000000usize; // Start at 1GB mark
    
    // Try to find free space with proper overlap checking
    let mut attempts = 0;
    while attempts < 10 {
        // Check if this range would overlap with existing mappings
        let end_vaddr = vaddr + aligned_length - 1;
        if let Some(overlapping_map) = task.vm_manager.check_overlap(vaddr, end_vaddr) {
            // crate::println!("sys_mmap: Address range {:#x}-{:#x} overlaps with existing mapping {:#x}-{:#x}", 
            //     vaddr, end_vaddr, overlapping_map.vmarea.start, overlapping_map.vmarea.end);
            vaddr += 0x10000000; // Try 256MB higher
            attempts += 1;
            continue;
        }
        
        match task.allocate_data_pages(vaddr, num_pages) {
            Ok(_mmap) => {
                // crate::println!("sys_mmap: Successfully allocated memory at {:#x}-{:#x}", 
                //     vaddr, vaddr + aligned_length - 1);
                return vaddr;
            }
            Err(e) => {
                // crate::println!("sys_mmap: Failed to allocate at {:#x}: {}", vaddr, e);
                vaddr += 0x10000000; // Try 256MB higher
                attempts += 1;
            }
        }
    }

    crate::println!("sys_mmap: Failed to find suitable memory region");
    usize::MAX // -ENOMEM
}

pub fn sys_mprotect(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let addr = trapframe.get_arg(0);
    let length = trapframe.get_arg(1);
    let prot = trapframe.get_arg(2);

    trapframe.increment_pc_next(task);
    // crate::println!("sys_mprotect: addr={:#x}, length={}, prot={:#x}", addr, length, prot);

    let paddr = task.vm_manager.translate_vaddr(addr as usize);
    if paddr.is_none() {
        crate::println!("sys_mprotect: Invalid address {:#x}", addr);
        return usize::MAX; // -EINVAL
    }

    0 // Not implemented yet, return success for now
}

pub fn sys_munmap(_abi: &mut LinuxRiscv64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    let addr = trapframe.get_arg(0);
    let length = trapframe.get_arg(1);

    trapframe.increment_pc_next(task);
    
    // crate::println!("sys_munmap: addr={:#x}, length={}", addr, length);

    // Basic validation
    if length == 0 {
        crate::println!("sys_munmap: Invalid length 0");
        return usize::MAX; // -EINVAL
    }

    if addr == 0 {
        crate::println!("sys_munmap: Cannot unmap null address");
        return usize::MAX; // -EINVAL
    }

    // Check if address is page-aligned
    if addr % PAGE_SIZE != 0 {
        crate::println!("sys_munmap: Address {:#x} is not page-aligned", addr);
        return usize::MAX; // -EINVAL
    }

    // Calculate aligned length and pages
    let aligned_length = (length + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let num_pages = aligned_length / PAGE_SIZE;
    let end_addr = addr + aligned_length - 1;

    // crate::println!("sys_munmap: Unmapping {} pages ({} bytes) from {:#x} to {:#x}", 
    //     num_pages, aligned_length, addr, end_addr);

    // Check if the memory region is actually mapped
    let start_paddr = task.vm_manager.translate_vaddr(addr);
    if start_paddr.is_none() {
        crate::println!("sys_munmap: Address {:#x} is not mapped", addr);
        return usize::MAX; // -EINVAL
    }

    // Try to deallocate the memory region using free_data_pages
    task.free_data_pages(addr, num_pages);
    // crate::println!("sys_munmap: Successfully unmapped memory region {:#x}-{:#x}", addr, end_addr);
    0 // Success
}