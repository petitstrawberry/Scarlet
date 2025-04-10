//! Virtual memory module.
//! 
//! This module provides the virtual memory abstraction for the kernel. It
//! includes functions for managing virtual address spaces.

use manager::VirtualMemoryManager;
use vmem::MemoryArea;
use vmem::VirtualMemoryMap;
use vmem::VirtualMemoryPermission;

use crate::arch::get_cpu;
use crate::arch::get_kernel_trapvector_paddr;
use crate::arch::get_user_trapvector_paddr;
use crate::arch::set_trapvector;
use crate::arch::vm::alloc_virtual_address_space;
use crate::arch::vm::get_page_table;
use crate::arch::vm::get_root_page_table_idx;
use crate::arch::Arch;
use crate::environment::KERNEL_VM_STACK_END;
use crate::environment::KERNEL_VM_STACK_SIZE;
use crate::environment::KERNEL_VM_STACK_START;
use crate::environment::NUM_OF_CPUS;
use crate::environment::PAGE_SIZE;
use crate::environment::VMMAX;
use crate::mem::page::allocate_pages;
use crate::println;
use crate::print;
use crate::sched::scheduler::get_scheduler;
use crate::task::Task;

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

static mut KERNEL_AREA: Option<MemoryArea> = None;
/* Initialize MMU and enable paging */
#[allow(static_mut_refs)]
pub fn kernel_vm_init(kernel_area: MemoryArea) {
    let manager = get_kernel_vm_manager();

    let asid = alloc_virtual_address_space(); /* Kernel ASID */
    let root_page_table_idx = get_root_page_table_idx(asid).unwrap();
    let root_page_table = get_page_table(root_page_table_idx).unwrap();
    manager.set_asid(asid);

    /* Map kernel space */
    let kernel_start = kernel_area.start;
    let kernel_end = kernel_area.end;

    let kernel_area = MemoryArea {
        start: kernel_start,
        end: kernel_end,
    };
    unsafe {
        KERNEL_AREA = Some(kernel_area);
    }

    let kernel_map = VirtualMemoryMap {
        vmarea: kernel_area,
        pmarea: kernel_area,
        permissions: 
            VirtualMemoryPermission::Read as usize |
            VirtualMemoryPermission::Write as usize |
            VirtualMemoryPermission::Execute as usize,
    };
    manager.add_memory_map(kernel_map);
    /* Pre-map the kernel space */
    root_page_table.map_memory_area(kernel_map);

    let dev_map = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: 0x00,
            end: 0x7fff_ffff,
        },
        pmarea: MemoryArea {
            start: 0x00,
            end: 0x7fff_ffff,
        },
        permissions: 
            VirtualMemoryPermission::Read as usize |
            VirtualMemoryPermission::Write as usize,
    };
    manager.add_memory_map(dev_map);

    println!("Device space mapped       : {:#018x} - {:#018x}", dev_map.vmarea.start, dev_map.vmarea.end);
    println!("Kernel space mapped       : {:#018x} - {:#018x}", kernel_start, kernel_end);

    setup_trampoline(manager);

    root_page_table.switch(manager.get_asid());
}

pub fn user_vm_init(task: &mut Task) {
    let manager = &mut task.vm_manager;

    let asid = alloc_virtual_address_space();
    manager.set_asid(asid);

    let first_page = allocate_pages(1);

    manager.add_memory_map(
        VirtualMemoryMap {
            vmarea: MemoryArea {
                start: 0x00,
                end: 0xfff,
            },
            pmarea: MemoryArea {
                start: first_page as usize,
                end: first_page as usize + 0xfff,
            },
            permissions: 
                VirtualMemoryPermission::Read as usize |
                VirtualMemoryPermission::Write as usize |
                VirtualMemoryPermission::Execute as usize,
        }
    );

    println!("User space mapped         : {:#018x} - {:#018x}", 0x00, 0xfff);
    println!("(First Page)              : {:#018x}", first_page as usize);

    setup_trampoline(manager);
}

pub fn user_kernel_vm_init(task: &mut Task) {
    let manager = &mut task.vm_manager;

    let asid = alloc_virtual_address_space();
    let root_page_table_idx = get_root_page_table_idx(asid).unwrap();
    let root_page_table = get_page_table(root_page_table_idx).unwrap();
    manager.set_asid(asid);

    let kernel_area = unsafe { KERNEL_AREA.unwrap() };

    let kernel_map = VirtualMemoryMap {
        vmarea: kernel_area,
        pmarea: kernel_area,
        permissions: 
            VirtualMemoryPermission::Read as usize |
            VirtualMemoryPermission::Write as usize |
            VirtualMemoryPermission::Execute as usize,
    };
    manager.add_memory_map(kernel_map);
    /* Pre-map the kernel space */
    root_page_table.map_memory_area(kernel_map);
    task.data_size = kernel_area.end + 1;

    let stack_pages = allocate_pages(KERNEL_VM_STACK_SIZE / PAGE_SIZE);
    let stack_map = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: KERNEL_VM_STACK_START,
            end: KERNEL_VM_STACK_END,
        },
        pmarea: MemoryArea {
            start: stack_pages as usize,
            end: stack_pages as usize + KERNEL_VM_STACK_SIZE - 1,
        },
        permissions: 
            VirtualMemoryPermission::Read as usize |
            VirtualMemoryPermission::Write as usize,
    };
    manager.add_memory_map(stack_map);
    task.stack_size = KERNEL_VM_STACK_SIZE;
    /* Pre-map the kernel stack */
    root_page_table.map_memory_area(stack_map);

    let dev_map = VirtualMemoryMap {
        vmarea: MemoryArea {
            start: 0x00,
            end: 0x7fff_ffff,
        },
        pmarea: MemoryArea {
            start: 0x00,
            end: 0x7fff_ffff,
        },
        permissions: 
            VirtualMemoryPermission::Read as usize |
            VirtualMemoryPermission::Write as usize,
    };
    manager.add_memory_map(dev_map);

    println!("Device space mapped       : {:#018x} - {:#018x}", dev_map.vmarea.start, dev_map.vmarea.end);
    println!("Kernel space mapped       : {:#018x} - {:#018x}", kernel_area.start, kernel_area.end);
    println!("Kernel stack mapped       : {:#018x} - {:#018x}", stack_map.vmarea.start as usize, stack_map.vmarea.end as usize);
    println!("(Stack page)              : {:#018x}", stack_pages as usize);

    setup_trampoline(manager);
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
        },
        permissions: 
            VirtualMemoryPermission::Read as usize |
            VirtualMemoryPermission::Write as usize |
            VirtualMemoryPermission::Execute as usize,
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

pub fn switch_to_kernel_vm() {
    let manager = get_kernel_vm_manager();
    let root_page_table = manager.get_root_page_table().expect("Root page table is not set");
    set_trapvector(get_kernel_trapvector_paddr());
    root_page_table.switch(manager.get_asid());
}

pub fn switch_to_user_vm(cpu: &mut Arch) {
    let cpu_id = cpu.get_cpuid();
    let task = get_scheduler().get_current_task(cpu_id).expect("No current task found");
    let manager = &task.vm_manager;
    let root_page_table = manager.get_root_page_table().expect("Root page table is not set");
    set_trapvector(get_trampoline_trap_vector());
    root_page_table.switch(manager.get_asid());
}