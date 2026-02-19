//! RISC-V H-extension trap information

use core::arch::asm;

use crate::arch::Trapframe;
use crate::arch::hv::csr;
use crate::hypervisor::VmObject;
use crate::hypervisor::types::VmExit;
use crate::hypervisor::vm::fast_path_flags;

pub const HSTATUS_SPV: u64 = 1 << 7;
pub const HSTATUS_SPVP: u64 = 1 << 8;

pub const CAUSE_ECALL_FROM_VS: usize = 10;
pub const CAUSE_INST_GUEST_PAGE_FAULT: usize = 20;
pub const CAUSE_LOAD_GUEST_PAGE_FAULT: usize = 21;
pub const CAUSE_VIRTUAL_INSTRUCTION: usize = 22;
pub const CAUSE_STORE_GUEST_PAGE_FAULT: usize = 23;

const SBI_EXT_TIMER: u64 = 0x54494D45;
const SBI_TIMER_SET_TIMER: u64 = 0;

pub fn is_from_guest() -> bool {
    let hstatus: u64;
    unsafe {
        asm!("csrr {0}, hstatus", out(reg) hstatus);
    }
    (hstatus & HSTATUS_SPV) != 0
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
    inst_len: u8,
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

    let inst_len = if (htinst & 0x3) == 0x3 { 4 } else { 2 };

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
            Some(MmioDecode {
                inst_len,
                size,
                reg: rd,
            })
        }
        0x23 => {
            let size = match funct3 {
                0 => 1,
                1 => 2,
                2 => 4,
                3 => 8,
                _ => return None,
            };
            Some(MmioDecode {
                inst_len,
                size,
                reg: rs2,
            })
        }
        _ => None,
    }
}

pub fn arch_guest_trap_handler(trapframe: &mut Trapframe, vm: &VmObject) -> Option<VmExit> {
    let cause = (csr::read_scause() & 0x7fff_ffff_ffff_ffff) as usize;

    // crate::early_println!(
    //     "[guest trap] cause={} scause={:#x}",
    //     cause,
    //     csr::read_scause()
    // );
    // crate::early_println!(
    //     "[guest trap] hstatus={:#x} hgatp={:#x}",
    //     csr::read_hstatus(),
    //     csr::read_hgatp()
    // );
    // crate::early_println!(
    //     "[guest trap] vsstatus={:#x} vsatp={:#x} vsepc={:#x}",
    //     csr::read_vsstatus(),
    //     csr::read_vsatp(),
    //     csr::read_vsepc()
    // );
    // crate::early_println!(
    //     "[guest trap] sepc={:#x} stval={:#x} htval={:#x}",
    //     csr::read_sepc(),
    //     read_stval(),
    //     read_htval()
    // );

    match cause {
        CAUSE_INST_GUEST_PAGE_FAULT
        | CAUSE_LOAD_GUEST_PAGE_FAULT
        | CAUSE_STORE_GUEST_PAGE_FAULT => {
            let gpa = get_gpa();
            // crate::early_println!("[guest pf] cause={} gpa={:#x}", cause, gpa);

            let hgatp = csr::read_hgatp();
            let root_ppn = hgatp & 0xffff_ffff_fff;
            let root_addr = root_ppn << 12;
            // crate::early_println!("[guest pf] hgatp={:#x} root_addr={:#x}", hgatp, root_addr);

            let vpn3 = (gpa as usize >> 39) & 0x7ff;
            let vpn2 = (gpa as usize >> 30) & 0x1ff;
            let vpn1 = (gpa as usize >> 21) & 0x1ff;
            let vpn0 = (gpa as usize >> 12) & 0x1ff;
            // crate::early_println!(
            //     "[guest pf] vpn3={} vpn2={} vpn1={} vpn0={}",
            //     vpn3,
            //     vpn2,
            //     vpn1,
            //     vpn0
            // );

            unsafe {
                let root_pte = core::ptr::read((root_addr as usize + vpn3 * 8) as *const u64);
                // crate::early_println!(
                //     "[guest pf] L3 pte @ {:#x} = {:#x}",
                //     root_addr as usize + vpn3 * 8,
                //     root_pte
                // );
                if root_pte & 1 == 0 {
                    // crate::early_println!("[guest pf] L3 NOT VALID!");
                } else {
                    let l2_addr = ((root_pte >> 10) & 0x3ffffffffff) << 12;
                    let l2_pte = core::ptr::read((l2_addr as usize + vpn2 * 8) as *const u64);
                    // crate::early_println!(
                    //     "[guest pf] L2 pte @ {:#x} = {:#x}",
                    //     l2_addr as usize + vpn2 * 8,
                    //     l2_pte
                    // );
                    if l2_pte & 1 == 0 {
                        // crate::early_println!("[guest pf] L2 NOT VALID!");
                    } else {
                        let l1_addr = ((l2_pte >> 10) & 0x3ffffffffff) << 12;
                        let l1_pte = core::ptr::read((l1_addr as usize + vpn1 * 8) as *const u64);
                        // crate::early_println!(
                        //     "[guest pf] L1 pte @ {:#x} = {:#x}",
                        //     l1_addr as usize + vpn1 * 8,
                        //     l1_pte
                        // );
                        if l1_pte & 1 == 0 {
                            // crate::early_println!("[guest pf] L1 NOT VALID!");
                        } else {
                            let l0_addr = ((l1_pte >> 10) & 0x3ffffffffff) << 12;
                            let l0_pte =
                                core::ptr::read((l0_addr as usize + vpn0 * 8) as *const u64);
                            // crate::early_println!(
                            //     "[guest pf] L0 pte @ {:#x} = {:#x}",
                            //     l0_addr as usize + vpn0 * 8,
                            //     l0_pte
                            // );
                        }
                    }
                }
            }

            match vm.find_memory_slot(gpa) {
                Some(slot) => {
                    let hpa = slot.gpa_to_hpa(gpa);
                    // crate::early_println!("[guest pf] slot found: hpa={:#x}", hpa);

                    unsafe {
                        let code = core::ptr::read(hpa as *const u32);
                        // crate::early_println!("[guest pf] code at hpa: {:#x}", code);
                    }

                    let writable = !slot.flags.readonly;
                    let accessed = true;
                    let dirty = cause == CAUSE_STORE_GUEST_PAGE_FAULT;
                    let result = vm.map_stage2_page(gpa, hpa, writable, accessed, dirty);
                    // crate::early_println!("[guest pf] map_stage2_page result={:?}", result);
                    None
                }
                None => {
                    // crate::early_println!("[guest pf] no slot found, treating as MMIO");
                    let mmio = decode_mmio();
                    let (inst_len, size, reg) = match mmio {
                        Some(m) => (m.inst_len, m.size, m.reg),
                        None => (4, 8, 0),
                    };

                    let epc = csr::read_sepc();
                    trapframe.epc = epc.wrapping_add(inst_len as u64);

                    let is_write = cause == CAUSE_STORE_GUEST_PAGE_FAULT;
                    Some(if is_write {
                        VmExit::MmioWrite {
                            epc,
                            addr: gpa,
                            size,
                            reg,
                            data: 0,
                        }
                    } else {
                        VmExit::MmioRead {
                            epc,
                            addr: gpa,
                            size,
                            reg,
                        }
                    })
                }
            }
        }
        CAUSE_ECALL_FROM_VS => {
            let epc = csr::read_sepc();
            trapframe.epc = epc.wrapping_add(4);
            Some(VmExit::FirmwareCall { epc })
        }
        CAUSE_VIRTUAL_INSTRUCTION => Some(VmExit::Hlt),
        _ => Some(VmExit::Unknown(cause as u64)),
    }
}
