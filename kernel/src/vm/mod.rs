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

pub mod manager;
pub mod vmem;

unsafe extern "C" {
    static __KERNEL_SPACE_START: usize;
    static __KERNEL_SPACE_END: usize;
}

static mut KERNEL_VM_MANAGER: Option<VirtualMemoryManager> = None;

pub fn get_kernel_vm_manager() -> &'static mut VirtualMemoryManager {
    unsafe
    {
        match KERNEL_VM_MANAGER {
            Some(ref mut m) => m,
            None => {
                kernel_vm_manager_init();
                get_kernel_vm_manager()
            }
        }
    }
}

fn kernel_vm_manager_init() {
    let mut manager = VirtualMemoryManager::new();

    let asid = alloc_virtual_address_space(); /* Kernel ASID */
    let root_page_table_idx = get_root_page_table_idx(asid).unwrap();
    let root_page_table = get_page_table(root_page_table_idx).unwrap();
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
    /* Pre-map the kernel space */
    root_page_table.map_memory_area(memmap);

    let devmap = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: 0x0,
            end: 0x8000_0000,
        },
        pmarea: MemoryArea {
            start: 0x0,
            end: 0x8000_0000,
        }
    };
    manager.add_memory_map(devmap);

    unsafe {
        KERNEL_VM_MANAGER = Some(manager);
    }
}

/* Initialize MMU and enable paging */
pub fn kernel_vm_init() {
    let manager = get_kernel_vm_manager();
    let root_page_table = manager.get_root_page_table().unwrap();
    root_page_table.switch(manager.get_asid());
}

