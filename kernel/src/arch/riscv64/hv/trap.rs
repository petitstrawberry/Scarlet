//! RISC-V H-extension trap information

use core::arch::asm;

use crate::arch::Trapframe;
use crate::arch::hv::csr::{self, read_htinst};
use crate::arch::hv::vm::Riscv64VmObject;
use crate::hypervisor::types::VmExit;
use crate::timer::tick;

const CAUSE_BREAKPOINT: usize = 3;
const SUPERVISOR_TIMER_INTERRUPT: usize = 5;
const CAUSE_ECALL_FROM_VS: usize = 10;
const CAUSE_INST_GUEST_PAGE_FAULT: usize = 20;
const CAUSE_LOAD_GUEST_PAGE_FAULT: usize = 21;
const CAUSE_VIRTUAL_INSTRUCTION: usize = 22;
const CAUSE_STORE_GUEST_PAGE_FAULT: usize = 23;

fn get_gpa() -> u64 {
    let stval = csr::read_stval();
    let htval = csr::read_htval();
    (htval << 2) | (stval & 0x3)
}

struct MmioDecode {
    inst_len: u32,
    size: u8,
    rd: u8,
    rs2: u8,
}

fn decode_mmio() -> Option<MmioDecode> {
    let htinst = csr::read_htinst();
    if htinst == 0 {
        return None;
    }
    let size = match (htinst >> 12) & 0x7 {
        0 => 1,
        1 => 2,
        2 => 4,
        3 => 8,
        _ => return None,
    };
    let rd = ((htinst >> 7) & 0x1f) as u8;
    let rs2 = ((htinst >> 20) & 0x1f) as u8;

    // Instruction length is determined by lowest 2 bits of htinst
    // bits[1:0] = 0b11 -> 32-bit instruction
    // bits[1:0] = 0b01 -> 16-bit compressed instruction
    // bits[1:0] = 0b00 -> pseudo-instruction
    let inst_len = if (htinst & 0x3) == 0x3 { 4 } else { 2 };

    Some(MmioDecode {
        inst_len,
        size,
        rd,
        rs2,
    })
}

fn fetch_guest_inst(sepc: u64, vm: &Riscv64VmObject) -> u32 {
    if let Some(slot) = vm.find_memory_slot(sepc) {
        let hpa = slot.gpa_to_hpa(sepc);
        let inst_ptr = hpa as *const u32;
        unsafe { core::ptr::read_volatile(inst_ptr) }
    } else {
        0
    }
}

fn decode_load_store_inst(inst: u32) -> Option<(u8, u8, bool, u8, u32)> {
    // Check for compressed instruction (16-bit)
    if (inst & 0x3) != 0x3 {
        // Compressed instruction
        let c_inst = inst as u16;
        let opcode = c_inst & 0x3;
        let funct3 = (c_inst >> 13) & 0x7;
        let rd = ((c_inst >> 7) & 0x7) as u8; // rd/rs2 for compressed

        match (opcode, funct3) {
            // C.LW (load word, 32-bit)
            (0b00, 0b010) => Some((4, rd + 8, false, rd + 8, 2)), // rd is s0-s7 (x8-x15)
            // C.LD (load doubleword, 64-bit)
            (0b00, 0b011) => Some((8, rd + 8, false, rd + 8, 2)),
            // C.SW (store word)
            (0b10, 0b110) => Some((4, 0, true, rd + 8, 2)),
            // C.SD (store doubleword)
            (0b10, 0b111) => Some((8, 0, true, rd + 8, 2)),
            // C.LWSP (load word from stack pointer)
            (0b10, 0b010) => {
                let rd = ((c_inst >> 7) & 0x1f) as u8;
                if rd == 0 {
                    return None;
                } // Reserved
                Some((4, rd, false, rd, 2))
            }
            // C.LDSP (load doubleword from stack pointer)
            (0b10, 0b011) => {
                let rd = ((c_inst >> 7) & 0x1f) as u8;
                if rd == 0 {
                    return None;
                }
                Some((8, rd, false, rd, 2))
            }
            _ => None,
        }
    } else {
        // 32-bit instruction
        let opcode = inst & 0x7f;

        match opcode {
            0b0100011 => {
                let funct3 = (inst >> 12) & 0x7;
                let rs2 = ((inst >> 20) & 0x1f) as u8;
                let size = match funct3 {
                    0b000 => 1,
                    0b001 => 2,
                    0b010 => 4,
                    0b011 => 8,
                    _ => return None,
                };
                Some((size, 0, true, rs2, 4))
            }
            0b0000011 => {
                let funct3 = (inst >> 12) & 0x7;
                let rd = ((inst >> 7) & 0x1f) as u8;
                let size = match funct3 {
                    0b000 => 1,
                    0b001 => 2,
                    0b010 => 4,
                    0b011 => 8,
                    0b100 => 1,
                    0b101 => 2,
                    _ => return None,
                };
                Some((size, rd, false, rd, 4))
            }
            _ => None,
        }
    }
}

pub fn arch_guest_trap_handler(trapframe: &mut Trapframe, vm: &Riscv64VmObject) -> Option<VmExit> {
    let scause = csr::read_scause();
    let is_interrupt = (scause & 0x8000_0000_0000_0000) != 0;
    let cause = (scause & 0x7fff_ffff_ffff_ffff) as usize;

    if is_interrupt {
        let vstvec = csr::read_vstvec();
        // crate::println!(
        //     "[guest trap] Interrupt with epc: {:#x}, vstvec={:#x}",
        //     trapframe.epc, vstvec
        // );

        if cause == SUPERVISOR_TIMER_INTERRUPT {
            tick(trapframe);
            return None;
        }
        return Some(VmExit::Unknown(scause));
    }

    // crate::println!(
    //     "[guest trap] cause={} scause={:#x}",
    //     cause,
    //     csr::read_scause()
    // );
    // crate::println!("[guest trap] cause={} is_interrupt={}", cause, is_interrupt);
    // crate::println!(
    //     "[guest trap] hstatus={:#x} hgatp={:#x}",
    //     csr::read_hstatus(),
    //     csr::read_hgatp()
    // );
    // crate::println!(
    //     "[guest trap] vsstatus={:#x} vsatp={:#x} vsepc={:#x}",
    //     csr::read_vsstatus(),
    //     csr::read_vsatp(),
    //     csr::read_vsepc()
    // );
    // crate::println!(
    //     "[guest trap] sepc={:#x} stval={:#x} htval={:#x}",
    //     csr::read_sepc(),
    //     read_stval(),
    //     read_htval()
    // );
    // crate::println!("[guest trap] cause={}", cause);

    match cause {
        CAUSE_INST_GUEST_PAGE_FAULT
        | CAUSE_LOAD_GUEST_PAGE_FAULT
        | CAUSE_STORE_GUEST_PAGE_FAULT => {
            let gpa = get_gpa();

            let hgatp = csr::read_hgatp();
            let root_ppn = hgatp & 0xffff_ffff_fff;
            let root_addr = root_ppn << 12;
            // crate::println!("[guest pf] hgatp={:#x} root_addr={:#x}", hgatp, root_addr);

            let vpn3 = (gpa as usize >> 39) & 0x7ff;
            let vpn2 = (gpa as usize >> 30) & 0x1ff;
            let vpn1 = (gpa as usize >> 21) & 0x1ff;
            let vpn0 = (gpa as usize >> 12) & 0x1ff;
            // crate::println!(
            //     "[guest pf] vpn3={} vpn2={} vpn1={} vpn0={}",
            //     vpn3,
            //     vpn2,
            //     vpn1,
            //     vpn0
            // );

            unsafe {
                let root_pte = core::ptr::read((root_addr as usize + vpn3 * 8) as *const u64);
                // crate::println!(
                //     "[guest pf] L3 pte @ {:#x} = {:#x}",
                //     root_addr as usize + vpn3 * 8,
                //     root_pte
                // );
                if root_pte & 1 == 0 {
                    // crate::println!("[guest pf] L3 NOT VALID!");
                } else {
                    let l2_addr = ((root_pte >> 10) & 0x3ffffffffff) << 12;
                    let l2_pte = core::ptr::read((l2_addr as usize + vpn2 * 8) as *const u64);
                    // crate::println!(
                    //     "[guest pf] L2 pte @ {:#x} = {:#x}",
                    //     l2_addr as usize + vpn2 * 8,
                    //     l2_pte
                    // );
                    if l2_pte & 1 == 0 {
                        // crate::println!("[guest pf] L2 NOT VALID!");
                    } else {
                        let l1_addr = ((l2_pte >> 10) & 0x3ffffffffff) << 12;
                        let l1_pte = core::ptr::read((l1_addr as usize + vpn1 * 8) as *const u64);
                        // crate::println!(
                        //     "[guest pf] L1 pte @ {:#x} = {:#x}",
                        //     l1_addr as usize + vpn1 * 8,
                        //     l1_pte
                        // );
                        if l1_pte & 1 == 0 {
                            // crate::println!("[guest pf] L1 NOT VALID!");
                        } else {
                            let l0_addr = ((l1_pte >> 10) & 0x3ffffffffff) << 12;
                            let l0_pte =
                                core::ptr::read((l0_addr as usize + vpn0 * 8) as *const u64);
                            // crate::println!(
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

                    let writable = !slot.flags.readonly;
                    let _result = vm.map_stage2_page(gpa, hpa, writable);
                    // crate::println!("[guest pf] mapped RAM gpa={:#x}", gpa);
                    None
                }
                None => {
                    // crate::println!("[guest pf] MMIO gpa={:#x}", gpa);
                    let is_write = cause == CAUSE_STORE_GUEST_PAGE_FAULT;

                    let (inst_len, size, reg, data) = if let Some(m) = decode_mmio() {
                        let data = if is_write && m.rs2 != 0 {
                            trapframe.regs.reg[m.rs2 as usize] as u64
                        } else {
                            0
                        };
                        // crate::println!(
                        //     "[MMIO] decoded: len={} size={} reg={}",
                        //     m.inst_len,
                        //     m.size,
                        //     m.rd
                        // );
                        (m.inst_len, m.size, m.rd, data)
                    } else {
                        let sepc = csr::read_sepc();
                        let inst = fetch_guest_inst(sepc, vm);
                        if let Some((sz, rd, is_st, data_reg, inst_len)) =
                            decode_load_store_inst(inst)
                        {
                            let data = if is_st && data_reg != 0 {
                                trapframe.regs.reg[data_reg as usize] as u64
                            } else {
                                0
                            };
                            (inst_len, sz, rd, data)
                        } else {
                            (4, 1, 10, 0)
                        }
                    };

                    let epc = csr::read_sepc();
                    trapframe.epc = epc.wrapping_add(inst_len as u64);
                    // crate::println!(
                    //     "[MMIO] exit: epc={:#x}->{:#x} addr={:#x}",
                    //     epc,
                    //     trapframe.epc,
                    //     gpa
                    // );

                    Some(if is_write {
                        VmExit::MmioWrite {
                            epc,
                            addr: gpa,
                            size,
                            reg: 0,
                            data,
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
        CAUSE_VIRTUAL_INSTRUCTION => {
            let htinst = read_htinst();
            let inst = htinst as u32;

            // crate::println!("[virt_inst] ENTER htinst={:#x}", htinst);

            let hvip = csr::read_hvip();
            let vsie = csr::read_vsie();
            let vsstatus = csr::read_vsstatus();

            let vs_bits = (1u64 << 2) | (1u64 << 6) | (1u64 << 10);
            let pending = hvip & vs_bits;
            let enabled = vsie & vs_bits;
            let active = pending & enabled;
            let sie_enabled = (vsstatus & 0x02) != 0;

            // crate::println!(
            //     "[virt_inst] hvip={:#x} vsie={:#x} vsstatus={:#x} active={:#x} sie={}",
            //     hvip,
            //     vsie,
            //     vsstatus,
            //     active,
            //     sie_enabled
            // );

            if active != 0 && sie_enabled {
                let epc = csr::read_sepc();
                // crate::println!("[virt_inst] taking interrupt, advancing PC");
                trapframe.epc = epc.wrapping_add(4);
                return None;
            }

            Some(VmExit::VirtualInstruction {
                epc: trapframe.epc,
                inst: Some(inst),
                inst_len: Some(4),
            })
        }
        _ => Some(VmExit::Unknown(cause as u64)),
    }
}

pub const HSTATUS_SPV: u64 = 1 << 7;

pub fn is_from_guest() -> bool {
    let hstatus: u64;
    unsafe {
        asm!("csrr {0}, hstatus", out(reg) hstatus);
    }
    (hstatus & HSTATUS_SPV) != 0
}

pub fn clear_guest_mode() {
    let hstatus: u64;
    unsafe {
        asm!("csrr {0}, hstatus", out(reg) hstatus);
    }
    let hstatus = hstatus & !HSTATUS_SPV;
    unsafe {
        asm!("csrw hstatus, {0}", in(reg) hstatus);
    }
}
