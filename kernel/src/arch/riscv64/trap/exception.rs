use core::arch::asm;
use core::panic;

use crate::abi::syscall_dispatcher;
use crate::arch::trap::print_traplog;
use crate::arch::{Trapframe, get_cpu};
use crate::println;
use crate::sched::scheduler::get_scheduler;
use crate::task::mytask;

fn log_fatal_page_fault_context(
    trapframe: &Trapframe,
    cause: usize,
    vaddr: usize,
    task_id: usize,
    task_name: &str,
    asid: u16,
) {
    use crate::arch::vm::{get_root_pagetable_ptr, is_asid_used};

    let cpu_id = get_cpu().get_cpuid();
    let epc = trapframe.epc as usize;
    // RISC-V integer registers: x1=ra, x2=sp, x8=s0/fp
    let ra = trapframe.regs.reg[1] as usize;
    let sp = trapframe.regs.reg[2] as usize;
    let fp = trapframe.regs.reg[8] as usize;

    let asid_used = is_asid_used(asid);
    let root_pt = get_root_pagetable_ptr(asid).unwrap_or(core::ptr::null_mut());

    println!(
        "[Trap] fatal page fault map failed: cpu={} cause={} task_id={} name={} asid={} asid_used={} root_pt={:p}",
        cpu_id, cause, task_id, task_name, asid, asid_used, root_pt
    );
    println!(
        "[Trap] epc={:#x} vaddr={:#x} ra={:#x} sp={:#x} fp={:#x}",
        epc, vaddr, ra, sp, fp
    );
}

pub fn arch_exception_handler(trapframe: &mut Trapframe, cause: usize) {
    match cause {
        /* Environment call from U-mode */
        8 => {
            /* Execute SystemCall */
            match syscall_dispatcher(trapframe) {
                Ok(ret) => {
                    trapframe.set_return_value(ret);
                }
                Err(msg) => {
                    // panic!("Syscall error: {}", msg);
                    println!("Syscall error: {}", msg);
                    trapframe.set_return_value(usize::MAX); // Set error code: -1
                    trapframe.increment_pc_next(mytask().unwrap());
                }
            }
        }
        /* Instruction page fault */
        12 => {
            let mut vaddr = trapframe.epc as usize;
            let task = get_scheduler()
                .get_current_task(get_cpu().get_cpuid())
                .unwrap();
            use crate::object::capability::memory_mapping::{AccessKind, AccessOp};
            loop {
                let access = AccessKind {
                    op: AccessOp::Instruction,
                    vaddr,
                    size: None,
                };
                match task.vm_manager.lazy_map_page_with(access) {
                    Ok(_) => (),
                    Err(_) => {
                        print_traplog(trapframe);
                        log_fatal_page_fault_context(
                            trapframe,
                            cause,
                            vaddr,
                            task.get_id(),
                            &task.name,
                            task.vm_manager.get_asid(),
                        );
                        panic!(
                            "Failed to map page for instruction page fault at vaddr: {:#x}",
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
        /* Load/Store page fault */
        13 | 15 => {
            let mut vaddr;
            unsafe {
                asm!("csrr {}, stval", out(reg) vaddr);
            }
            let task = get_scheduler()
                .get_current_task(get_cpu().get_cpuid())
                .unwrap();
            use crate::object::capability::memory_mapping::{AccessKind, AccessOp};
            loop {
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
                match task.vm_manager.lazy_map_page_with(access) {
                    Ok(_) => (),
                    Err(_) => {
                        print_traplog(trapframe);
                        log_fatal_page_fault_context(
                            trapframe,
                            cause,
                            vaddr,
                            task.get_id(),
                            &task.name,
                            task.vm_manager.get_asid(),
                        );
                        panic!(
                            "Failed to map page for load/store page fault at vaddr: {:#x}",
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
