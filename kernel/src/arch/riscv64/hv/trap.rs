//! RISC-V H-extension trap information

use core::arch::asm;

use crate::arch::Trapframe;
use crate::arch::hv::csr;
use crate::hypervisor::VmObject;
use crate::hypervisor::types::VmExit;

pub const HSTATUS_SPV: u64 = 1 << 7;
pub const HSTATUS_SPVP: u64 = 1 << 8;

pub const CAUSE_ECALL_FROM_VS: usize = 10;
pub const CAUSE_INST_GUEST_PAGE_FAULT: usize = 20;
pub const CAUSE_LOAD_GUEST_PAGE_FAULT: usize = 21;
pub const CAUSE_VIRTUAL_INSTRUCTION: usize = 22;
pub const CAUSE_STORE_GUEST_PAGE_FAULT: usize = 23;

pub fn is_from_guest() -> bool {
    let hstatus: u64;
    unsafe {
        asm!("csrr {0}, hstatus", out(reg) hstatus);
    }
    (hstatus & HSTATUS_SPVP) != 0
}

pub fn clear_guest_mode() {
    let mut hstatus: u64;
    unsafe {
        asm!("csrr {0}, hstatus", out(reg) hstatus);
        hstatus &= !(HSTATUS_SPV | HSTATUS_SPVP);
        asm!("csrw hstatus, {0}", in(reg) hstatus);
    }
}

fn read_stval() -> u64 {
    let val: u64;
    unsafe {
        asm!("csrr {0}, stval", out(reg) val);
    }
    val
}

fn read_htval() -> u64 {
    let val: u64;
    unsafe {
        asm!("csrr {0}, htval", out(reg) val);
    }
    val
}

fn read_htinst() -> u64 {
    let val: u64;
    unsafe {
        asm!("csrr {0}, htinst", out(reg) val);
    }
    val
}

fn get_gpa() -> u64 {
    (read_htval() << 2) | (read_stval() & 0x3)
}

#[derive(Debug, Clone, Copy)]
struct MmioDecode {
    size: u8,
    reg: u8,
}

fn decode_mmio() -> Option<MmioDecode> {
    let htinst = read_htinst();

    if htinst == 0 {
        return None;
    }

    let opcode = htinst & 0x7F;
    let funct3 = (htinst >> 12) & 0x7;
    let rd = ((htinst >> 7) & 0x1F) as u8;
    let rs2 = ((htinst >> 20) & 0x1F) as u8;

    match opcode {
        0x03 => {
            let size = match funct3 {
                0 => 1,
                1 => 2,
                2 => 4,
                3 => 8,
                4 => 1,
                5 => 2,
                6 => 4,
                _ => return None,
            };
            Some(MmioDecode { size, reg: rd })
        }
        0x23 => {
            let size = match funct3 {
                0 => 1,
                1 => 2,
                2 => 4,
                3 => 8,
                _ => return None,
            };
            Some(MmioDecode { size, reg: rs2 })
        }
        _ => None,
    }
}

pub fn arch_guest_trap_handler(trapframe: &mut Trapframe, vm: &VmObject) -> Option<VmExit> {
    let cause = (csr::read_scause() & 0x7fff_ffff_ffff_ffff) as usize;

    match cause {
        CAUSE_INST_GUEST_PAGE_FAULT
        | CAUSE_LOAD_GUEST_PAGE_FAULT
        | CAUSE_STORE_GUEST_PAGE_FAULT => {
            let gpa = get_gpa();
            match vm.find_memory_slot(gpa) {
                Some(slot) => {
                    let hpa = slot.gpa_to_hpa(gpa);
                    let writable = !slot.flags.readonly;
                    let _ = vm.map_stage2_page(gpa, hpa, writable);
                    None
                }
                None => {
                    let mmio = decode_mmio();
                    let (size, reg) = match mmio {
                        Some(m) => (m.size, m.reg),
                        None => (8, 0),
                    };

                    let is_write = cause == CAUSE_STORE_GUEST_PAGE_FAULT;
                    Some(if is_write {
                        VmExit::MmioWrite {
                            addr: gpa,
                            size,
                            reg,
                            data: 0,
                        }
                    } else {
                        VmExit::MmioRead {
                            addr: gpa,
                            size,
                            reg,
                        }
                    })
                }
            }
        }
        CAUSE_ECALL_FROM_VS => Some(VmExit::Hlt),
        CAUSE_VIRTUAL_INSTRUCTION => Some(VmExit::Hlt),
        _ => Some(VmExit::Unknown(cause as u64)),
    }
}
