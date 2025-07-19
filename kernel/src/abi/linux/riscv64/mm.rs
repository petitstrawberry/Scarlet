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

    crate::println!("sys_mmap: addr={:#x}, length={}, prot={:#x}, flags={:#x}", 
        addr, length, prot, flags);
    crate::println!("sys_mmap: Allocating {} pages ({} bytes)", num_pages, aligned_length);

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
            crate::println!("sys_mmap: Address range {:#x}-{:#x} overlaps with existing mapping {:#x}-{:#x}", 
                vaddr, end_vaddr, overlapping_map.vmarea.start, overlapping_map.vmarea.end);
            vaddr += 0x10000000; // Try 256MB higher
            attempts += 1;
            continue;
        }
        
        match task.allocate_data_pages(vaddr, num_pages) {
            Ok(_mmap) => {
                crate::println!("sys_mmap: Successfully allocated memory at {:#x}-{:#x}", 
                    vaddr, vaddr + aligned_length - 1);
                return vaddr;
            }
            Err(e) => {
                crate::println!("sys_mmap: Failed to allocate at {:#x}: {}", vaddr, e);
                vaddr += 0x10000000; // Try 256MB higher
                attempts += 1;
            }
        }
    }

    crate::println!("sys_mmap: Failed to find suitable memory region");
    usize::MAX // -ENOMEM
}