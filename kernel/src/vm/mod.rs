//! Virtual memory module.
//! 
//! This module provides the virtual memory abstraction for the kernel. It
//! includes functions for managing virtual address spaces.

use manager::VirtualMemoryManager;
use vmem::MemoryArea;
use vmem::VirtualMemoryMap;

use crate::arch::get_cpu;
use crate::arch::set_trap_frame;
use crate::arch::set_trap_vector;
use crate::arch::vm::alloc_virtual_address_space;
use crate::arch::vm::get_page_table;
use crate::arch::vm::get_root_page_table_idx;
use crate::environment::VMMAX;
use crate::println;
use crate::print;

pub mod manager;
pub mod vmem;

unsafe extern "C" {
    static __KERNEL_SPACE_START: usize;
    static __KERNEL_SPACE_END: usize;
    static __TRAMPOLINE_START: usize;
    static __TRAMPOLINE_END: usize;
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
    let manager = VirtualMemoryManager::new();

    unsafe {
        KERNEL_VM_MANAGER = Some(manager);
    }
}

/* Initialize MMU and enable paging */
pub fn kernel_vm_init() {
    let manager = get_kernel_vm_manager();

    let asid = alloc_virtual_address_space(); /* Kernel ASID */
    let root_page_table_idx = get_root_page_table_idx(asid).unwrap();
    let root_page_table = get_page_table(root_page_table_idx).unwrap();
    manager.set_asid(asid);

    /* Map kernel space */
    let kernel_start =  unsafe { &__KERNEL_SPACE_START as *const usize as usize };
    let kernel_end = unsafe { &__KERNEL_SPACE_END as *const usize as usize };

    let kernel_map = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: kernel_start,
            end: kernel_end,
        },
        pmarea: MemoryArea {
            start: kernel_start,
            end: kernel_end,
        }
    };
    manager.add_memory_map(kernel_map);
    /* Pre-map the kernel space */
    root_page_table.map_memory_area(kernel_map);

    let dev_map = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: 0x0,
            end: 0x8000_0000,
        },
        pmarea: MemoryArea {
            start: 0x0,
            end: 0x8000_0000,
        }
    };
    manager.add_memory_map(dev_map);

    setup_trampoline(manager);

    root_page_table.switch(manager.get_asid());


}

fn setup_trampoline(manager: &mut VirtualMemoryManager) {
    let trampoline_start = unsafe { &__TRAMPOLINE_START as *const usize as usize };
    let trampoline_end = unsafe { &__TRAMPOLINE_END as *const usize as usize } - 1;
    let trampoline_size = trampoline_end - trampoline_start;

    let arch = get_cpu();
    let trampoline_vaddr_start = VMMAX - trampoline_size;
    let trampoline_vaddr_end = VMMAX;

    let trap_entry_paddr = arch.get_user_trap_entry_paddr();
    let trapframe_paddr = arch.get_trapframe_paddr();
    let trap_entry_offset = trap_entry_paddr - trampoline_start;
    let trapframe_offset = trapframe_paddr - trampoline_start;

    let trap_entry_vaddr = trampoline_vaddr_start + trap_entry_offset;
    let trapframe_vaddr = trampoline_vaddr_start + trapframe_offset;
    
    println!("Trampoline paddr  : {:#x} - {:#x}", trampoline_start, trampoline_end);
    println!("Trap entry paddr  : {:#x}", trap_entry_paddr);
    println!("Trap frame paddr  : {:#x}", trapframe_paddr);
    println!("Trampoline vaddr  : {:#x} - {:#x}", trampoline_vaddr_start, trampoline_vaddr_end);
    println!("Trap entry vaddr  : {:#x}", trap_entry_vaddr);
    println!("Trap frame vaddr  : {:#x}", trapframe_vaddr);
    
    let trampoline_map = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: trampoline_vaddr_start,
            end: trampoline_vaddr_end,
        },
        pmarea: MemoryArea {
            start: trampoline_start,
            end: trampoline_end,
        }
    };

    manager.add_memory_map(trampoline_map);
    /* Pre-map the trampoline space */
    manager.get_root_page_table().unwrap().map_memory_area(trampoline_map);

    /* Set trap vector and frame for handling exceptions */
    set_trap_vector(trap_entry_vaddr);
    set_trap_frame(trapframe_vaddr);
}
