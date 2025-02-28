//! Virtual memory module.
//! 
//! This module provides the virtual memory abstraction for the kernel. It
//! includes functions for initializing the memory management unit (MMU) and
//! managing virtual address spaces.

use manager::VirtualMemoryManager;
use vmem::MemoryArea;
use vmem::VirtualMemoryMap;

use crate::arch::vm::alloc_virtual_address_space;
use crate::arch::vm::get_page_table;
use crate::arch::vm::get_root_page_table_idx;
use crate::print;
use crate::println;

pub mod manager;
pub mod vmem;

unsafe extern "C" {
    static __KERNEL_SPACE_START: usize;
    static __KERNEL_SPACE_END: usize;
}

static mut MANAGER: Option<VirtualMemoryManager> = None;

pub fn get_kernel_virtual_memory_manager() -> &'static mut VirtualMemoryManager {
    unsafe
    {
        match MANAGER {
            Some(ref mut m) => m,
            None => panic!("Virtual memory manager is not initialized"),
        }
    }
}

/* Initialize MMU and enable paging */
pub fn kernel_vm_init() {
    let asid = alloc_virtual_address_space(); /* Kernel ASID */
    let root_page_table_idx = get_root_page_table_idx(asid).unwrap();
    let root_page_table = get_page_table(root_page_table_idx).unwrap();
    let mut manager = VirtualMemoryManager::new();
    manager.set_asid(asid);

    /* Map kernel space */
    let kernel_start =  unsafe { &__KERNEL_SPACE_START as *const usize as usize };
    let kernel_end = unsafe { &__KERNEL_SPACE_END as *const usize as usize };
    let memmap = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: kernel_start,
            end: kernel_end,
        },
        pmarea: MemoryArea {
            start: kernel_start,
            end: kernel_end,
        }
    };
    manager.add_memory_map(memmap);
    println!("Kernel space: {:#x} - {:#x}", kernel_start, kernel_end);
    /* Pre-map the kernel space */
    root_page_table.map_memory_area(memmap);
    unsafe {
        MANAGER = Some(manager);
    }

    /* Switch to the new page table */
    root_page_table.switch(asid);

    println!("Kernel VM initialized\n");
    println!("Now, we are in the virtual memory mode");
}