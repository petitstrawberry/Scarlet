//! AArch64 exception handler
//!
//! Strategy follows RISC-V implementation: simple cause-based matching
//! with lazy page mapping on page faults.

use core::arch::asm;
use core::panic;

use crate::abi::syscall_dispatcher;
use crate::arch::{Trapframe, get_cpu};
use crate::object::capability::memory_mapping::{AccessKind, AccessOp};
use crate::sched::scheduler::get_scheduler;
use crate::task::mytask;
use crate::{early_println, println};

use super::emulator;

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

/// Get SCTLR_EL1 value
fn get_sctlr_el1() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, sctlr_el1", out(reg) val) };
    val
}

/// Exception Class (ESR_EL1[31:26])
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum ExceptionClass {
    Unknown = 0x00,
    TrappedSystemReg = 0x08,
    FpSimdAccess = 0x07,
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
            0x07 => ExceptionClass::FpSimdAccess,
            0x08 => ExceptionClass::TrappedSystemReg,
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
pub fn arch_exception_handler(trapframe: &mut Trapframe, trap_kind: usize) {
    let ec = ExceptionClass::from(trapframe.esr_el1);
    let esr = trapframe.esr_el1;

    match ec {
        ExceptionClass::FpSimdAccess => {
            if crate::arch::user_fpu_enabled() {
                let cpu_id = get_cpu().get_cpuid();
                let task = get_scheduler().get_current_task(cpu_id).unwrap();
                task.vcpu.lock().fpu_used = true;
                crate::arch::fpu::set_user_fpu_enabled(true);
                unsafe {
                    task.vcpu.lock().fpu.restore();
                }
                return;
            }

            print_trap_info(trapframe, esr);
            panic!("FP/SIMD is disabled by build config or DTB");
        }

        ExceptionClass::SvcAarch64 => match syscall_dispatcher(trapframe) {
            Ok(ret) => {
                trapframe.set_return_value(ret);
            }
            Err(msg) => {
                println!("Syscall error: {}", msg);
                trapframe.set_return_value(usize::MAX);
                trapframe.increment_pc_next(mytask().unwrap());
            }
        },

        ExceptionClass::InstructionAbortLowerEl => {
            let vaddr = trapframe.elr as usize;
            handle_instruction_fault(trapframe, vaddr);
        }

        ExceptionClass::DataAbortLowerEl => {
            let far = get_far_el1() as usize;
            let is_write = (esr >> 6) & 1 == 1;
            handle_data_fault(trapframe, far, is_write);
        }

        ExceptionClass::InstructionAbortSameEl => {
            let far = get_far_el1();
            print_trap_info(trapframe, esr);
            crate::println!("Kernel instruction abort at FAR={:#x}", far);
            loop {
                unsafe { asm!("wfi") }
            }
        }

        ExceptionClass::DataAbortSameEl => {
            let far = get_far_el1();
            print_trap_info(trapframe, esr);
            crate::println!("Kernel data abort at FAR={:#x}", far);
            loop {
                unsafe { asm!("wfi") }
            }
        }

        // Unknown (EC=0) or Trapped system register (EC=8):
        // cortex-a57 traps PAC/LSE/RCpc instructions here.
        ExceptionClass::Unknown | ExceptionClass::TrappedSystemReg => {
            if emulator::try_emulate_instruction(trapframe) {
                return;
            }
            print_trap_info(trapframe, esr);
            crate::println!(
                "[trap] unhandled: ESR={:#x} EC={:?} FAR={:#x} ELR={:#x}",
                esr,
                ec,
                get_far_el1(),
                trapframe.elr,
            );
            loop {
                unsafe { asm!("wfi") }
            }
        }

        _ => {
            let ec_raw = ((esr >> 26) & 0x3f) as u8;

            // EC=0 or EC=8 in the catch-all (shouldn't normally happen, but handle defensively)
            if ec_raw == 0x00 || ec_raw == 0x08 {
                if emulator::try_emulate_instruction(trapframe) {
                    return;
                }
            }

            // BRK from userspace (EC=0x30 or EC=0x3C)
            if ec_raw == 0x30 || ec_raw == 0x3c {
                handle_brk(trapframe, esr);
                return;
            }

            print_trap_info(trapframe, esr);
            crate::println!(
                "[trap] unhandled: kind={} ESR={:#x} FAR={:#x} ELR={:#x}",
                trap_kind,
                esr,
                get_far_el1(),
                trapframe.elr,
            );
            loop {
                unsafe { asm!("wfi") }
            }
        }
    }
}

fn handle_brk(trapframe: &mut Trapframe, esr: u64) {
    let imm = (esr & 0xFFFF) as u32;
    let task = mytask().unwrap();

    if imm == 0xb001 {
        let x0 = trapframe.regs.reg[0];
        let x1 = trapframe.regs.reg[1];
        let lr = trapframe.regs.reg[30];
        crate::println!("[darwin] dyld halt: x0={:#x} x1={:#x} LR={:#x}", x0, x1, lr);

        if x1 > 0x1000 {
            if let Some(kva) = task.vm_manager.translate_to_kva(x1) {
                let bytes = unsafe { core::slice::from_raw_parts(kva as *const u8, 256) };
                crate::print!("[darwin] halt msg=\"");
                for &b in bytes.iter() {
                    if b >= 0x20 && b < 0x7f {
                        crate::print!("{}", b as char);
                    } else {
                        break;
                    }
                }
                crate::println!("\"");
            } else {
                crate::println!("[darwin] halt msg: could not translate x1={:#x}", x1);
            }
        }

        if x0 > 0x1000 && x0 != x1 {
            if let Some(kva) = task.vm_manager.translate_to_kva(x0) {
                let bytes = unsafe { core::slice::from_raw_parts(kva as *const u8, 256) };
                crate::print!("[darwin] halt x0-str=\"");
                for &b in bytes.iter() {
                    if b >= 0x20 && b < 0x7f {
                        crate::print!("{}", b as char);
                    } else {
                        break;
                    }
                }
                crate::println!("\"");
            }
        }

        trapframe.increment_pc_next(task);
        task.exit(128 + 6);
        return;
    }

    let x0 = trapframe.regs.reg[0];
    let x1 = trapframe.regs.reg[1];
    let lr = trapframe.regs.reg[30];
    let x8 = trapframe.regs.reg[8];
    crate::println!("[darwin] dyld BRK #{:#x} at ELR={:#x}", imm, trapframe.elr);
    crate::println!(
        "[darwin]   x0={:#x} x1={:#x} LR={:#x} x8={:#x}",
        x0,
        x1,
        lr,
        x8
    );

    let task = mytask().unwrap();

    crate::println!(
        "[darwin]   TPIDRRO_EL0={:#x} tpidr_el0={:#x}",
        trapframe.tpidrro_el0,
        trapframe.tpidr_el0
    );
    let tpidrro = trapframe.tpidrro_el0 as usize;
    if tpidrro > 0x1000 {
        if let Some(kva) = task.vm_manager.translate_to_kva(tpidrro) {
            let slot0 = unsafe { core::ptr::read(kva as *const u64) };
            let slot3 = unsafe { core::ptr::read((kva as *const u64).add(3)) };
            crate::println!("[darwin]   TLS[0]={:#x} TLS[3]={:#x}", slot0, slot3);
        }
    }

    let str_ptr = x1;
    if str_ptr > 0x1000 {
        if let Some(kva) = task.vm_manager.translate_to_kva(str_ptr) {
            let bytes = unsafe { core::slice::from_raw_parts(kva as *const u8, 200) };
            crate::print!("[darwin]   msg=\"");
            for &b in bytes.iter() {
                if b >= 0x20 && b < 0x7f {
                    crate::print!("{}", b as char);
                } else {
                    break;
                }
            }
            crate::println!("\"");
        }
    }

    trapframe.increment_pc_next(task);
}

/// Handle instruction page fault (like RISC-V cause 12)
fn handle_instruction_fault(trapframe: &mut Trapframe, vaddr: usize) {
    let task = get_scheduler()
        .get_current_task(get_cpu().get_cpuid())
        .unwrap();

    let access = AccessKind {
        op: AccessOp::Instruction,
        vaddr,
        size: None,
    };

    match task.vm_manager.lazy_map_page_with(access) {
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

    let esr = get_esr_el1();
    let dfsc = esr & 0x3f;

    if dfsc >= 0x0d && dfsc <= 0x0f {
        early_println!(
            "[PF] PERMISSION FAULT: vaddr={:#x} write={} PC={:#x} DFSC={:#x}",
            vaddr,
            is_write,
            trapframe.get_current_pc(),
            dfsc
        );
        if let Some(map) = task.vm_manager.search_memory_map(vaddr) {
            early_println!(
                "[PF] vmarea=[{:#x}..{:#x}] perms={:#x} (R={} W={} X={} U={})",
                map.vmarea.start,
                map.vmarea.end,
                map.permissions,
                map.permissions & 0x1 != 0,
                map.permissions & 0x2 != 0,
                map.permissions & 0x4 != 0,
                map.permissions & 0x8 != 0,
            );
        } else {
            early_println!("[PF] No mapping for vaddr={:#x}", vaddr);
        }
    }

    match task.vm_manager.lazy_map_page_with(access) {
        Ok(_) => (),
        Err(e) => {
            print_trap_info(trapframe, get_esr_el1());
            if let Some(task) = get_scheduler().get_current_task(get_cpu().get_cpuid()) {
                early_println!(
                    "Task {} (PID {}) data fault at vaddr: {:#x} (write={}) from PC: {:#x}",
                    task.name.read(),
                    task.get_id(),
                    vaddr,
                    is_write,
                    trapframe.get_current_pc()
                );
            }
            panic!(
                "Failed to map page for data fault at vaddr: {:#x} (write={}) from PC: {:#x}",
                vaddr,
                is_write,
                trapframe.get_current_pc()
            );
        }
    }
}

/// Print trap information for debugging
fn print_trap_info(trapframe: &Trapframe, esr: u64) {
    let far = get_far_el1();
    let ec = (esr >> 26) & 0x3f;
    let fsc = esr & 0x3f;

    crate::println!("=== Trap Info ===");
    crate::println!("ESR_EL1: {:#018x} (EC={:#x}, FSC={:#x})", esr, ec, fsc);
    crate::println!("FAR_EL1: {:#018x}", far);
    crate::println!("ELR_EL1: {:#018x}", trapframe.elr);
    crate::println!(
        "x0={:#x} x1={:#x} x2={:#x} x3={:#x}",
        trapframe.regs.reg[0],
        trapframe.regs.reg[1],
        trapframe.regs.reg[2],
        trapframe.regs.reg[3]
    );
    crate::println!(
        "x4={:#x} x5={:#x} x6={:#x} x7={:#x}",
        trapframe.regs.reg[4],
        trapframe.regs.reg[5],
        trapframe.regs.reg[6],
        trapframe.regs.reg[7]
    );
    crate::println!("TPIDR_EL0={:#x}", trapframe.tpidr_el0);
    crate::println!(
        "x8={:#x} x9={:#x}",
        trapframe.regs.reg[8],
        trapframe.regs.reg[9]
    );
}
