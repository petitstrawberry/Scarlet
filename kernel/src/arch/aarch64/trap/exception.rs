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

/// Get DAIF value
fn get_daif() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, daif", out(reg) val) };
    val
}

/// Get ISR_EL1 (pending interrupt status)
fn get_isr_el1() -> u64 {
    let val: u64;
    unsafe { asm!("mrs {}, isr_el1", out(reg) val) };
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
    /// Trapped FP/SIMD access (typically because CPACR_EL1.FPEN traps EL0).
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

    // Decode useful fields for Data/Instruction aborts.
    // ISS layout differs by EC, but WnR(bit 6) and DFSC/IFSC(bits 5:0) are consistent
    // for abort classes.
    let iss = esr & 0x01ff_ffff;
    let fsc = (iss & 0x3f) as u8;
    let wnr = ((iss >> 6) & 0x1) as u8;

    // Debug: log every trap using early_println
    let sctlr = get_sctlr_el1();

    let kind_str = match trap_kind {
        0 => "Sync",
        1 => "IRQ",
        2 => "FIQ",
        3 => "SError",
        _ => "UnknownKind",
    };

    // crate::println!(
    //     "[trap] kind={}({}) ESR={:#x} EC={:?} ISS={:#x} FSC={:#x} WnR={} FAR={:#x} ELR={:#x} SCTLR={:#x} M={} DAIF={:#x} CurrentEL={:#x}(EL{}) SPSR={:#x} SP_EL0={:#x} KernelSP={:#x} ISR_EL1={:#x}",
    //     trap_kind,
    //     kind_str,
    //     esr,
    //     ec,
    //     iss,
    //     fsc,
    //     wnr,
    //     get_far_el1(),
    //     trapframe.elr,
    //     sctlr,
    //     (sctlr & 1) as u8,
    //     get_daif(),
    //     get_current_el(),
    //     current_el_number(),
    //     trapframe.spsr,
    //     trapframe.sp,
    //     trapframe.tpidrro_el0,
    //     get_isr_el1(),
    // );

    match ec {
        // User tried to execute FP/SIMD while EL0 access is trapped.
        // Enable access for this task and restore its context, then retry.
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

        // SVC from AArch64 user mode (syscall)
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

        // Unknown exception (EC=0): undefined instruction from EL0.
        // Try to emulate PAC (Pointer Authentication) branch instructions
        // used by macOS arm64e binaries (dyld, etc.) on CPUs without ARMv8.3 support.
        ExceptionClass::Unknown => {
            if try_emulate_pac_instruction(trapframe) {
                return;
            }

            print_trap_info(trapframe, esr);
            crate::println!(
                "[trap] unhandled exception: kind={}({}) ESR={:#x} FAR={:#x} ELR={:#x}",
                trap_kind,
                kind_str,
                esr,
                get_far_el1(),
                trapframe.elr,
            );
            loop {
                unsafe { asm!("wfi") }
            }
        }

        _ => {
            print_trap_info(trapframe, esr);

            crate::println!(
                "[trap] unhandled exception: kind={}({}) ESR={:#x} FAR={:#x} ELR={:#x}",
                trap_kind,
                kind_str,
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

    // Get ESR to determine fault type
    let esr = get_esr_el1();
    let dfsc = esr & 0x3f; // Data Fault Status Code (bits 5:0)

    // Debug permission faults
    if dfsc >= 0x0d && dfsc <= 0x0f {
        early_println!(
            "[PF] PERMISSION FAULT: vaddr={:#x} write={} PC={:#x} DFSC={:#x}",
            vaddr,
            is_write,
            trapframe.get_current_pc(),
            dfsc
        );
        // Print current memory mapping for this address
        if let Some(map) = task.vm_manager.search_memory_map(vaddr) {
            early_println!(
                "[PF] Mapping found: vmarea=[{:#x}..{:#x}] perms={:#x} (R={} W={} X={} U={})",
                map.vmarea.start,
                map.vmarea.end,
                map.permissions,
                map.permissions & 0x1 != 0,
                map.permissions & 0x2 != 0,
                map.permissions & 0x4 != 0,
                map.permissions & 0x8 != 0,
            );
        } else {
            early_println!("[PF] No mapping found for vaddr={:#x}", vaddr);
        }
        // Print instruction that caused the fault
        early_println!(
            "[PF] Registers: x0={:#x} x1={:#x} x2={:#x} x3={:#x}",
            trapframe.regs.reg[0],
            trapframe.regs.reg[1],
            trapframe.regs.reg[2],
            trapframe.regs.reg[3],
        );

        // Check if this is actually a TLB issue by reading back TTBR0
        let current_ttbr0: u64;
        unsafe { asm!("mrs {}, ttbr0_el1", out(reg) current_ttbr0) };
        early_println!("[PF] Current TTBR0_EL1={:#x}", current_ttbr0);
    }

    match task.vm_manager.lazy_map_page_with(access) {
        Ok(_) => (),
        Err(e) => {
            print_trap_info(trapframe, get_esr_el1());
            if let Some(task) = get_scheduler().get_current_task(get_cpu().get_cpuid()) {
                early_println!(
                    "Task {} (PID {}) caused data fault at vaddr: {:#x} (write={}) from PC: {:#x}",
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
    let iss = esr & 0x1ffffff;
    let fsc = iss & 0x3f;

    // NOTE: Use early_println to avoid depending on heap/locking during faults.
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

/// Authenticated branch (register) instruction detection mask.
///
/// ARM64 PAC branch instructions share this encoding pattern:
///   bits [31:25] = 1101011  (unconditional branch register group)
///   bit  [24]    = 0
///   bits [15:12] = 0000
///   bits [4:0]   = 11111    (distinguishes auth from non-auth variants)
///
/// opc (bits [23:21]) identifies the operation:
///   000 = BRAA/BRAB/BRAAZ/BRABZ  (branch with auth)
///   001 = BLRAA/BLRAB/BLRAAZ/BLRABZ (branch with link and auth)
///   010 = RETAA/RETAB            (return with auth)
const PAC_BRANCH_MASK: u32 = 0xFF00F01F;
const PAC_BRANCH_VALUE: u32 = 0xD600001F;

/// Try to emulate a PAC (Pointer Authentication) instruction that trapped
/// as EC=0 (Unknown/Undefined) on a CPU without ARMv8.3 support.
///
/// We emulate authenticated branch instructions as their non-authenticated
/// equivalents (strip pointer authentication, treat as plain branch).
/// This is safe because we never actually compute PAC codes — the
/// "authenticated" pointers were never signed in the first place.
fn try_emulate_pac_instruction(trapframe: &mut Trapframe) -> bool {
    let task = match get_scheduler().get_current_task(get_cpu().get_cpuid()) {
        Some(t) => t,
        None => return false,
    };

    let elr = trapframe.elr as usize;
    let kva = match task.vm_manager.translate_to_kva(elr) {
        Some(a) => a,
        None => return false,
    };

    let instr = unsafe { *(kva as *const u32) };

    if !is_pac_branch(instr) {
        return try_emulate_pac_data_processing(trapframe, instr);
    }

    let opc = (instr >> 21) & 0x7;
    let rn = ((instr >> 16) & 0x1F) as usize;

    match opc {
        // RETAA/RETAB: return with authenticate → treat as RET (ELR = X30/LR)
        0b010 => {
            trapframe.elr = trapframe.regs.reg[30] as u64;
        }
        // BRAA/BRAB/BRAAZ/BRABZ: branch with auth → treat as BR (ELR = Xn)
        0b000 => {
            trapframe.elr = trapframe.regs.reg[rn] as u64;
        }
        // BLRAA/BLRAB/BLRAAZ/BLRABZ: branch with link and auth → treat as BLR
        0b001 => {
            trapframe.regs.reg[30] = (trapframe.elr + 4) as usize;
            trapframe.elr = trapframe.regs.reg[rn] as u64;
        }
        _ => return false,
    }

    true
}

fn is_pac_branch(instr: u32) -> bool {
    (instr & PAC_BRANCH_MASK) == PAC_BRANCH_VALUE
}

fn try_emulate_pac_data_processing(trapframe: &mut Trapframe, instr: u32) -> bool {
    // PAC data-processing instructions share bits [31:21] = 11011010110
    // 0xDAC00000 mask: all PAC register-register instructions (pacia, pacda, pacdb,
    // autia, autib, autda, autdb, paciza, xpaci, xpacd, etc.)
    if (instr & 0xFFE00000) != 0xDAC00000 {
        return false;
    }

    let opc = (instr >> 11) & 0xF;
    // PAC opc values: 0000-0100 (sign/auth), 1000 (xpac)
    // Non-PAC opcodes in this space (CRC32=0110, CRC32C=0111, etc.) won't trap
    // on cortex-a57, so we only see PAC instructions here.
    if opc > 4 && opc != 8 {
        return false;
    }

    // All PAC data-processing instructions: NOP (skip, don't modify registers).
    // Since we never compute PAC codes, pointers are already "unsigned".
    // PAC sign: leaves pointer unchanged (no code to add)
    // AUT auth: leaves pointer unchanged (nothing to strip/verify)
    // XPAC: leaves pointer unchanged (nothing to strip)
    trapframe.elr += 4;
    true
}
