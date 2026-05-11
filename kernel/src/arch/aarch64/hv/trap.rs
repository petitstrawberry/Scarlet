use core::arch::asm;

use super::switch::{HCR_EL2_HOST, HCR_EL2_VM, HOST_HV_CTX};
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

const SYS_CNTV_CTL_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7) | (3 << 3) | 1;
const SYS_CNTV_CVAL_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7) | (3 << 3) | 2;
const SYS_CNTV_TVAL_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7) | (3 << 3) | 0;
const SYS_CNTVCT_EL0: u32 = (3 << 14) | (3 << 11) | (14 << 7) | (0 << 3) | 2;

const GIC_DIST_BASE: u64 = 0x0800_0000;
const GIC_DIST_SIZE: u64 = 0x1_0000;
const GIC_REDIST_BASE: u64 = 0x080A_0000;
const GIC_REDIST_SIZE: u64 = 0x00F6_0000;

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
        SYS_CNTV_CTL_EL0 | SYS_CNTV_CVAL_EL0 | SYS_CNTV_TVAL_EL0 | SYS_CNTVCT_EL0
    )
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

pub fn is_from_guest() -> bool {
    let hcr: u64;
    // SAFETY: reading HCR_EL2 is side-effect free at EL2.
    unsafe {
        asm!("mrs {0}, hcr_el2", out(reg) hcr, options(nostack));
    }
    (hcr & HCR_EL2_VM) != 0
}

pub fn arch_guest_trap_handler(trapframe: &mut Trapframe, vm: &Vm) -> Option<VmExit> {
    let esr = trapframe.esr_el1;
    let ec = ((esr >> ESR_EC_SHIFT) & ESR_EC_MASK) as u32;
    let iss = (esr & ESR_ISS_MASK) as u32;

    crate::println!(
        "[vmexit] ESR={:#x} EC={:#x} ISS={:#x} ELR={:#x}",
        esr,
        ec,
        iss,
        trapframe.elr
    );

    match ec {
        ESR_EC_WFX => {
            trapframe.elr = trapframe.elr.wrapping_add(4);
            Some(VmExit::Hlt)
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
        ESR_EC_IABT_LOW => {
            let ipa = guest_fault_ipa();
            crate::println!("[vmexit] IABT_LOW ipa={:#x}", ipa);
            let result = handle_guest_stage2_fault(vm, ipa, false);
            crate::println!("[vmexit] IABT_LOW result={:?}", result.is_some());
            result
        }
        ESR_EC_DABT_LOW => {
            let ipa = guest_fault_ipa();
            if (ipa >= GIC_DIST_BASE && ipa < GIC_DIST_BASE + GIC_DIST_SIZE)
                || (ipa >= GIC_REDIST_BASE && ipa < GIC_REDIST_BASE + GIC_REDIST_SIZE)
            {
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
        _ => Some(VmExit::Unknown(esr)),
    }
}

pub fn clear_guest_mode() {
    // SAFETY: reading the saved host HCR_EL2 value is part of the EL2 guest-exit path.
    let host_hcr = unsafe { core::ptr::addr_of!(HOST_HV_CTX.hcr_el2).read_volatile() };
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
