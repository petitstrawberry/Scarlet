//! Virtual memory module.
//! 
//! This module provides the virtual memory abstraction for the kernel. It
//! includes functions for managing virtual address spaces.

use manager::VirtualMemoryManager;
use vmem::MemoryArea;
use vmem::VirtualMemoryMap;

use crate::arch::get_cpu;
use crate::arch::get_user_trapvector_paddr;
use crate::arch::vm::alloc_virtual_address_space;
use crate::arch::vm::get_page_table;
use crate::arch::vm::get_root_page_table_idx;
use crate::environment::NUM_OF_CPUS;
use crate::environment::VMMAX;
use crate::println;
use crate::print;

extern crate alloc;

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
#[allow(static_mut_refs)]
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
            start: 0x00,
            end: 0x8000_0000,
        },
        pmarea: MemoryArea {
            start: 0x00,
            end: 0x8000_0000,
        }
    };
    manager.add_memory_map(dev_map);

    // let kernel_stack_page: Box<[u8; 4096]> = Box::new([0; 4096]);
    // let kernel_stack_page_paddr = Box::into_raw(kernel_stack_page) as usize;
    // let kernel_stack_map = VirtualMemoryMap {
    //     vmarea: MemoryArea {
    //         start: unsafe { KERNEL_STACK.top() },
    //         end: unsafe { KERNEL_STACK.bottom() } - 1,
    //     },
    //     pmarea: MemoryArea {
    //         start: kernel_stack_page_paddr,
    //         end: kernel_stack_page_paddr + 0xfff,
    //     }
    // };
    // manager.add_memory_map(kernel_stack_map);

    println!("Device space mapped       : {:#018x} - {:#018x}", dev_map.vmarea.start, dev_map.vmarea.end);
    println!("Kernel space mapped       : {:#018x} - {:#018x}", kernel_start, kernel_end);

    setup_trampoline(manager);

    root_page_table.switch(manager.get_asid());
}


static mut TRAMPOLINE_TRAP_VECTOR: Option<usize> = None;
static mut TRAMPOLINE_TRAPFRAME: [Option<usize>; NUM_OF_CPUS] = [None; NUM_OF_CPUS];

fn setup_trampoline(manager: &mut VirtualMemoryManager) {
    let trampoline_start = unsafe { &__TRAMPOLINE_START as *const usize as usize };
    let trampoline_end = unsafe { &__TRAMPOLINE_END as *const usize as usize } - 1;
    let trampoline_size = trampoline_end - trampoline_start;

    let arch = get_cpu();
    let trampoline_vaddr_start = VMMAX - trampoline_size;
    let trampoline_vaddr_end = VMMAX;

    let trap_entry_paddr = get_user_trapvector_paddr();
    let trapframe_paddr = arch.get_trapframe_paddr();
    let trap_entry_offset = trap_entry_paddr - trampoline_start;
    let trapframe_offset = trapframe_paddr - trampoline_start;

    let trap_entry_vaddr = trampoline_vaddr_start + trap_entry_offset;
    let trapframe_vaddr = trampoline_vaddr_start + trapframe_offset;
    
    println!("Trampoline space mapped   : {:#x} - {:#x}", trampoline_vaddr_start, trampoline_vaddr_end);
    println!("  Trampoline paddr  : {:#x} - {:#x}", trampoline_start, trampoline_end);
    println!("  Trap entry paddr  : {:#x}", trap_entry_paddr);
    println!("  Trap frame paddr  : {:#x}", trapframe_paddr);
    println!("  Trampoline vaddr  : {:#x} - {:#x}", trampoline_vaddr_start, trampoline_vaddr_end);
    println!("  Trap entry vaddr  : {:#x}", trap_entry_vaddr);
    println!("  Trap frame vaddr  : {:#x}", trapframe_vaddr);
    
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

    set_trampoline_trap_vector(trap_entry_vaddr);
    set_trampoline_trapframe(arch.get_cpuid(), trapframe_vaddr);
}

pub fn set_trampoline_trap_vector(trap_vector: usize) {
    unsafe {
        TRAMPOLINE_TRAP_VECTOR = Some(trap_vector);
    }
}

pub fn get_trampoline_trap_vector() -> usize {
    unsafe {
        match TRAMPOLINE_TRAP_VECTOR {
            Some(v) => v,
            None => panic!("Trampoline is not initialized"),
        }
    }
}

pub fn set_trampoline_trapframe(cpu_id: usize, trap_frame: usize) {
    unsafe {
        TRAMPOLINE_TRAPFRAME[cpu_id] = Some(trap_frame);
    }
}

pub fn get_trampoline_trapframe(cpu_id: usize) -> usize {
    unsafe {
        match TRAMPOLINE_TRAPFRAME[cpu_id] {
            Some(v) => v,
            None => panic!("Trampoline is not initialized"),
        }
    }
}