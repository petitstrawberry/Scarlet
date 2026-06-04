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
use crate::sched::scheduler::current_task;
use crate::task::mytask;

/// Get CurrentEL value
fn get_current_el() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, CurrentEL", out(reg) val) };
    val
}

fn current_el_number() -> u64 {
    // CurrentEL encodes the exception level in bits [3:2] as EL<<2.
    // Return the EL number (0-3).
    (get_current_el() >> 2) & 0x3
}

fn get_hcr_el2() -> u64 {
    if current_el_number() != 2 {
        return 0;
    }

    let val: u64;
    unsafe { asm!("mrs {}, hcr_el2", out(reg) val) };
    val
}

/// Get DAIF value
fn get_daif() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, daif", out(reg) val) };
    val
}

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
    /// Trapped FP/SIMD access (typically because CPACR_EL1.FPEN traps EL0).
    FpSimdAccess = 0x07,
    SvcAarch64 = 0x15,
    /// Trapped SVE access (typically because CPACR_EL1.ZEN traps EL0).
    SveAccess = 0x19,
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
            0x15 => ExceptionClass::SvcAarch64,
            0x19 => ExceptionClass::SveAccess,
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

    // Decode useful fields for Data/Instruction aborts.
    // ISS layout differs by EC, but WnR(bit 6) and DFSC/IFSC(bits 5:0) are consistent
    // for abort classes.
    let iss = esr & 0x01ff_ffff;
    let _fsc = (iss & 0x3f) as u8;
    let _wnr = ((iss >> 6) & 0x1) as u8;

    let kind_str = match trap_kind {
        0 => "Sync",
        1 => "IRQ",
        2 => "FIQ",
        3 => "SError",
        _ => "UnknownKind",
    };

    match ec {
        // User tried to execute FP/SIMD while EL0 access is trapped.
        // Enable access for this task and restore its context, then retry.
        ExceptionClass::FpSimdAccess => {
            if crate::arch::user_fpu_enabled() {
                let cpu_id = get_cpu().get_cpuid();
                let task = current_task(cpu_id).unwrap();
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

        ExceptionClass::SveAccess => {
            if !handle_sve_access_trap(trapframe) {
                let instr_at_elr = read_user_instruction(trapframe.elr as usize);
                let instr_before_elr =
                    read_user_instruction((trapframe.elr as usize).wrapping_sub(4));
                print_trap_info(trapframe, esr);
                crate::println!(
                    "[trap] unsupported SVE access trap at ELR={:#x}; instr@ELR={:?} instr@ELR-4={:?}; SVE context is not implemented",
                    trapframe.elr,
                    instr_at_elr,
                    instr_before_elr,
                );
                loop {
                    unsafe { asm!("wfi") }
                }
            }
        }

        // SVC from AArch64 user mode (syscall)
        ExceptionClass::SvcAarch64 => match syscall_dispatcher(trapframe) {
            Ok(ret) => {
                trapframe.set_return_value(ret);
                crate::sched::scheduler::process_pending_events_before_user_return(trapframe);
            }
            Err(msg) => {
                println!("Syscall error: {}", msg);
                trapframe.set_return_value(usize::MAX);
                trapframe.increment_pc_next(mytask().unwrap());
                crate::sched::scheduler::process_pending_events_before_user_return(trapframe);
            }
        },

        // Instruction abort from lower EL
        ExceptionClass::InstructionAbortLowerEl => {
            let vaddr = trapframe.elr as usize;
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
            crate::println!("Kernel instruction abort at FAR={:#x}", far);
            loop {
                unsafe { asm!("wfi") }
            }
        }

        // Data abort from same EL (kernel bug)
        ExceptionClass::DataAbortSameEl => {
            let far = get_far_el1();
            print_trap_info(trapframe, esr);
            crate::println!("Kernel data abort at FAR={:#x}", far);
            loop {
                unsafe { asm!("wfi") }
            }
        }

        // Unknown or unhandled exception.
        // We must stop here: this indicates a real bring-up bug (e.g. unexpected
        // asynchronous exception path) and masking would hide the root cause.
        _ => {
            print_trap_info(trapframe, esr);

            crate::println!(
                "[trap] unhandled exception: kind={}({}) ESR={:#x} FAR={:#x} ELR={:#x} CurrentEL=EL{} SPSR={:#x} DAIF={:#x} HCR_EL2={:#x}",
                trap_kind,
                kind_str,
                esr,
                get_far_el1(),
                trapframe.elr,
                current_el_number(),
                trapframe.spsr,
                get_daif(),
                get_hcr_el2(),
            );

            loop {
                unsafe { asm!("wfi") }
            }
        }
    }
}

/// Handle a trapped SVE instruction without enabling general SVE execution.
///
/// Scarlet does not currently save/restore SVE Z/P/FFR state. Enabling SVE for
/// EL0 would therefore corrupt task state across context switches. Some AArch64
/// libgcc unwinder paths still use `CNTD Xt` to query the vector granule count,
/// so emulate that scalar query narrowly and leave all other SVE instructions
/// unsupported.
fn handle_sve_access_trap(trapframe: &mut Trapframe) -> bool {
    let instr_addr = trapframe.elr as usize;
    let instr = match read_user_instruction(instr_addr) {
        Some(instr) => instr,
        None => return false,
    };

    // CNTD Xt is encoded as 0x04e0e3e0 | Rt for the all-pattern variant used by
    // libgcc's AArch64 unwinder. Return the architectural minimum SVE vector
    // length expressed in doublewords: 128 bits / 64 bits = 2.
    if instr & 0xffff_ffe0 == 0x04e0_e3e0 {
        let rt = (instr & 0x1f) as usize;
        if rt < trapframe.regs.reg.len() {
            trapframe.regs.reg[rt] = 2;
        }
        trapframe.elr = trapframe.elr.wrapping_add(4);
        return true;
    }

    false
}

fn read_user_instruction(instr_addr: usize) -> Option<u32> {
    let task = current_task(get_cpu().get_cpuid()).unwrap();
    let instr_kva = task.vm_manager.translate_to_kva(instr_addr)?;
    Some(unsafe { core::ptr::read_unaligned(instr_kva as *const u32) })
}

/// Handle instruction page fault (like RISC-V cause 12)
fn handle_instruction_fault(trapframe: &mut Trapframe, vaddr: usize) {
    let task = current_task(get_cpu().get_cpuid()).unwrap();

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
    let task = current_task(get_cpu().get_cpuid()).unwrap();
    let pc = trapframe.get_current_pc();

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

    match task.vm_manager.lazy_map_page_with(access) {
        Ok(_) => (),
        Err(e) => {
            print_trap_info(trapframe, get_esr_el1());
            if let Some(task) = current_task(get_cpu().get_cpuid()) {
                println!(
                    "Task {} (PID {}) caused data fault at vaddr: {:#x} (write={}) from PC: {:#x}",
                    task.name.read(),
                    task.get_id(),
                    vaddr,
                    is_write,
                    pc
                );
            }
            panic!(
                "Failed to map page for data fault at vaddr: {:#x} (write={}) from PC: {:#x}: {}",
                vaddr, is_write, pc, e
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

    crate::println!("=== Trap Info ===");
    crate::println!("ESR_EL1: {:#018x} (EC={:#x}, FSC={:#x})", esr, ec, fsc);
    crate::println!("FAR_EL1: {:#018x}", far);
    crate::println!("ELR_EL1: {:#018x}", trapframe.elr);

    // Print first 8 general registers
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
}
