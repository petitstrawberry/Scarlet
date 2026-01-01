//! AArch64 exception handler
//!
//! Strategy follows RISC-V implementation: simple cause-based matching
//! with lazy page mapping on page faults.

use core::arch::asm;
use core::panic;

use crate::abi::syscall_dispatcher;
use crate::arch::{Trapframe, get_cpu};
use crate::object::capability::memory_mapping::{AccessKind, AccessOp};
use crate::println;
use crate::sched::scheduler::get_scheduler;
use crate::task::mytask;

/// Get ESR_EL1 value
fn get_esr_el1() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, esr_el1", out(reg) val) };
    val
}

/// Get FAR_EL1 value  
fn get_far_el1() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, far_el1", out(reg) val) };
    val
}

/// Exception Class (ESR_EL1[31:26])
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum ExceptionClass {
    Unknown = 0x00,
    SvcAarch64 = 0x15,
    InstructionAbortLowerEl = 0x20,
    InstructionAbortSameEl = 0x21,
    DataAbortLowerEl = 0x24,
    DataAbortSameEl = 0x25,
    Other = 0xFF,
}

impl From<u64> for ExceptionClass {
    fn from(val: u64) -> Self {
        let ec = ((val >> 26) & 0x3f) as u8;
        match ec {
            0x00 => ExceptionClass::Unknown,
            0x15 => ExceptionClass::SvcAarch64,
            0x20 => ExceptionClass::InstructionAbortLowerEl,
            0x21 => ExceptionClass::InstructionAbortSameEl,
            0x24 => ExceptionClass::DataAbortLowerEl,
            0x25 => ExceptionClass::DataAbortSameEl,
            _ => ExceptionClass::Other,
        }
    }
}

/// Main exception handler
pub fn arch_exception_handler(trapframe: &mut Trapframe) {
    let esr = get_esr_el1();
    let ec = ExceptionClass::from(esr);

    // Debug: log every trap using early_println
    crate::early_println!(
        "[trap] ESR={:#x} EC={:?} FAR={:#x} ELR={:#x}",
        esr,
        ec,
        get_far_el1(),
        trapframe.epc
    );

    match ec {
        // SVC from AArch64 user mode (syscall)
        ExceptionClass::SvcAarch64 => {
            // Minimal syscall trace for debugging AArch64 SVC path.
            // AArch64 syscall number: x8, args: x0-x5.
            crate::early_println!(
                "[syscall/aarch64] nr={} x0={:#x} x1={:#x} x2={:#x} x3={:#x} x4={:#x} x5={:#x} sp={:#x} elr={:#x}",
                trapframe.get_syscall_number(),
                trapframe.get_arg(0),
                trapframe.get_arg(1),
                trapframe.get_arg(2),
                trapframe.get_arg(3),
                trapframe.get_arg(4),
                trapframe.get_arg(5),
                trapframe.sp,
                trapframe.epc,
            );
            // panic!("AArch64 syscall handler not implemented");
            match syscall_dispatcher(trapframe) {
                Ok(ret) => {
                    crate::early_println!("[syscall/aarch64] -> ret={:#x}", ret);
                    trapframe.set_return_value(ret);
                }
                Err(msg) => {
                    println!("Syscall error: {}", msg);
                    trapframe.set_return_value(usize::MAX);
                    trapframe.increment_pc_next(mytask().unwrap());
                }
            }
        }

        // Instruction abort from lower EL
        ExceptionClass::InstructionAbortLowerEl => {
            let vaddr = trapframe.epc as usize;
            handle_instruction_fault(trapframe, vaddr);
        }

        // Data abort from lower EL
        ExceptionClass::DataAbortLowerEl => {
            let far = get_far_el1() as usize;
            let is_write = (esr >> 6) & 1 == 1; // WnR bit
            handle_data_fault(trapframe, far, is_write);
        }

        // Instruction abort from same EL (kernel bug)
        ExceptionClass::InstructionAbortSameEl => {
            let far = get_far_el1();
            print_trap_info(trapframe, esr);
            panic!("Kernel instruction abort at FAR={:#x}", far);
        }

        // Data abort from same EL (kernel bug)
        ExceptionClass::DataAbortSameEl => {
            let far = get_far_el1();
            print_trap_info(trapframe, esr);
            panic!("Kernel data abort at FAR={:#x}", far);
        }

        // Unknown or unhandled exception
        _ => {
            print_trap_info(trapframe, esr);
            panic!("Unhandled exception: EC={:#x}", (esr >> 26) & 0x3f);
        }
    }
}

/// Handle instruction page fault (like RISC-V cause 12)
fn handle_instruction_fault(trapframe: &mut Trapframe, vaddr: usize) {
    let task = get_scheduler()
        .get_current_task(get_cpu().get_cpuid())
        .unwrap();
    let manager = &mut task.vm_manager;

    let access = AccessKind {
        op: AccessOp::Instruction,
        vaddr,
        size: None,
    };

    match manager.lazy_map_page_with(access) {
        Ok(_) => (),
        Err(_) => {
            print_trap_info(trapframe, get_esr_el1());
            panic!(
                "Failed to map page for instruction fault at vaddr: {:#x}",
                vaddr
            );
        }
    }
}

/// Handle data page fault (like RISC-V cause 13/15)
fn handle_data_fault(trapframe: &mut Trapframe, vaddr: usize, is_write: bool) {
    let task = get_scheduler()
        .get_current_task(get_cpu().get_cpuid())
        .unwrap();
    let manager = &mut task.vm_manager;

    let op = if is_write {
        AccessOp::Store
    } else {
        AccessOp::Load
    };

    let access = AccessKind {
        op,
        vaddr,
        size: None,
    };

    match manager.lazy_map_page_with(access) {
        Ok(_) => (),
        Err(_) => {
            print_trap_info(trapframe, get_esr_el1());
            panic!(
                "Failed to map page for data fault at vaddr: {:#x} (write={})",
                vaddr, is_write
            );
        }
    }
}

/// Print trap information for debugging
fn print_trap_info(trapframe: &Trapframe, esr: u64) {
    let far = get_far_el1();
    let ec = (esr >> 26) & 0x3f;
    let iss = esr & 0x1ffffff;
    let fsc = iss & 0x3f;

    println!("=== Trap Info ===");
    println!("ESR_EL1: {:#018x} (EC={:#x}, FSC={:#x})", esr, ec, fsc);
    println!("FAR_EL1: {:#018x}", far);
    println!("ELR_EL1: {:#018x}", trapframe.epc);

    // Print first 8 general registers
    println!(
        "x0={:#x} x1={:#x} x2={:#x} x3={:#x}",
        trapframe.regs.reg[0], trapframe.regs.reg[1], trapframe.regs.reg[2], trapframe.regs.reg[3]
    );
    println!(
        "x4={:#x} x5={:#x} x6={:#x} x7={:#x}",
        trapframe.regs.reg[4], trapframe.regs.reg[5], trapframe.regs.reg[6], trapframe.regs.reg[7]
    );
}
