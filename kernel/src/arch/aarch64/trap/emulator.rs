//! Instruction emulator for ARM extensions not supported by all CPUs.
//!
//! Handles:
//! - PAC (Pointer Authentication, ARMv8.3) → TODO: disabled; requires native PAC hardware
//!   or a complete software model. Re-enable when running on non-PAC hardware (e.g. Pi 4/5).
//! - LSE atomics (CAS, LDADD, SWP, ARMv8.1) → non-atomic load/store
//! - LDAPR/STLLR (LO-acquire/LO-release, ARMv8.3-RCpc / ARMv8.4) → plain LDR/STR

use core::arch::asm;

use crate::arch::{Trapframe, get_cpu};
use crate::println;
use crate::sched::scheduler::get_scheduler;

// --- PAC branch instruction detection ---

const PAC_BRANCH_MASK: u32 = 0xFE00F000;
const PAC_BRANCH_VALUE: u32 = 0xD6000000;

// --- Top-level entry point ---

/// Try to emulate an undefined instruction that trapped as EC=0 (Unknown)
/// or EC=8 (Trapped system register) on cortex-a57.
///
/// Returns true if emulation succeeded and ELR has been advanced.
pub fn try_emulate_instruction(trapframe: &mut Trapframe) -> bool {
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

    static ENTRY_COUNT: spin::Mutex<usize> = spin::Mutex::new(0);
    {
        let mut c = ENTRY_COUNT.lock();
        *c += 1;
        if *c <= 50 {
            println!(
                "[emulator] entry #{}: instr={:#010x} elr={:#x}",
                *c, instr, elr
            );
        }
    }

    // TODO: PAC emulator disabled — requires native PAC hardware (-cpu max)
    // or a complete software model. Re-enable for non-PAC hardware (e.g. Pi 4/5).
    // if emulate_pac_branch(trapframe, instr) {
    //     if instr == 0xd503237f {
    //         static PACIBSP_COUNT: spin::Mutex<usize> = spin::Mutex::new(0);
    //         let mut c = PACIBSP_COUNT.lock();
    //         *c += 1;
    //         if *c <= 3 { println!("[emulator] pacibsp emulated as nop, count={}", *c); }
    //     }
    //     return true;
    // }
    // if emulate_pac_data_processing(trapframe, instr) {
    //     return true;
    // }
    if emulate_rcpc(trapframe, instr, &task) {
        return true;
    }
    if emulate_lse_atomic(trapframe, instr, &task) {
        return true;
    }

    static EMU_COUNT: spin::Mutex<usize> = spin::Mutex::new(0);
    let mut c = EMU_COUNT.lock();
    *c += 1;
    if *c <= 5 || instr != 0xd503237f {
        println!(
            "[emulator] unhandled instr={:#010x} elr={:#x} total_unhandled={}",
            instr, elr, *c
        );
    }
    false
}

// --- PAC authenticated branches ---

fn strip_pac(ptr: u64) -> u64 {
    if ptr & (1u64 << 55) != 0 {
        ptr | 0xFFFF_0000_0000_0000
    } else {
        ptr & 0x0000_FFFF_FFFF_FFFF
    }
}

fn emulate_pac_branch(trapframe: &mut Trapframe, instr: u32) -> bool {
    if (instr & PAC_BRANCH_MASK) != PAC_BRANCH_VALUE {
        return false;
    }
    // Exclude standard non-auth BR/BLR/RET
    let o1 = (instr >> 24) & 1;
    if o1 == 0 && (instr >> 10) & 3 == 3 && instr & 0x1F == 0 {
        return false;
    }
    let opc = (instr >> 21) & 0x7;
    if !matches!(opc, 0b000 | 0b001 | 0b010) {
        return false;
    }

    let rn = if o1 == 0 {
        ((instr >> 16) & 0x1F) as usize
    } else {
        ((instr >> 5) & 0x1F) as usize
    };

    match opc {
        0b010 => {
            // RETAA/RETAB → strip PAC from LR, then RET
            trapframe.elr = strip_pac(trapframe.regs.reg[30] as u64);
        }
        0b000 => {
            // BRAA/BRAB/BRAAZ/BRABZ → strip PAC from Rn, then BR
            trapframe.elr = strip_pac(trapframe.regs.reg[rn] as u64);
        }
        0b001 => {
            // BLRAA/BLRAB/BLRAAZ/BLRABZ → strip PAC from Rn, then BLR
            trapframe.regs.reg[30] = (trapframe.elr + 4) as usize;
            trapframe.elr = strip_pac(trapframe.regs.reg[rn] as u64);
        }
        _ => return false,
    }
    true
}

// --- PAC data processing (sign/auth/xpac) ---

fn emulate_pac_data_processing(trapframe: &mut Trapframe, instr: u32) -> bool {
    if (instr & 0xFFE00000) != 0xDAC00000 {
        return false;
    }
    let opc = (instr >> 11) & 0xF;
    if opc > 9 {
        return false;
    }

    let rd = (instr & 0x1F) as usize;

    match opc {
        0..=3 => {
            // PACIA/PACIB/PACDA/PACDB: NOP (can't generate real PAC)
        }
        4..=9 => {
            // AUTIA/AUTIB/AUTDA/AUTDB/XPACI/XPACD: strip PAC from rd
            let ptr = trapframe.regs.reg[rd] as u64;
            trapframe.regs.reg[rd] = strip_pac(ptr) as usize;
        }
        _ => return false,
    }

    trapframe.elr += 4;
    true
}

// --- LDAPR / STLLR (ARMv8.3-RCpc / ARMv8.4-LSE) ---
//
// LDAPR Wt/Xt, [Xn|SP]:  x_111000_01_0_11111_1_00_Rn_Rt  (bit[21]=0)
// STLLR Wt/Xt, [Xn|SP]:  x_111000_01_1_11111_1_00_Rn_Rt  (bit[21]=1)
//
// Both are memory-ordering variants of LDR/STR. On a single-threaded
// emulator they are equivalent to plain LDR/STR.

fn emulate_rcpc(trapframe: &mut Trapframe, instr: u32, task: &crate::task::Task) -> bool {
    // Check: bits[29:22] = 11100001 and bits[15:12] = 1000
    if (instr >> 22) & 0xFF != 0b11100001 {
        return false;
    }
    if (instr >> 12) & 0xF != 0b1000 {
        return false;
    }
    // bits[20:16] must be 11111
    if (instr >> 16) & 0x1F != 0b11111 {
        return false;
    }
    // bit[11:10] must be 00
    if (instr >> 10) & 0x3 != 0 {
        return false;
    }

    let is_store = (instr >> 21) & 1 == 1;
    let is_64bit = (instr >> 31) & 1 == 1;
    let rn = ((instr >> 5) & 0x1F) as usize;
    let rt = (instr & 0x1F) as usize;
    let user_addr = trapframe.regs.reg[rn];

    let kva_addr = match task.vm_manager.translate_to_kva(user_addr) {
        Some(a) => a,
        None => return false,
    };

    if is_store {
        // STLLR → plain store
        if is_64bit {
            unsafe {
                core::ptr::write_volatile(kva_addr as *mut u64, trapframe.regs.reg[rt] as u64)
            };
        } else {
            unsafe {
                core::ptr::write_volatile(kva_addr as *mut u32, trapframe.regs.reg[rt] as u32)
            };
        }
    } else {
        // LDAPR → plain load
        if is_64bit {
            trapframe.regs.reg[rt] =
                unsafe { core::ptr::read_volatile(kva_addr as *const u64) } as usize;
        } else {
            trapframe.regs.reg[rt] =
                unsafe { core::ptr::read_volatile(kva_addr as *const u32) } as usize;
        }
    }

    trapframe.elr += 4;
    true
}

// --- LSE atomic instructions (ARMv8.1) ---
//
// CAS/CASA/CASL/CASAL: bits[28:24]=01000, bit[21]=1, bit[29]=0
// LDADD/CLR/EOR/SET/etc: bits[28:24]=10001, bit[21]=0, bit[29]=0
// SWP: bits[28:24]=11000, bit[21]=0, bit[29]=0

fn emulate_lse_atomic(trapframe: &mut Trapframe, instr: u32, task: &crate::task::Task) -> bool {
    // bit[29]=0 required for LSE atomics
    if (instr >> 29) & 1 != 0 {
        return false;
    }

    let group = (instr >> 24) & 0x1F;
    if !matches!(group, 0b01000 | 0b10001 | 0b11000) {
        return false;
    }
    // For CAS: bit[21] must be 1
    if group == 0b01000 && (instr >> 21) & 1 != 1 {
        return false;
    }

    let rs = ((instr >> 16) & 0x1F) as usize;
    let rn = ((instr >> 5) & 0x1F) as usize;
    let rt = (instr & 0x1F) as usize;
    let user_addr = trapframe.regs.reg[rn];

    let kva_addr = match task.vm_manager.translate_to_kva(user_addr) {
        Some(a) => a,
        None => return false,
    };

    let is_64bit = (instr >> 30) & 0x3 == 0b11;

    match group {
        0b01000 => {
            // CAS family
            if is_64bit {
                do_cas_64(trapframe, rs, rt, kva_addr);
            } else {
                do_cas_32(trapframe, rs, rt, kva_addr);
            }
        }
        0b10001 => {
            // LDADD/CLR/EOR/SET/SMAX/SMIN/UMAX/UMIN
            let o3 = ((instr >> 15) & 1) as u8;
            let opcode = ((instr >> 12) & 0x7) as u8;
            if is_64bit {
                do_lse_binop_64(trapframe, opcode, o3, rs, rt, kva_addr);
            } else {
                do_lse_binop_32(trapframe, opcode, o3, rs, rt, kva_addr);
            }
        }
        0b11000 => {
            // SWP
            let swp_rs_val = read_reg(trapframe, rs);
            let swp_old = if is_64bit {
                unsafe { core::ptr::read_volatile(kva_addr as *const u64) }
            } else {
                unsafe { core::ptr::read_volatile(kva_addr as *const u32) as u64 }
            };
            println!(
                "[lse] SWP{} rs={} rt={} rs_val={:#x} old={:#x} addr={:#x} elr={:#x}",
                if is_64bit { "64" } else { "32" },
                rs,
                rt,
                swp_rs_val,
                swp_old,
                user_addr,
                trapframe.elr
            );
            if is_64bit {
                do_swp_64(trapframe, rs, rt, kva_addr);
            } else {
                do_swp_32(trapframe, rs, rt, kva_addr);
            }
        }
        _ => return false,
    }

    trapframe.elr += 4;
    true
}

fn read_reg(tf: &Trapframe, idx: usize) -> usize {
    if idx == 31 { 0 } else { tf.regs.reg[idx] }
}

fn write_reg(tf: &mut Trapframe, idx: usize, val: usize) {
    if idx != 31 {
        tf.regs.reg[idx] = val;
    }
}

// --- CAS: Rs = old[Rn]; if Rs == [Rn] then [Rn] = Rt ---

fn do_cas_32(trapframe: &mut Trapframe, rs: usize, rt: usize, addr: usize) {
    let ptr = addr as *mut u32;
    let old_val = unsafe { core::ptr::read_volatile(ptr) };
    let rs_val = read_reg(trapframe, rs) as u32;
    let rt_val = read_reg(trapframe, rt) as u32;
    if old_val == rs_val {
        unsafe { core::ptr::write_volatile(ptr, rt_val) };
    }
    write_reg(trapframe, rs, old_val as usize);
}

fn do_cas_64(trapframe: &mut Trapframe, rs: usize, rt: usize, addr: usize) {
    let ptr = addr as *mut u64;
    let old_val = unsafe { core::ptr::read_volatile(ptr) };
    let rs_val = read_reg(trapframe, rs) as u64;
    let rt_val = read_reg(trapframe, rt) as u64;
    if old_val == rs_val {
        unsafe { core::ptr::write_volatile(ptr, rt_val) };
    }
    write_reg(trapframe, rs, old_val as usize);
}

// --- SWP: tmp = [Rn]; [Rn] = Rs; Rs = tmp; Rt = tmp ---

fn do_swp_32(trapframe: &mut Trapframe, rs: usize, rt: usize, addr: usize) {
    let ptr = addr as *mut u32;
    let old_val = unsafe { core::ptr::read_volatile(ptr) };
    unsafe { core::ptr::write_volatile(ptr, read_reg(trapframe, rs) as u32) };
    write_reg(trapframe, rs, old_val as usize);
    write_reg(trapframe, rt, old_val as usize);
}

fn do_swp_64(trapframe: &mut Trapframe, rs: usize, rt: usize, addr: usize) {
    let ptr = addr as *mut u64;
    let old_val = unsafe { core::ptr::read_volatile(ptr) };
    unsafe { core::ptr::write_volatile(ptr, read_reg(trapframe, rs) as u64) };
    write_reg(trapframe, rs, old_val as usize);
    write_reg(trapframe, rt, old_val as usize);
}

// --- LDADD/CLR/EOR/SET/SMAX/SMIN/UMAX/UMIN: Rt = [Rn]; [Rn] = [Rn] OP Rs ---

fn do_lse_binop_32(
    trapframe: &mut Trapframe,
    opcode: u8,
    o3: u8,
    rs: usize,
    rt: usize,
    addr: usize,
) {
    let ptr = addr as *mut u32;
    let old_val = unsafe { core::ptr::read_volatile(ptr) };
    let rs_val = read_reg(trapframe, rs) as u32;

    let new_val = match (opcode, o3) {
        (0b000, 0) => old_val.wrapping_add(rs_val),
        (0b001, 0) => old_val & !rs_val,
        (0b010, 0) => old_val ^ rs_val,
        (0b011, 0) => old_val | rs_val,
        (0b100, 0) => old_val.max(rs_val),
        (0b101, 0) => old_val.min(rs_val),
        (0b110, 0) => old_val.max(rs_val),
        (0b111, 0) => old_val.min(rs_val),
        _ => return,
    };

    unsafe { core::ptr::write_volatile(ptr, new_val) };
    write_reg(trapframe, rt, old_val as usize);
}

fn do_lse_binop_64(
    trapframe: &mut Trapframe,
    opcode: u8,
    o3: u8,
    rs: usize,
    rt: usize,
    addr: usize,
) {
    let ptr = addr as *mut u64;
    let old_val = unsafe { core::ptr::read_volatile(ptr) };
    let rs_val = read_reg(trapframe, rs) as u64;

    let new_val = match (opcode, o3) {
        (0b000, 0) => old_val.wrapping_add(rs_val),
        (0b001, 0) => old_val & !rs_val,
        (0b010, 0) => old_val ^ rs_val,
        (0b011, 0) => old_val | rs_val,
        (0b100, 0) => old_val.max(rs_val),
        (0b101, 0) => old_val.min(rs_val),
        (0b110, 0) => old_val.max(rs_val),
        (0b111, 0) => old_val.min(rs_val),
        _ => return,
    };

    unsafe { core::ptr::write_volatile(ptr, new_val) };
    write_reg(trapframe, rt, old_val as usize);
}
