use core::arch::naked_asm;
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
        naked_asm!(
            "
        .option norvc
        .option norelax
        .align 8
                /* Disable the interrupt */
                csrci   sstatus, 0x2
                /* Decrease the stack pointer */
                addi    sp, sp, -288
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
                addi   sp, sp, 288

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
        arch_interrupt_handler(trapframe, cause & !0x8000000000000000);
    } else {
        arch_kernel_exception_handler(trapframe, cause & !0x8000000000000000);
    }
}

fn arch_kernel_exception_handler(trapframe: &mut Trapframe, cause: usize) {
    match cause {
        /* Instruction page fault */
        12 => {
            let vaddr = trapframe.epc as usize;
            crate::early_println!("[kernel trap] inst page fault at {:#x}", vaddr);
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
