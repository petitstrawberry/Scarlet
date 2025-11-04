use core::arch::naked_asm;
use core::{arch::asm, mem::transmute};

use crate::arch::trap::print_traplog;
use crate::arch::{get_cpu, Trapframe};
use crate::println;
use crate::vm::get_kernel_vm_manager;

#[unsafe(export_name = "_kernel_trap_entry")]
#[unsafe(naked)]
pub extern "C" fn _kernel_trap_entry() {
    unsafe {
        naked_asm!("
        .option norvc
        .option norelax
        .align 8
                /* Disable the interrupt */
                csrci   sstatus, 0x2
                /* Decrease the stack pointer */
                addi    sp, sp, -280
                /* Save the context of the current hart */
                sd      x0, 0(sp)
                sd      x1, 8(sp)
                // sd      x2, 16(sp)
                sd      x3, 24(sp)
                sd      x4, 32(sp)
                sd      x5, 40(sp)
                sd      x6, 48(sp)
                sd      x7, 56(sp)
                sd      x8, 64(sp)
                sd      x9, 72(sp)
                sd      x10, 80(sp)
                sd      x11, 88(sp)
                sd      x12, 96(sp)
                sd      x13, 104(sp)
                sd      x14, 112(sp)
                sd      x15, 120(sp)
                sd      x16, 128(sp)
                sd      x17, 136(sp)
                sd      x18, 144(sp)
                sd      x19, 152(sp)
                sd      x20, 160(sp)
                sd      x21, 168(sp)
                sd      x22, 176(sp)
                sd      x23, 184(sp)
                sd      x24, 192(sp)
                sd      x25, 200(sp)
                sd      x26, 208(sp)
                sd      x27, 216(sp)
                sd      x28, 224(sp)
                sd      x29, 232(sp)
                sd      x30, 240(sp)
                sd      x31, 248(sp)
                /* Save the epc */
                csrr    t0, sepc
                sd      t0, 256(sp)

                mv      a0, sp
                call   arch_kernel_trap_handler

                /* Restore the context of the current hart */ 
                /* epc */
                ld     t0, 256(sp)
                csrw   sepc, t0
                /* Register */
                ld     x0, 0(sp)
                ld     x1, 8(sp)
                // ld     x2, 16(sp)
                ld     x3, 24(sp)
                ld     x4, 32(sp)
                ld     x5, 40(sp)
                ld     x6, 48(sp)
                ld     x7, 56(sp)
                ld     x8, 64(sp)
                ld     x9, 72(sp)
                ld     x10, 80(sp)
                ld     x11, 88(sp)
                ld     x12, 96(sp)
                ld     x13, 104(sp)
                ld     x14, 112(sp)
                ld     x15, 120(sp)
                ld     x16, 128(sp)
                ld     x17, 136(sp)
                ld     x18, 144(sp)
                ld     x19, 152(sp)
                ld     x20, 160(sp)
                ld     x21, 168(sp)
                ld     x22, 176(sp)
                ld     x23, 184(sp)
                ld     x24, 192(sp)
                ld     x25, 200(sp)
                ld     x26, 208(sp)
                ld     x27, 216(sp)
                ld     x28, 224(sp)
                ld     x29, 232(sp)
                ld     x30, 240(sp)
                ld     x31, 248(sp)

                /* Increase the stack pointer */
                addi   sp, sp, 280

                sret
            "
        );
    }
}

#[unsafe(export_name = "arch_kernel_trap_handler")]
pub extern "C" fn arch_kernel_trap_handler(addr: usize) {
    let trapframe: &mut Trapframe = unsafe { transmute(addr) };

    let cause: usize;
    unsafe {
        asm!(
            "csrr {0}, scause",
            out(reg) cause,
        );
    }

    let interrupt = cause & 0x8000000000000000 != 0;
    if interrupt {
        panic!("Interrupt is not supported in kernel mode");
    } else {
        arch_kernel_exception_handler(trapframe, cause & !0x8000000000000000);
    }
}

fn arch_kernel_exception_handler(trapframe: &mut Trapframe, cause: usize) {
    match cause {
        /* Instruction page fault */
        12 => {
            let vaddr = trapframe.epc as usize;
            let manager = get_kernel_vm_manager();
            match manager.search_memory_map(vaddr) {
                Some(mmap) => {
                    match manager.get_root_page_table() {
                        Some(root_page_table) => {
                            let paddr = mmap.get_paddr(vaddr).unwrap();
                            root_page_table.map(manager.get_asid(), vaddr, paddr, mmap.permissions);
                        }
                        None => panic!("Root page table is not found"),
                    }
                }
                None => panic!("Not found memory map matched with vaddr: {:#x}", vaddr),
            }
        }
        /* Load/Store page fault */
        13 | 15 => {
            let mut vaddr;
            unsafe {
                asm!("csrr {}, stval", out(reg) vaddr);
            }

            // For kernel addresses, check if they are valid
            let manager = get_kernel_vm_manager();
            loop {
                crate::println!("[kernel] Handling page fault at vaddr: {:#x}", vaddr);
                
                // Additional validation for suspicious addresses
                if vaddr == 0 || vaddr == usize::MAX {
                    print_traplog(trapframe);
                    panic!("Invalid memory access at vaddr: {:#x}", vaddr);
                }
                
                // Check if this is a per-core kernel stack guard page access
                // Per-core stacks: the first page of each stack area is a guard page
                #[allow(static_mut_refs)]
                {
                    use crate::environment::{STACK_SIZE, NUM_OF_CPUS};
                    use crate::mem::KERNEL_STACK;
                    let stack_start = unsafe { crate::mem::KERNEL_STACK.start() };
                    let stack_with_guard_size = STACK_SIZE + crate::environment::PAGE_SIZE;
                    
                    for cpu_id in 0..NUM_OF_CPUS {
                        let cpu_area_start = stack_start + cpu_id * stack_with_guard_size;
                        let guard_page_start = cpu_area_start;
                        let guard_page_end = guard_page_start + crate::environment::PAGE_SIZE - 1;
                        
                        if vaddr >= guard_page_start && vaddr <= guard_page_end {
                            print_traplog(trapframe);
                            panic!("KERNEL STACK OVERFLOW (CPU {}): Guard page accessed at vaddr: {:#x}\nThe per-core kernel stack has overflowed.", cpu_id, vaddr);
                        }
                    }
                }
                
                // TODO: Per-task guard page check disabled
                // Problem: Heap-allocated stacks in straight-mapped space cause conflicts
                // when guard pages are registered and memory is reused after task exit.
                // Check if this is a per-task kernel stack guard page access
                // Per-task stacks: first PAGE_SIZE bytes are guard page
                // {
                //     use crate::sched::scheduler::get_scheduler;
                //     if let Some(task) = get_scheduler().get_current_task(get_cpu().get_cpuid()) {
                //         let guard_area = task.kernel_context.get_guard_page_memory_area();
                //         if vaddr >= guard_area.start && vaddr <= guard_area.end {
                //             print_traplog(trapframe);
                //             panic!("KERNEL STACK OVERFLOW (Task {}): Guard page accessed at vaddr: {:#x}\nThe per-task kernel stack has overflowed.", task.get_id(), vaddr);
                //         }
                //     }
                // }
                
                // Check if this is a guard page in VM manager (for user stacks, etc.)
                if let Some(memory_map) = manager.search_memory_map(vaddr) {
                    if memory_map.permissions == 0 {
                        // This is a guard page
                        print_traplog(trapframe);
                        panic!("GUARD PAGE ACCESS: Guard page accessed at vaddr: {:#x}", vaddr);
                    }
                }
                
                match manager.lazy_map_page(vaddr) {
                    Ok(_) => (),
                    Err(_) => {
                        print_traplog(trapframe);
                        panic!("Not found memory map matched with kernel vaddr: {:#x}", vaddr);
                    }
                }
                crate::println!("Mapped page at vaddr: {:#x}", vaddr);

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