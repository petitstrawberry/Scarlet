use crate::abi::darwin::aarch64::DarwinAarch64Abi;
use crate::abi::darwin::error::*;
use crate::arch::Trapframe;
use crate::task::mytask;

pub fn sys_mach_task_self(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);
    trapframe.set_return_value(abi.mach_task_self());
    abi.mach_task_self()
}

pub fn sys_mach_msg_trap(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    crate::println!(
        "[darwin] mach_msg_trap: unimplemented (x0={:#x}, x1={:#x})",
        trapframe.get_arg(0),
        trapframe.get_arg(1)
    );

    trapframe.spsr |= 1 << 29;
    trapframe.set_return_value(ENOSYS);
    usize::MAX
}

pub fn sys_mach_port_allocate(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _task_port = trapframe.get_arg(0);
    let right = trapframe.get_arg(1) as u32;
    let name_ptr = trapframe.get_arg(2);

    crate::println!("[darwin] mach_port_allocate: right={}", right);

    let port_name = match abi.allocate_mach_port(right) {
        Ok(name) => name,
        Err(_) => {
            trapframe.spsr |= 1 << 29;
            trapframe.set_return_value(ENOMEM);
            return usize::MAX;
        }
    };

    if name_ptr != 0 {
        if let Some(kaddr) = task.vm_manager.translate_to_kva(name_ptr) {
            unsafe {
                *(kaddr as *mut u32) = port_name;
            }
        }
    }

    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}

pub fn sys_mach_port_deallocate(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _task_port = trapframe.get_arg(0);
    let name = trapframe.get_arg(1) as u32;

    abi.deallocate_mach_port(name);

    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}

pub fn sys_vm_allocate(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _target_task = trapframe.get_arg(0);
    let addr_ptr = trapframe.get_arg(1);
    let size = trapframe.get_arg(2) as usize;
    let _flags = trapframe.get_arg(3) as i32;

    let aligned_size =
        (size + crate::environment::PAGE_SIZE - 1) & !(crate::environment::PAGE_SIZE - 1);

    let vaddr = match task
        .vm_manager
        .find_unmapped_area(aligned_size, crate::environment::PAGE_SIZE)
    {
        Some(addr) => addr,
        None => {
            trapframe.set_return_value(KERN_NO_SPACE as usize);
            return usize::MAX;
        }
    };

    let num_pages = aligned_size / crate::environment::PAGE_SIZE;
    let pages = match crate::mem::page::ContiguousPages::new(num_pages) {
        Some(p) => p,
        None => {
            trapframe.set_return_value(KERN_RESOURCE_SHORTAGE as usize);
            return usize::MAX;
        }
    };

    let paddr = pages.as_paddr();
    let paddr_end = paddr + aligned_size;

    let mmap = crate::vm::vmem::VirtualMemoryMap::new(
        crate::vm::vmem::MemoryArea::new(paddr, paddr_end),
        crate::vm::vmem::MemoryArea::new(vaddr, vaddr + aligned_size),
        0x3,
        false,
        None,
    );

    match task.vm_manager.add_memory_map(mmap) {
        Ok(()) => {
            if addr_ptr != 0 {
                if let Some(kaddr) = task.vm_manager.translate_to_kva(addr_ptr) {
                    unsafe {
                        *(kaddr as *mut usize) = vaddr;
                    }
                }
            }
            trapframe.set_return_value(KERN_SUCCESS as usize);
            0
        }
        Err(_) => {
            trapframe.set_return_value(KERN_FAILURE as usize);
            usize::MAX
        }
    }
}

pub fn sys_vm_deallocate(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _target_task = trapframe.get_arg(0);
    let addr = trapframe.get_arg(1);
    let size = trapframe.get_arg(2);

    let _ = task.vm_manager.remove_memory_map_by_addr(addr);

    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}

pub fn sys_task_for_pid(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _target_task = trapframe.get_arg(0);
    let pid = trapframe.get_arg(1) as i32;
    let task_port_ptr = trapframe.get_arg(2);

    if pid as usize == task.get_id() {
        if task_port_ptr != 0 {
            if let Some(kaddr) = task.vm_manager.translate_to_kva(task_port_ptr) {
                unsafe {
                    *(kaddr as *mut u32) = abi.mach_task_self() as u32;
                }
            }
        }
        trapframe.set_return_value(KERN_SUCCESS as usize);
        0
    } else {
        trapframe.set_return_value(KERN_FAILURE as usize);
        usize::MAX
    }
}

pub fn sys_thread_create(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    crate::println!("[darwin] thread_create: stub (not implemented)");

    trapframe.set_return_value(KERN_FAILURE as usize);
    usize::MAX
}

pub fn sys_mach_timebase_info(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let info_ptr = trapframe.get_arg(0);

    // Apple Silicon: numer=1, denom=1 (nanosecond granularity)
    if info_ptr != 0 {
        if let Some(kaddr) = task.vm_manager.translate_to_kva(info_ptr) {
            unsafe {
                *(kaddr as *mut u32) = 1;
                *((kaddr as *mut u32).add(1)) = 1;
            }
        }
    }
    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}

pub fn sys_clock_get_time(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _clock_id = trapframe.get_arg(0);
    let time_ptr = trapframe.get_arg(1);

    if time_ptr != 0 {
        if let Some(kaddr) = task.vm_manager.translate_to_kva(time_ptr) {
            let now_us = crate::timer::get_time_us();
            unsafe {
                *(kaddr as *mut u64) = now_us / 1_000_000;
                *((kaddr as *mut u64).add(1)) = (now_us % 1_000_000) * 1000;
            }
        }
    }
    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}

pub fn sys_host_page_size(abi: &mut DarwinAarch64Abi, trapframe: &mut Trapframe) -> usize {
    let task = mytask().unwrap();
    trapframe.increment_pc_next(task);

    let _host_port = trapframe.get_arg(0);
    let size_ptr = trapframe.get_arg(1);

    if size_ptr != 0 {
        if let Some(kaddr) = task.vm_manager.translate_to_kva(size_ptr) {
            unsafe {
                *(kaddr as *mut u32) = crate::environment::PAGE_SIZE as u32;
            }
        }
    }
    trapframe.set_return_value(KERN_SUCCESS as usize);
    0
}
