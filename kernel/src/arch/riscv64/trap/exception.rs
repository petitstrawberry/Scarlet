use core::arch::asm;
use core::panic;

use crate::arch::Arch;
use crate::println;
use crate::print;
use crate::vm::get_kernel_virtual_memory_manager;

pub fn arch_exception_handler(arch: &mut Arch, cause: usize) {
    match cause {
        /* Instruction page fault */
        12 => {
            let vaddr = arch.epc as usize;
            let manager = get_kernel_virtual_memory_manager();
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
                None => (),
            }
        }
        /* Load/Store page fault */
        13 | 15 => {
            let vaddr;
            unsafe {
                asm!("csrr {}, stval", out(reg) vaddr);
            }
            let manager = get_kernel_virtual_memory_manager();
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
                None => (),
            }
        },
        _ => {
            println!("(Trapframe)\n{:#x?}", arch);
            panic!("Unhandled exception: {}", cause);
        }
    }
}