use core::arch::asm;
use core::panic;

use crate::arch::trap::print_traplog;
use crate::arch::Trapframe;
use crate::sched::scheduler::get_scheduler;

pub fn arch_exception_handler(trapframe: &mut Trapframe, cause: usize) {
    match cause {
        /* Instruction page fault */
        12 => {
            let vaddr = trapframe.epc as usize;
            let task = get_scheduler().get_current_task(trapframe.get_cpuid()).unwrap();
            let manager = &task.vm_manager;
            match manager.search_memory_map(vaddr) {
                Some(mmap) => {
                    match manager.get_root_page_table() {
                        Some(root_page_table) => {
                            let paddr = mmap.get_paddr(vaddr).unwrap();
                            root_page_table.map(vaddr, paddr);
                        }
                        None => panic!("Root page table is not found"),
                    }
                }
                None => panic!("Not found memory map matched with vaddr: {:#x}", vaddr),
            }
        }
        /* Load/Store page fault */
        13 | 15 => {
            let vaddr;
            unsafe {
                asm!("csrr {}, stval", out(reg) vaddr);
            }
            let task = get_scheduler().get_current_task(trapframe.get_cpuid()).unwrap();
            let manager = &task.vm_manager;
            match manager.search_memory_map(vaddr) {
                Some(mmap) => {
                    match manager.get_root_page_table() {
                        Some(root_page_table) => {
                            let paddr = mmap.get_paddr(vaddr).unwrap();
                            root_page_table.map(vaddr, paddr);
                        }
                        None => panic!("Root page table is not found"),
                    }
                }
                None => {
                    print_traplog(trapframe);
                    panic!("Not found memory map matched with vaddr: {:#x}", vaddr);
                }
            }
        },
        _ => {
            print_traplog(trapframe);
            panic!("Unhandled exception: {}", cause);
            
        }
    }
}