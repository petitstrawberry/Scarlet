use core::arch::asm;
use core::panic;

use crate::abi::syscall_dispatcher;
use crate::arch::trap::print_traplog;
use crate::arch::Trapframe;
use crate::println;
use crate::sched::scheduler::get_scheduler;
use crate::task::mytask;

pub fn arch_exception_handler(trapframe: &mut Trapframe, cause: usize) {
    match cause {
        /* Environment call from U-mode */
        8 => {
            /* Execute SystemCall */
            match syscall_dispatcher(trapframe) {
                Ok(ret) => {
                    trapframe.set_return_value(ret);
                }
                Err(msg) => {
                    // panic!("Syscall error: {}", msg);
                    println!("Syscall error: {}", msg);
                    trapframe.set_return_value(usize::MAX); // Set error code: -1
                    trapframe.increment_pc_next(mytask().unwrap());
                }
            }
        }
        /* Instruction page fault */
        12 => {
            let mut vaddr = trapframe.epc as usize;
            let task = get_scheduler().get_current_task(trapframe.get_cpuid()).unwrap();
            let manager = &mut task.vm_manager;
            loop {
                match manager.lazy_map_page(vaddr) {
                    Ok(_) => (),
                    Err(_) => {
                        print_traplog(trapframe);
                        panic!("Failed to map page for instruction page fault at vaddr: {:#x}", vaddr);
                    }
                }

                if vaddr & 0b11 == 0 {
                    // If the address is aligned, we can stop
                    break;
                }
                vaddr = (vaddr + 4) & !0b11; // Align to the next 4-byte boundary
            }
        }
        /* Load/Store page fault */
        13 | 15 => {
            let mut vaddr;
            unsafe {
                asm!("csrr {}, stval", out(reg) vaddr);
            }
            let task = get_scheduler().get_current_task(trapframe.get_cpuid()).unwrap();
            let manager = &mut task.vm_manager;
            loop {
                match manager.lazy_map_page(vaddr) {
                    Ok(_) => (),
                    Err(_) => {
                        print_traplog(trapframe);
                        panic!("Failed to map page for load/store page fault at vaddr: {:#x}", vaddr);
                    }
                }

                if vaddr & 0b11 == 0 {
                    // If the address is aligned, we can stop
                    break;
                }
                vaddr = (vaddr + 4) & !0b11; // Align to the next 4-byte boundary
            }
        },
        _ => {
            print_traplog(trapframe);
            panic!("Unhandled exception: {}", cause);
            
        }
    }
}