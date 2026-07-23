//! RISC-V H-extension trap information

use core::arch::asm;

use crate::arch::Trapframe;
use crate::arch::hv::csr::{self, read_htinst};
use crate::arch::hv::guest_vcpu::GuestVcpu;
use crate::arch::hv::vm::Riscv64VmObject;
use crate::hypervisor::types::VmExit;
use crate::timer::tick;

const VSTIP_BIT: u64 = 1 << 6;
const VS_INTERRUPT_BITS: u64 = (1 << 2) | (1 << 6) | (1 << 10);
const INST_WFI: u32 = 0x1050_0073;

pub fn clear_sbi_timer_pending() {
    let hvip = csr::read_hvip();
    csr::write_hvip(hvip & !VSTIP_BIT);
}

fn read_rdtime() -> u64 {
    let t: u64;
    unsafe { asm!("rdtime {}", out(reg) t, options(nostack)) };
    t
}

fn guest_time_now() -> u64 {
    read_rdtime().wrapping_add(csr::read_htimedelta())
}

pub fn set_sbi_timer_next_event(vcpu: &mut GuestVcpu, next_event: u64) {
    vcpu.set_sbi_timer_next_event(next_event);
    clear_sbi_timer_pending();
}

pub fn check_sbi_timer_expired(vcpu: &mut GuestVcpu) -> bool {
    let next = vcpu.sbi_timer_next_event();
    if next == u64::MAX {
        return false;
    }
    if guest_time_now() >= next {
        vcpu.clear_sbi_timer_next_event();
        let hvip = csr::read_hvip();
        csr::write_hvip(hvip | VSTIP_BIT);
        return true;
    }

    false
}

pub fn sbi_timer_timeout_ticks(vcpu: &GuestVcpu) -> Option<u64> {
    let next = vcpu.sbi_timer_next_event();
    if next == u64::MAX {
        return None;
    }

    let now = guest_time_now();
    if now >= next {
        return Some(0);
    }

    let cpu_id = crate::arch::get_cpu().get_cpuid() as u32;
    let freq = crate::interrupt::InterruptManager::global()
        .get_timer_frequency_hz(cpu_id)
        .unwrap_or(10_000_000);
    if freq == 0 {
        return Some(1);
    }

    let delta_cycles = next - now;
    let tick_cycles =
        ((freq as u128) * (crate::timer::TICK_INTERVAL_US as u128)).div_ceil(1_000_000) as u64;
    let tick_cycles = tick_cycles.max(1);

    Some(delta_cycles.div_ceil(tick_cycles).max(1))
}

const CAUSE_ILLEGAL_INSTRUCTION: usize = 2;
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
    rd: usize,
    rs2: usize,
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
    let rd = ((htinst >> 7) & 0x1f) as usize;
    let rs2 = ((htinst >> 20) & 0x1f) as usize;

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

fn resolve_guest_kva(gpa: u64, vm: &Riscv64VmObject) -> Option<usize> {
    let slot = vm.find_memory_slot(gpa)?;
    let userspace_addr = slot.gpa_to_userspace_addr(gpa) as usize;
    vm.owner_mm().translate_to_kva(userspace_addr)
}

fn resolve_guest_hpa(gpa: u64, vm: &Riscv64VmObject) -> Option<u64> {
    let slot = vm.find_memory_slot(gpa)?;
    let userspace_addr = slot.gpa_to_userspace_addr(gpa) as usize;
    vm.owner_mm()
        .translate_to_phys(userspace_addr)
        .map(|p| p as u64)
}

fn read_guest_halfword(kva: usize) -> Option<u16> {
    if kva & 1 != 0 {
        return None;
    }

    // SAFETY: the caller obtained kva from translate_to_kva. The explicit
    // alignment check satisfies read_volatile's u16 alignment requirement.
    Some(unsafe { core::ptr::read_volatile(kva as *const u16) })
}

fn fetch_guest_inst(sepc: u64, vm: &Riscv64VmObject) -> u32 {
    let Some(low_kva) = resolve_guest_kva(sepc, vm) else {
        return 0;
    };
    let Some(low) = read_guest_halfword(low_kva) else {
        return 0;
    };

    if low & 0x3 != 0x3 {
        return u32::from(low);
    }

    let Some(high_kva) = resolve_guest_kva(sepc.wrapping_add(2), vm) else {
        return 0;
    };
    let Some(high) = read_guest_halfword(high_kva) else {
        return 0;
    };

    u32::from(low) | (u32::from(high) << 16)
}

fn decode_load_store_inst(inst: u32) -> Option<(u8, usize, bool, usize, u32)> {
    // Check for compressed instruction (16-bit)
    if (inst & 0x3) != 0x3 {
        // Compressed instruction
        let c_inst = inst as u16;
        let opcode = c_inst & 0x3;
        let funct3 = (c_inst >> 13) & 0x7;
        let rd = ((c_inst >> 7) & 0x7) as usize; // rd/rs2 for compressed

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
                let rd = ((c_inst >> 7) & 0x1f) as usize;
                if rd == 0 {
                    return None;
                } // Reserved
                Some((4, rd, false, rd, 2))
            }
            // C.LDSP (load doubleword from stack pointer)
            (0b10, 0b011) => {
                let rd = ((c_inst >> 7) & 0x1f) as usize;
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
                let rs2 = ((inst >> 20) & 0x1f) as usize;
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
                let rd = ((inst >> 7) & 0x1f) as usize;
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

use core::sync::atomic::{AtomicU64, Ordering};

static PF_COUNT: AtomicU64 = AtomicU64::new(0);
static WFI_NONE_COUNT: AtomicU64 = AtomicU64::new(0);
static TIMER_NONE_COUNT: AtomicU64 = AtomicU64::new(0);

fn arch_guest_trap_handler_inner(
    trapframe: &mut Trapframe,
    vm: &Riscv64VmObject,
    vcpu: &mut GuestVcpu,
) -> Option<VmExit> {
    let scause = csr::read_scause();
    let is_interrupt = (scause & 0x8000_0000_0000_0000) != 0;
    let cause = (scause & 0x7fff_ffff_ffff_ffff) as usize;

    if is_interrupt {
        if cause == SUPERVISOR_TIMER_INTERRUPT {
            tick(trapframe);
            check_sbi_timer_expired(vcpu);
            let c = TIMER_NONE_COUNT.fetch_add(1, Ordering::Relaxed);
            if c < 5 {
                let sepc = csr::read_sepc();
                let vsatp = csr::read_vsatp();
                crate::println!("[TIMER] #{} sepc={:#x} vsatp={:#x}", c, sepc, vsatp);
            }
            return None;
        }
        return Some(VmExit::Unknown(scause));
    }

    match cause {
        CAUSE_ILLEGAL_INSTRUCTION => {
            let epc = csr::read_sepc();
            let inst = fetch_guest_inst(epc, vm);
            let inst_len = if (inst & 0x3) == 0x3 { 4 } else { 2 };
            Some(VmExit::IllegalInstruction {
                epc,
                inst: Some(inst),
                inst_len: Some(inst_len),
            })
        }
        CAUSE_BREAKPOINT => {
            let epc = csr::read_sepc();
            Some(VmExit::Breakpoint { epc })
        }
        CAUSE_INST_GUEST_PAGE_FAULT
        | CAUSE_LOAD_GUEST_PAGE_FAULT
        | CAUSE_STORE_GUEST_PAGE_FAULT => {
            let gpa = get_gpa();

            match vm.find_memory_slot(gpa) {
                Some(slot) => {
                    let Some(hpa) = resolve_guest_hpa(gpa, vm) else {
                        return Some(VmExit::FailEntry {
                            hardware_entry_failure_reason: 0,
                        });
                    };
                    let writable = !slot.flags.readonly;
                    if vm.map_stage2_page(gpa, hpa, writable).is_err() {
                        return Some(VmExit::FailEntry {
                            hardware_entry_failure_reason: 0,
                        });
                    }
                    let c = PF_COUNT.fetch_add(1, Ordering::Relaxed);
                    if c % 10000 == 0 {
                        let sepc = csr::read_sepc();
                        crate::println!("[PF] #{} gpa={:#x} sepc={:#x}", c, gpa, sepc);
                    }
                    None
                }
                None => {
                    let is_write = cause == CAUSE_STORE_GUEST_PAGE_FAULT;

                    let (inst_len, size, reg, data) = if let Some(m) = decode_mmio() {
                        let data = if is_write && m.rs2 != 0 {
                            trapframe.regs.reg[m.rs2 as usize] as u64
                        } else {
                            0
                        };
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
            let epc = csr::read_sepc();
            let inst = if htinst != 0 {
                htinst as u32
            } else {
                fetch_guest_inst(epc, vm)
            };

            let hvip = csr::read_hvip();
            let vsie = csr::read_vsie();
            let vsstatus = csr::read_vsstatus();

            let pending = hvip & VS_INTERRUPT_BITS;
            let enabled = vsie & VS_INTERRUPT_BITS;
            let active = pending & enabled;
            let sie_enabled = (vsstatus & 0x02) != 0;

            if inst == INST_WFI {
                trapframe.epc = epc.wrapping_add(4);
                if active != 0 && sie_enabled {
                    let c = WFI_NONE_COUNT.fetch_add(1, Ordering::Relaxed);
                    if c % 10000 == 0 {
                        crate::println!(
                            "[WFI-NONE] #{} active={:#x} hvip={:#x} vsie={:#x}",
                            c,
                            active,
                            hvip,
                            vsie
                        );
                    }
                    return None;
                }
                return Some(VmExit::Wfi);
            }

            Some(VmExit::VirtualInstruction {
                epc,
                inst: Some(inst),
                inst_len: Some(4),
            })
        }
        _ => Some(VmExit::Unknown(cause as u64)),
    }
}

pub fn arch_guest_trap_handler(
    trapframe: &mut Trapframe,
    vm: &Riscv64VmObject,
    vcpu: &mut GuestVcpu,
) -> Option<VmExit> {
    arch_guest_trap_handler_inner(trapframe, vm, vcpu)
}

pub const HSTATUS_SPV: u64 = 1 << 7;

pub fn is_from_guest() -> bool {
    let hstatus: u64;
    // SAFETY: reading a RISC-V CSR (hstatus) has no side effects.
    unsafe {
        asm!("csrr {0}, hstatus", out(reg) hstatus);
    }
    (hstatus & HSTATUS_SPV) != 0
}

pub fn clear_guest_mode() {
    let hstatus: u64;
    // SAFETY: reading then writing hstatus.SPv to 0 is a valid privilege
    // mode transition operation in HS-mode.
    unsafe {
        asm!("csrr {0}, hstatus", out(reg) hstatus);
    }
    let hstatus = hstatus & !HSTATUS_SPV;
    unsafe {
        asm!("csrw hstatus, {0}", in(reg) hstatus);
    }
}

#[cfg(test)]
mod tests {
    use super::read_guest_halfword;

    #[repr(C, align(4))]
    struct TwoByteAlignedInstruction {
        padding: u16,
        instruction: [u16; 2],
    }

    #[test_case]
    fn test_read_guest_halfword_accepts_two_byte_alignment() {
        let data = TwoByteAlignedInstruction {
            padding: 0,
            instruction: [0x1234, 0x5678],
        };
        let instruction_ptr = core::ptr::addr_of!(data.instruction) as *const u16 as usize;

        assert_eq!(instruction_ptr & 0x3, 2);
        assert_eq!(read_guest_halfword(instruction_ptr), Some(0x1234));
        assert_eq!(read_guest_halfword(instruction_ptr + 2), Some(0x5678));
    }

    #[test_case]
    fn test_read_guest_halfword_rejects_odd_address() {
        let data = [0u16; 2];
        let odd_ptr = data.as_ptr() as usize + 1;

        assert_eq!(read_guest_halfword(odd_ptr), None);
    }
}
