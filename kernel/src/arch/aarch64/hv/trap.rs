use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

use super::guest_vcpu::GuestVcpu;
use super::switch::{HCR_EL2_HOST, HCR_EL2_VM, current_host_hv_context};
use super::sysreg::GuestSystemRegs;
use super::vm::Vm;
use crate::arch::Trapframe;
use crate::hypervisor::types::VmExit;

const ESR_EC_SHIFT: u64 = 26;
const ESR_EC_MASK: u64 = 0x3f;
const ESR_ISS_MASK: u64 = 0x1ff_ffff;

const ESR_EC_WFX: u32 = 0x01;
const ESR_EC_ILL: u32 = 0x0e;
const ESR_EC_HVC64: u32 = 0x16;
const ESR_EC_SMC64: u32 = 0x17;
const ESR_EC_SYS64: u32 = 0x18;
const ESR_EC_IABT_LOW: u32 = 0x20;
const ESR_EC_DABT_LOW: u32 = 0x24;
const ESR_EC_BREAKPT_LOW: u32 = 0x30;
const ESR_EC_BRK64: u32 = 0x3c;

const ESR_WNR: u32 = 1 << 6;
const ESR_ISV: u32 = 1 << 24;
const ESR_SAS_SHIFT: u32 = 22;
const ESR_SRT_SHIFT: u32 = 16;
const ESR_SRT_MASK: u32 = 0x1f;
const ESR_SYS64_ISS_DIR_READ: u32 = 1;
const ESR_SYS64_ISS_RT_MASK: u32 = 0x1f;
const ESR_SYS64_ISS_RT_SHIFT: u32 = 5;

const SYS_CNTFRQ_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7);
const SYS_CNTP_CTL_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7) | (2 << 3) | 1;
const SYS_CNTP_CVAL_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7) | (2 << 3) | 2;
const SYS_CNTP_TVAL_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7) | (2 << 3);
const SYS_CNTPCT_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7) | 1;
const SYS_CNTV_CTL_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7) | (3 << 3) | 1;
const SYS_CNTV_CVAL_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7) | (3 << 3) | 2;
const SYS_CNTV_TVAL_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7) | (3 << 3) | 0;
const SYS_CNTVCT_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7) | (0 << 3) | 2;
const SYS_MPIDR_EL1: u32 = (3 << 14) | (0 << 11) | (0 << 7) | (0 << 3) | 5;
const SYS_ID_AA64PFR0_EL1: u32 = (3 << 14) | (4 << 3);
const SYS_TCR2_EL1: u32 = (3 << 14) | (2 << 7) | 3;
const SYS_PIRE0_EL1: u32 = (3 << 14) | (10 << 7) | (2 << 3) | 2;
const SYS_PIR_EL1: u32 = (3 << 14) | (10 << 7) | (2 << 3) | 3;
const SYS_CLIDR_EL1: u32 = (3 << 14) | (1 << 11) | (0 << 7) | (0 << 3) | 1;
const SYS_CCSIDR_EL1: u32 = (3 << 14) | (1 << 11) | (0 << 7) | (0 << 3) | 0;
const SYS_CSSELR_EL1: u32 = (3 << 14) | (2 << 11) | (0 << 7) | (0 << 3) | 0;
const SYS_CTR_EL0: u32 = (3 << 14) | (3 << 11) | (0 << 7) | (0 << 3) | 1;
const TIMER_CTL_ENABLE: u64 = 1 << 0;
const TIMER_CTL_ISTATUS: u64 = 1 << 2;

const GIC_DIST_SIZE: u64 = 0x1_0000;
const GIC_CPUI_SIZE: u64 = 0x2_0000;
const RESCHEDULE_SGI: u32 = 0;
static UNKNOWN_GUEST_TRAP_DEBUG_COUNT: AtomicU32 = AtomicU32::new(0);

pub fn set_sbi_timer_next_event(_next_event: u64) {}

fn data_abort_access_size(iss: u32) -> u8 {
    if (iss & ESR_ISV) == 0 {
        return 4;
    }
    1u8 << ((iss >> ESR_SAS_SHIFT) & 0x3)
}

fn data_abort_srt(iss: u32) -> u8 {
    ((iss >> ESR_SRT_SHIFT) & ESR_SRT_MASK) as u8
}

fn esr_sys64_to_sysreg(iss: u32) -> u32 {
    let op0 = (iss >> 20) & 0x3;
    let op1 = (iss >> 14) & 0x7;
    let crn = (iss >> 10) & 0xf;
    let crm = (iss >> 1) & 0xf;
    let op2 = (iss >> 17) & 0x7;
    ((op0 as u32) << 14)
        | ((op1 as u32) << 11)
        | ((crn as u32) << 7)
        | ((crm as u32) << 3)
        | (op2 as u32)
}

fn is_timer_sysreg(sysreg: u32) -> bool {
    matches!(
        sysreg,
        SYS_CNTFRQ_EL0
            | SYS_CNTP_CTL_EL0
            | SYS_CNTP_CVAL_EL0
            | SYS_CNTP_TVAL_EL0
            | SYS_CNTPCT_EL0
            | SYS_CNTV_CTL_EL0
            | SYS_CNTV_CVAL_EL0
            | SYS_CNTV_TVAL_EL0
            | SYS_CNTVCT_EL0
    )
}

fn read_cntpct_el0() -> u64 {
    let value: u64;
    // SAFETY: reading the architected physical counter is side-effect free.
    unsafe {
        asm!("mrs {0}, cntpct_el0", out(reg) value, options(nostack));
    }
    value
}

fn guest_virtual_count(sysregs: &GuestSystemRegs) -> u64 {
    read_cntpct_el0().wrapping_sub(sysregs.cntvoff_el2)
}

fn read_cntfrq_el0() -> u64 {
    let value: u64;
    // SAFETY: reading the architected counter frequency is side-effect free.
    unsafe {
        asm!("mrs {0}, cntfrq_el0", out(reg) value, options(nostack));
    }
    value
}

fn timer_ctl_with_status(sysregs: &GuestSystemRegs) -> u64 {
    let mut ctl = sysregs.cntv_ctl_el0;
    if (ctl & TIMER_CTL_ENABLE) != 0 && guest_virtual_count(sysregs) >= sysregs.cntv_cval_el0 {
        ctl |= TIMER_CTL_ISTATUS;
    } else {
        ctl &= !TIMER_CTL_ISTATUS;
    }
    ctl
}

fn emulate_timer_sysreg(
    trapframe: &mut Trapframe,
    guest: &mut GuestVcpu,
    sysreg: u32,
    reg: u8,
    is_read: bool,
) -> bool {
    let mut sysregs = guest.sysregs.clone();
    sysregs.take_pending_into();

    match sysreg {
        SYS_CNTFRQ_EL0 if is_read => {
            if reg < 31 {
                trapframe.regs.reg[reg as usize] = read_cntfrq_el0() as usize;
            }
        }
        SYS_CNTPCT_EL0 | SYS_CNTVCT_EL0 if is_read => {
            if reg < 31 {
                trapframe.regs.reg[reg as usize] = guest_virtual_count(&sysregs) as usize;
            }
        }
        SYS_CNTP_CTL_EL0 | SYS_CNTV_CTL_EL0 if is_read => {
            if reg < 31 {
                trapframe.regs.reg[reg as usize] = timer_ctl_with_status(&sysregs) as usize;
            }
        }
        SYS_CNTP_CTL_EL0 | SYS_CNTV_CTL_EL0 => {
            if reg < 31 {
                sysregs.cntv_ctl_el0 =
                    (trapframe.regs.reg[reg as usize] as u64) & !TIMER_CTL_ISTATUS;
            }
        }
        SYS_CNTP_CVAL_EL0 | SYS_CNTV_CVAL_EL0 if is_read => {
            if reg < 31 {
                trapframe.regs.reg[reg as usize] = sysregs.cntv_cval_el0 as usize;
            }
        }
        SYS_CNTP_CVAL_EL0 | SYS_CNTV_CVAL_EL0 => {
            if reg < 31 {
                sysregs.cntv_cval_el0 = trapframe.regs.reg[reg as usize] as u64;
            }
        }
        SYS_CNTP_TVAL_EL0 | SYS_CNTV_TVAL_EL0 if is_read => {
            if reg < 31 {
                trapframe.regs.reg[reg as usize] = sysregs
                    .cntv_cval_el0
                    .wrapping_sub(guest_virtual_count(&sysregs))
                    as usize;
            }
        }
        SYS_CNTP_TVAL_EL0 | SYS_CNTV_TVAL_EL0 => {
            if reg < 31 {
                sysregs.cntv_cval_el0 = guest_virtual_count(&sysregs)
                    .wrapping_add(trapframe.regs.reg[reg as usize] as u64);
            }
        }
        _ => return false,
    }

    guest.sysregs = sysregs;
    trapframe.elr = trapframe.elr.wrapping_add(4);
    true
}

fn virtual_id_sysreg_value(sysreg: u32) -> Option<u64> {
    let op0 = (sysreg >> 14) & 0x3;
    let op1 = (sysreg >> 11) & 0x7;
    let crn = (sysreg >> 7) & 0xf;
    let crm = (sysreg >> 3) & 0xf;

    if op0 == 3 && op1 == 0 && crn == 0 {
        Some(if sysreg == SYS_MPIDR_EL1 {
            0x8000_0000
        } else if sysreg == SYS_ID_AA64PFR0_EL1 {
            0x11
        } else if crm <= 7 {
            0
        } else {
            return None;
        })
    } else {
        match sysreg {
            // CLIDR_EL1: L1I + L1D (inner), no L2.  LoUIS=1, LoC=1, LoUU=1.
            SYS_CLIDR_EL1 => Some((1 << 0) | (1 << 3) | (1 << 21) | (1 << 24) | (1 << 27)),
            // CTR_EL0: 64-byte cache line, L1 I/D caches present.
            SYS_CTR_EL0 => Some((4 << 16) | (4 << 0) | (1 << 14) | (1 << 30)),
            // CCSIDR_EL1: return a plausible L1 64KB 4-way associativity.
            // Raw format: (LineSize-4)<<0 | (Associativity-1)<<3 | (NumSets-1)<<13
            SYS_CCSIDR_EL1 => Some((3 << 13) | (3 << 3) | 3),
            // CSSELR_EL1: select L1 D-cache by default (0b00).
            SYS_CSSELR_EL1 => Some(0),
            _ => None,
        }
    }
}

fn emulate_el1_sysreg(
    trapframe: &mut Trapframe,
    guest: &mut GuestVcpu,
    sysreg: u32,
    reg: u8,
    is_read: bool,
) -> bool {
    let mut sysregs = guest.sysregs.clone();
    sysregs.take_pending_into();
    let value = match sysreg {
        SYS_TCR2_EL1 => &mut sysregs.tcr2_el1,
        SYS_PIRE0_EL1 => &mut sysregs.pire0_el1,
        SYS_PIR_EL1 => &mut sysregs.pir_el1,
        _ => return false,
    };

    if is_read {
        if reg < 31 {
            trapframe.regs.reg[reg as usize] = *value as usize;
        }
    } else if reg < 31 {
        *value = trapframe.regs.reg[reg as usize] as u64;
    }

    guest.sysregs = sysregs;
    trapframe.elr = trapframe.elr.wrapping_add(4);
    true
}

fn read_hpfar_el2() -> u64 {
    let hpfar: u64;
    // SAFETY: reading HPFAR_EL2 is valid while handling a guest trap at EL2.
    unsafe {
        asm!("mrs {0}, hpfar_el2", out(reg) hpfar, options(nostack));
    }
    hpfar
}

fn read_far_el2() -> u64 {
    let far: u64;
    // SAFETY: reading FAR_EL2 is valid while handling a guest trap at EL2.
    unsafe {
        asm!("mrs {0}, far_el2", out(reg) far, options(nostack));
    }
    far
}

fn guest_fault_ipa() -> u64 {
    let hpfar = read_hpfar_el2();
    let far = read_far_el2();
    ((hpfar & !0xf) << 8) | (far & 0xfff)
}

fn resolve_guest_hpa(gpa: u64, vm: &Vm) -> Option<u64> {
    let slot = vm.find_memory_slot(gpa)?;
    let userspace_addr = slot.gpa_to_userspace_addr(gpa) as usize;
    vm.owner_mm()
        .translate_to_phys(userspace_addr)
        .map(|p| p as u64)
}

fn handle_guest_stage2_fault(vm: &Vm, ipa: u64, writable: bool) -> Option<VmExit> {
    let Some(slot) = vm.find_memory_slot(ipa) else {
        return Some(VmExit::Unknown(ipa));
    };
    let Some(hpa) = resolve_guest_hpa(ipa, vm) else {
        return Some(VmExit::FailEntry {
            hardware_entry_failure_reason: 0,
        });
    };

    match vm.map_stage2_page(ipa, hpa, writable && !slot.flags.readonly) {
        Ok(()) => None,
        Err(_) => Some(VmExit::FailEntry {
            hardware_entry_failure_reason: 0,
        }),
    }
}

fn handle_host_irq_from_guest(
    trap_kind: usize,
    _trapframe: &Trapframe,
    _guest: &GuestVcpu,
) -> bool {
    let mut scratch = Trapframe::new();

    if trap_kind == 2 && crate::arch::interrupt::is_arch_timer_pending() {
        crate::timer::tick_with_scheduler(&mut scratch, false);
        return false;
    }

    let cpu_id = crate::arch::get_cpu().get_cpuid() as u32;
    match crate::interrupt::InterruptManager::global().claim_and_handle_external_interrupt(cpu_id) {
        Ok(Some(interrupt_id)) => {
            if interrupt_id == RESCHEDULE_SGI {
                crate::sched::scheduler::debug_log_reschedule_ipi(cpu_id as usize, false, true);
                return crate::sched::scheduler::has_ready_tasks(cpu_id as usize);
            } else if interrupt_id == crate::drivers::pic::arm_generic_timer::timer_ppi_irq() {
                crate::timer::tick_with_scheduler(&mut scratch, false);
            }
        }
        Ok(None) => {
            if crate::arch::interrupt::is_arch_timer_pending() {
                crate::timer::tick_with_scheduler(&mut scratch, false);
            }
        }
        Err(e) => {
            crate::println!("[AARCH64-HV] failed to handle host irq from guest: {}", e);
        }
    }

    false
}

pub fn is_from_guest() -> bool {
    let hcr: u64;
    // SAFETY: reading HCR_EL2 is side-effect free at EL2.
    unsafe {
        asm!("mrs {0}, hcr_el2", out(reg) hcr, options(nostack));
    }
    (hcr & HCR_EL2_VM) != 0
}

pub fn arch_guest_trap_handler(
    trapframe: &mut Trapframe,
    vm: &Vm,
    guest: &mut GuestVcpu,
) -> Option<VmExit> {
    guest.sysregs.take_pending_into();

    let esr = trapframe.esr_el1;
    if esr == 1 || esr == 2 {
        return if handle_host_irq_from_guest(esr as usize, trapframe, guest) {
            Some(VmExit::HostInterrupt)
        } else {
            None
        };
    }

    let ec = ((esr >> ESR_EC_SHIFT) & ESR_EC_MASK) as u32;
    let iss = (esr & ESR_ISS_MASK) as u32;

    match ec {
        ESR_EC_WFX => {
            trapframe.elr = trapframe.elr.wrapping_add(4);
            Some(VmExit::Wfi)
        }
        ESR_EC_HVC64 | ESR_EC_SMC64 => {
            let epc = trapframe.elr;
            trapframe.elr = trapframe.elr.wrapping_add(4);
            Some(VmExit::FirmwareCall { epc })
        }
        ESR_EC_SYS64 => {
            let epc = trapframe.elr;
            let sysreg = esr_sys64_to_sysreg(iss) as u64;
            let reg = ((iss >> ESR_SYS64_ISS_RT_SHIFT) & ESR_SYS64_ISS_RT_MASK) as u8;
            let is_read = (iss & ESR_SYS64_ISS_DIR_READ) != 0;

            if emulate_el1_sysreg(trapframe, guest, sysreg as u32, reg, is_read) {
                return None;
            }

            if emulate_timer_sysreg(trapframe, guest, sysreg as u32, reg, is_read) {
                return None;
            }

            if is_read && let Some(value) = virtual_id_sysreg_value(sysreg as u32) {
                if reg < 31 {
                    trapframe.regs.reg[reg as usize] = value as usize;
                }
                trapframe.elr = trapframe.elr.wrapping_add(4);
                return None;
            }

            if !is_timer_sysreg(sysreg as u32) {
                return Some(VmExit::IllegalInstruction {
                    epc,
                    inst: None,
                    inst_len: None,
                });
            }

            let data = if !is_read && reg < 31 {
                trapframe.regs.reg[reg as usize] as u64
            } else {
                0
            };

            trapframe.elr = trapframe.elr.wrapping_add(4);

            Some(if is_read {
                VmExit::MmioRead {
                    epc,
                    addr: sysreg,
                    size: 8,
                    reg,
                }
            } else {
                VmExit::MmioWrite {
                    epc,
                    addr: sysreg,
                    size: 8,
                    reg,
                    data,
                }
            })
        }
        ESR_EC_IABT_LOW => handle_guest_stage2_fault(vm, guest_fault_ipa(), false),
        ESR_EC_DABT_LOW => {
            let ipa = guest_fault_ipa();

            // Check registered in-kernel MMIO devices first
            if let Some(device) = vm.find_mmio_device(ipa) {
                let (base, _) = device.addr_range();
                let offset = ipa - base;
                let size = data_abort_access_size(iss);
                let reg = data_abort_srt(iss);
                let is_write = (iss & ESR_WNR) != 0;

                if is_write {
                    let data = if reg < 31 {
                        trapframe.regs.reg[reg as usize] as u64
                    } else {
                        0
                    };
                    device.write(offset, size, data);
                } else {
                    let value = device.read(offset, size);
                    if reg < 31 {
                        trapframe.regs.reg[reg as usize] = value as usize;
                    }
                }

                trapframe.elr = trapframe.elr.wrapping_add(4);
                return None; // Handled in-kernel, continue guest
            }

            let is_gic_mmio = if let Some((dist_base, dist_size, cpu_base)) = vm.gic_mmio_range() {
                let in_dist = ipa >= dist_base && ipa < dist_base + dist_size;
                let in_cpu = cpu_base.is_some_and(|cpu| ipa >= cpu && ipa < cpu + GIC_CPUI_SIZE);
                in_dist || in_cpu
            } else {
                false
            };
            if is_gic_mmio {
                let epc = trapframe.elr;
                let size = data_abort_access_size(iss);
                let reg = data_abort_srt(iss);
                let is_write = (iss & ESR_WNR) != 0;
                let data = if is_write && reg < 31 {
                    trapframe.regs.reg[reg as usize] as u64
                } else {
                    0
                };

                trapframe.elr = trapframe.elr.wrapping_add(4);

                return Some(if is_write {
                    VmExit::MmioWrite {
                        epc,
                        addr: ipa,
                        size,
                        reg,
                        data,
                    }
                } else {
                    VmExit::MmioRead {
                        epc,
                        addr: ipa,
                        size,
                        reg,
                    }
                });
            }

            if vm.find_memory_slot(ipa).is_some() {
                return handle_guest_stage2_fault(vm, ipa, (iss & ESR_WNR) != 0);
            }

            let epc = trapframe.elr;
            let size = data_abort_access_size(iss);
            let reg = data_abort_srt(iss);
            let is_write = (iss & ESR_WNR) != 0;
            let data = if is_write && reg < 31 {
                trapframe.regs.reg[reg as usize] as u64
            } else {
                0
            };

            trapframe.elr = trapframe.elr.wrapping_add(4);

            Some(if is_write {
                VmExit::MmioWrite {
                    epc,
                    addr: ipa,
                    size,
                    reg,
                    data,
                }
            } else {
                VmExit::MmioRead {
                    epc,
                    addr: ipa,
                    size,
                    reg,
                }
            })
        }
        ESR_EC_ILL => Some(VmExit::IllegalInstruction {
            epc: trapframe.elr,
            inst: None,
            inst_len: None,
        }),
        ESR_EC_BREAKPT_LOW | ESR_EC_BRK64 => Some(VmExit::Breakpoint { epc: trapframe.elr }),
        _ => {
            let count = UNKNOWN_GUEST_TRAP_DEBUG_COUNT.fetch_add(1, Ordering::Relaxed);
            if count < 32 {
                crate::println!(
                    "[AARCH64-HV] unknown trap esr={:#x} ec={:#x} iss={:#x} elr={:#x} spsr={:#x} far={:#x} hpfar={:#x}",
                    esr,
                    ec,
                    iss,
                    trapframe.elr,
                    trapframe.spsr,
                    read_far_el2(),
                    read_hpfar_el2()
                );
            }
            Some(VmExit::Unknown(esr))
        }
    }
}

pub fn clear_guest_mode() {
    let host_hcr = current_host_hv_context().hcr_el2;
    let restore_hcr = if host_hcr == 0 {
        HCR_EL2_HOST
    } else {
        host_hcr
    };

    // SAFETY: restoring the host HCR_EL2 state and clearing VTTBR_EL2 is valid at EL2.
    unsafe {
        asm!(
            "msr hcr_el2, {restore_hcr}",
            "msr vttbr_el2, xzr",
            "isb",
            restore_hcr = in(reg) restore_hcr,
            options(nostack),
        );
    }
}
