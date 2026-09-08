use core::{arch::asm, mem::transmute};

use crate::arch::trap::interrupt::arch_interrupt_handler;
use crate::arch::trap::print_traplog;
use crate::arch::{Trapframe, get_cpu};
use crate::environment::PAGE_SIZE;
use crate::object::capability::memory_mapping::{AccessKind, AccessOp};
use crate::sched::scheduler::current_task;
use crate::vm::{
    get_kernel_vm_manager,
    vmem::{MemoryAttribute, VirtualMemoryPermission},
};

#[unsafe(export_name = "_kernel_trap_entry")]
#[unsafe(naked)]
pub extern "C" fn _kernel_trap_entry() {
    unsafe {
        riscv_naked_asm!(
            "
        .option norvc
        .option norelax
        .align 8
                /* Disable the interrupt */
                csrci   sstatus, 0x2
                /* Decrease the stack pointer */
                addi    sp, sp, -SCARLET_TRAPFRAME_SIZE
                /* Save the context of the current hart */
                SCARLET_S      x0, 0*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x1, 1*SCARLET_WORD_BYTES(sp)
                // SCARLET_S      x2, 2*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x3, 3*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x4, 4*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x5, 5*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x6, 6*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x7, 7*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x8, 8*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x9, 9*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x10, 10*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x11, 11*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x12, 12*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x13, 13*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x14, 14*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x15, 15*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x16, 16*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x17, 17*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x18, 18*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x19, 19*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x20, 20*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x21, 21*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x22, 22*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x23, 23*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x24, 24*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x25, 25*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x26, 26*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x27, 27*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x28, 28*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x29, 29*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x30, 30*SCARLET_WORD_BYTES(sp)
                SCARLET_S      x31, 31*SCARLET_WORD_BYTES(sp)
                /* Save the epc */
                csrr    t0, sepc
                SCARLET_S      t0, 32*SCARLET_WORD_BYTES(sp)

                mv      a0, sp
                call   arch_kernel_trap_handler

                /* Restore the context of the current hart */ 
                /* epc */
                SCARLET_L     t0, 32*SCARLET_WORD_BYTES(sp)
                csrw   sepc, t0
                /* Register */
                SCARLET_L     x0, 0*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x1, 1*SCARLET_WORD_BYTES(sp)
                // SCARLET_L     x2, 2*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x3, 3*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x4, 4*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x5, 5*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x6, 6*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x7, 7*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x8, 8*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x9, 9*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x10, 10*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x11, 11*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x12, 12*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x13, 13*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x14, 14*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x15, 15*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x16, 16*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x17, 17*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x18, 18*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x19, 19*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x20, 20*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x21, 21*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x22, 22*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x23, 23*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x24, 24*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x25, 25*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x26, 26*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x27, 27*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x28, 28*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x29, 29*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x30, 30*SCARLET_WORD_BYTES(sp)
                SCARLET_L     x31, 31*SCARLET_WORD_BYTES(sp)

                /* Increase the stack pointer */
                addi   sp, sp, SCARLET_TRAPFRAME_SIZE

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

    let interrupt = cause & (1usize << (usize::BITS - 1)) != 0;
    if interrupt {
        arch_interrupt_handler(trapframe, cause & !(1usize << (usize::BITS - 1)));
    } else {
        arch_kernel_exception_handler(trapframe, cause & !(1usize << (usize::BITS - 1)));
    }
}

fn arch_kernel_exception_handler(trapframe: &mut Trapframe, cause: usize) {
    match cause {
        /* Instruction page fault */
        12 => {
            let vaddr = trapframe.epc as usize;
            crate::println!("[kernel trap] inst page fault at {:#x}", vaddr);
            let manager = get_kernel_vm_manager();
            match manager.search_memory_map(vaddr) {
                Some(mmap) => match manager.get_root_page_table() {
                    Some(mut root_page_table) => {
                        let paddr = mmap.pmarea.start + (vaddr - mmap.vmarea.start);
                        root_page_table.map(
                            vaddr,
                            paddr,
                            mmap.permissions,
                            mmap.memory_attribute,
                            true,
                            false,
                        );
                    }
                    None => panic!("Root page table is not found"),
                },
                None => panic!("Not found memory map matched with vaddr: {:#x}", vaddr),
            }
        }
        /* Load/Store page fault */
        13 | 15 => {
            let mut vaddr;
            unsafe {
                asm!("csrr {}, stval", out(reg) vaddr);
            }

            // Detect kernel stack overflow via guard-page hit
            // Also handle kstack window accesses that might not have VMA
            if let Some(task) = current_task(get_cpu().get_cpuid()) {
                if let Some((_slot, base)) = task.get_kernel_stack_window_base() {
                    let kstack_start = base + crate::environment::PAGE_SIZE;
                    let kstack_end = kstack_start + crate::environment::TASK_KERNEL_STACK_SIZE;

                    // Guard page hit
                    if vaddr >= base && vaddr < kstack_start {
                        print_traplog(trapframe);
                        panic!(
                            "Kernel stack overflow detected: guard page hit at vaddr={:#x} (base={:#x})",
                            vaddr, base
                        );
                    }

                    // Kstack window access - map it directly
                    // Also handle kernel_sp (one past end) since trap entry might touch it
                    if vaddr >= kstack_start && vaddr <= kstack_end {
                        let manager = get_kernel_vm_manager();
                        if let Some(mut root_page_table) = manager.get_root_page_table() {
                            let kernel_stack_area = task.get_kernel_stack_memory_area_paddr();
                            let page_offset =
                                (vaddr - kstack_start) & !(crate::environment::PAGE_SIZE - 1);
                            let page_paddr = kernel_stack_area.start + page_offset;
                            let page_vaddr = vaddr & !(crate::environment::PAGE_SIZE - 1);
                            root_page_table.map(
                                page_vaddr,
                                page_paddr,
                                VirtualMemoryPermission::Read as usize
                                    | VirtualMemoryPermission::Write as usize,
                                MemoryAttribute::Normal,
                                true,
                                false,
                            );
                            return;
                        }
                    }
                }
            }

            // For kernel addresses, check if they are valid
            let manager = get_kernel_vm_manager();
            loop {
                // Additional validation for suspicious addresses
                if vaddr == 0 || vaddr == usize::MAX {
                    print_traplog(trapframe);
                    panic!("Invalid memory access at vaddr: {:#x}", vaddr);
                }

                let op = if cause == 13 {
                    AccessOp::Load
                } else {
                    AccessOp::Store
                };
                let access = AccessKind {
                    op,
                    vaddr,
                    size: None,
                };

                match manager.lazy_map_page_with(access) {
                    Ok(_) => (),
                    Err(_) => {
                        print_traplog(trapframe);
                        panic!(
                            "Not found memory map matched with kernel vaddr: {:#x}",
                            vaddr
                        );
                    }
                }

                if vaddr & 0b11 == 0 {
                    // If the address is aligned, we can stop
                    break;
                }
                vaddr = (vaddr + 4) & !0b11; // Align to the next 4-byte boundary
            }
        }
        _ => {
            print_traplog(trapframe);
            panic!("Unhandled exception: {}", cause);
        }
    }
}
