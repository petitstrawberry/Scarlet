//! AArch64 KVM register conversion and arch-specific hooks

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::{Once, RwLock};

use crate::arch::hv::reg_index::reg;
use crate::hypervisor::VcpuObject;
use crate::hypervisor::types::VmExit;

use super::{KVM_EXIT_SHUTDOWN, KVM_EXIT_SYSTEM_EVENT, KvmRun};
use crate::abi::linux::generic::LinuxAbi;
use crate::hypervisor::VmRef;
use crate::task::mytask;

// ---------------------------------------------------------------------------
// KvmRegs / KvmOneReg (C-compatible layouts)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmRegs {
    pub regs: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub pstate: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmOneReg {
    pub id: u64,
    pub addr: u64,
}

// ---------------------------------------------------------------------------
// KVM register ID decoding constants
// ---------------------------------------------------------------------------

const KVM_REG_ARCH_MASK: u64 = 0xff00_0000_0000_0000;
const KVM_REG_ARM64: u64 = 0x6000_0000_0000_0000;
const KVM_REG_SIZE_MASK: u64 = 0x00f0_0000_0000_0000;
const KVM_REG_SIZE_U64: u64 = 0x0030_0000_0000_0000;
const KVM_REG_SIZE_U32: u64 = 0x0020_0000_0000_0000;

const KVM_REG_ARM_COPROC_MASK: u64 = 0x0000_0000_0fff_0000;
const KVM_REG_ARM_COPROC_SHIFT: u64 = 16;

const KVM_REG_ARM_CORE: u64 = 0x0010 << KVM_REG_ARM_COPROC_SHIFT;
const KVM_REG_ARM64_SYSREG: u64 = 0x0013 << KVM_REG_ARM_COPROC_SHIFT;
const KVM_REG_ARM_FW: u64 = 0x0014 << KVM_REG_ARM_COPROC_SHIFT;

const KVM_REG_ARM64_SYSREG_OP0_SHIFT: u64 = 14;
const KVM_REG_ARM64_SYSREG_OP1_SHIFT: u64 = 11;
const KVM_REG_ARM64_SYSREG_CRN_SHIFT: u64 = 7;
const KVM_REG_ARM64_SYSREG_CRM_SHIFT: u64 = 3;
const KVM_REG_ARM64_SYSREG_OP2_SHIFT: u64 = 0;

const KVM_ARM_IRQ_TYPE_SHIFT: u32 = 24;
const KVM_ARM_IRQ_TYPE_SPI: u32 = 1;
const KVM_ARM_SPI_START: u32 = 32;

// ---------------------------------------------------------------------------
// PSCI constants (ARM DEN 0022E)
// ---------------------------------------------------------------------------

const PSCI_VERSION_1_1: u64 = (1 << 16) | 1;
const PSCI_RET_NOT_SUPPORTED: u64 = 0xFFFF_FFFF_FFFF_FFFF;
const PSCI_RET_DENIED: u64 = 0xFFFF_FFFF_FFFF_FFFD;

const PSCI_FN_VERSION: u64 = 0x84000000;
const PSCI_FN_CPU_OFF: u64 = 0x84000002;
const PSCI_FN_CPU_ON: u64 = 0x84000003;
const PSCI_FN_SYSTEM_OFF: u64 = 0x84000008;
const PSCI_FN_SYSTEM_RESET: u64 = 0x84000009;

const PSCI_FN64_CPU_ON: u64 = 0xC4000003;
const PSCI_FN64_CPU_OFF: u64 = 0xC4000002;
const PSCI_FN64_SYSTEM_OFF: u64 = 0xC4000008;
const PSCI_FN64_SYSTEM_RESET: u64 = 0xC4000009;

const SMCCC_RET_NOT_SUPPORTED: u64 = 0xFFFF_FFFF_FFFF_FFFF;
static SMCCC_HELPER_RETURN_PC: AtomicU64 = AtomicU64::new(0);
static KVM_ARM_PSCI_VERSION: AtomicU64 = AtomicU64::new(PSCI_VERSION_1_1);

const KVM_REG_ARM_PSCI_VERSION: u64 = KVM_REG_ARM64 | KVM_REG_SIZE_U64 | KVM_REG_ARM_FW;
const KVM_REG_ARM_SMCCC_ARCH_WORKAROUND_1: u64 =
    KVM_REG_ARM64 | KVM_REG_SIZE_U64 | KVM_REG_ARM_FW | 1;
const KVM_REG_ARM_SMCCC_ARCH_WORKAROUND_1_NOT_REQUIRED: u64 = 2;

// ---------------------------------------------------------------------------
// In-kernel PSCI state (tracks per-VM PSCI SYSTEM_OFF/RESET results)
// ---------------------------------------------------------------------------

/// Result of an in-kernel firmware call handling.
pub enum FirmwareCallResult {
    /// Handled entirely in-kernel. Guest registers updated; re-enter guest.
    Handled,
    /// PSCI SYSTEM_OFF — should exit to userspace as KVM_EXIT_SYSTEM_EVENT.
    SystemOff,
    /// PSCI SYSTEM_RESET — should exit to userspace as KVM_EXIT_SYSTEM_EVENT.
    SystemReset,
    /// Not a PSCI call; forward to userspace as KVM_EXIT_MMIO or similar.
    ForwardToUserspace,
}

fn finish_smccc_function_call(vcpu: &dyn VcpuObject) {
    let pc = vcpu.get_reg(reg::PC).unwrap_or(0);
    let helper_pc = SMCCC_HELPER_RETURN_PC.load(Ordering::Acquire);
    let x8 = vcpu.get_reg(reg::X8).unwrap_or(0);

    if helper_pc == 0 && x8 != 0 {
        SMCCC_HELPER_RETURN_PC.store(pc, Ordering::Release);
    }

    if pc == SMCCC_HELPER_RETURN_PC.load(Ordering::Acquire) && x8 != 0 {
        // Linux's arm64 SMCCC helper exits to KVM after the stack load and
        // resumes at the result store. Preserve that PC and provide the result
        // pointer through X4 so the helper can complete normally.
        let _ = vcpu.set_reg(reg::X4, x8);
    }
}

// ---------------------------------------------------------------------------
// KVM register read/write (bulk)
// ---------------------------------------------------------------------------

fn validate_one_reg_id(id: u64) -> Result<(), ()> {
    if (id & KVM_REG_ARCH_MASK) != KVM_REG_ARM64 {
        return Err(());
    }

    match id & KVM_REG_SIZE_MASK {
        KVM_REG_SIZE_U64 | KVM_REG_SIZE_U32 => Ok(()),
        _ => Err(()),
    }
}

fn kvm_reg_type(id: u64) -> u64 {
    id & KVM_REG_ARM_COPROC_MASK
}

fn kvm_reg_index(id: u64) -> u64 {
    id & 0x0000_ffff
}

const KVM_CORE_REGS_SP: u64 = 62;
const KVM_CORE_REGS_PC: u64 = 64;
const KVM_CORE_REGS_PSTATE: u64 = 66;
const KVM_CORE_SP_EL1: u64 = 68;
const KVM_CORE_ELR_EL1: u64 = 70;
const KVM_CORE_SPSR_BASE: u64 = 72;
const KVM_NR_SPSR: usize = 5;

const fn encode_sysreg(op0: u64, op1: u64, crn: u64, crm: u64, op2: u64) -> u64 {
    KVM_REG_ARM64
        | KVM_REG_SIZE_U64
        | KVM_REG_ARM64_SYSREG
        | (op0 << KVM_REG_ARM64_SYSREG_OP0_SHIFT)
        | (op1 << KVM_REG_ARM64_SYSREG_OP1_SHIFT)
        | (crn << KVM_REG_ARM64_SYSREG_CRN_SHIFT)
        | (crm << KVM_REG_ARM64_SYSREG_CRM_SHIFT)
        | (op2 << KVM_REG_ARM64_SYSREG_OP2_SHIFT)
}

// Whitelisted system registers that kvmtool / Linux guest need.
// Format: (op0, op1, crn, crm, op2) — matches ARM sys_reg() encoding.
//
// Timer registers (accessed via EL1 trap in guest):
const SYSREG_CNTV_CTL_EL0: u64 = encode_sysreg(3, 3, 14, 3, 1);
const SYSREG_CNTV_CVAL_EL0: u64 = encode_sysreg(3, 3, 14, 3, 2);
const SYSREG_CNTV_TVAL_EL0: u64 = encode_sysreg(3, 3, 14, 3, 0);
// MPIDR (read-only in guest, but kvmtool may read it):
const SYSREG_MPIDR_EL1: u64 = encode_sysreg(3, 0, 0, 0, 5);
// EL1 system registers:
const SYSREG_SCTLR_EL1: u64 = encode_sysreg(3, 0, 1, 0, 0);
const SYSREG_VBAR_EL1: u64 = encode_sysreg(3, 0, 12, 0, 0);
const SYSREG_TCR_EL1: u64 = encode_sysreg(3, 0, 2, 0, 2);
const SYSREG_TTBR0_EL1: u64 = encode_sysreg(3, 0, 2, 0, 0);
const SYSREG_TTBR1_EL1: u64 = encode_sysreg(3, 0, 2, 0, 1);
const SYSREG_MAIR_EL1: u64 = encode_sysreg(3, 0, 10, 2, 0);
const SYSREG_AMAIR_EL1: u64 = encode_sysreg(3, 0, 10, 3, 0);
const SYSREG_ESR_EL1: u64 = encode_sysreg(3, 0, 5, 2, 0);
const SYSREG_FAR_EL1: u64 = encode_sysreg(3, 0, 6, 0, 0);
const SYSREG_ELR_EL1: u64 = encode_sysreg(3, 0, 4, 0, 1);
const SYSREG_SPSR_EL1: u64 = encode_sysreg(3, 0, 4, 0, 0);
const SYSREG_SP_EL1: u64 = encode_sysreg(3, 4, 4, 1, 0);
const SYSREG_CPACR_EL1: u64 = encode_sysreg(3, 0, 1, 0, 2);
const SYSREG_CONTEXTIDR_EL1: u64 = encode_sysreg(3, 0, 13, 0, 1);
const SYSREG_CNTKCTL_EL1: u64 = encode_sysreg(3, 0, 14, 1, 0);
const SYSREG_CNTVOFF_EL2: u64 = encode_sysreg(3, 4, 14, 0, 3);
const SYSREG_ID_AA64DFR0_EL1: u64 = encode_sysreg(3, 0, 0, 5, 0);
const SYSREG_ID_AA64ISAR0_EL1: u64 = encode_sysreg(3, 0, 0, 6, 0);
const SYSREG_ID_AA64ISAR1_EL1: u64 = encode_sysreg(3, 0, 0, 6, 1);
const SYSREG_ID_AA64MMFR0_EL1: u64 = encode_sysreg(3, 0, 0, 7, 0);
const SYSREG_ID_AA64MMFR1_EL1: u64 = encode_sysreg(3, 0, 0, 7, 1);
const SYSREG_ID_AA64MMFR2_EL1: u64 = encode_sysreg(3, 0, 0, 7, 2);
const SYSREG_ID_AA64PFR0_EL1: u64 = encode_sysreg(3, 0, 0, 4, 0);

/// Per-vCPU KVM-only state for core registers that Scarlet does not execute.
#[derive(Clone, Copy)]
struct KvmArmVcpuSysregState {
    sp_el0: u64,
    spsr: [u64; 5],
}

impl Default for KvmArmVcpuSysregState {
    fn default() -> Self {
        Self {
            sp_el0: 0,
            spsr: [0; 5],
        }
    }
}

struct KvmArmVcpuSysregEntry {
    vcpu_key: usize,
    state: KvmArmVcpuSysregState,
}

static KVM_ARM_SYSREG_STATES: Once<RwLock<Vec<KvmArmVcpuSysregEntry>>> = Once::new();

fn get_sysreg_states() -> &'static RwLock<Vec<KvmArmVcpuSysregEntry>> {
    KVM_ARM_SYSREG_STATES.call_once(|| RwLock::new(Vec::new()))
}

fn with_sysreg_state<R>(
    vcpu: &dyn VcpuObject,
    f: impl FnOnce(&mut KvmArmVcpuSysregState) -> R,
) -> R {
    let key = super::vcpu_key(vcpu);
    let mut states = get_sysreg_states().write();
    if let Some(entry) = states.iter_mut().find(|e| e.vcpu_key == key) {
        return f(&mut entry.state);
    }

    states.push(KvmArmVcpuSysregEntry {
        vcpu_key: key,
        state: KvmArmVcpuSysregState::default(),
    });

    if let Some(entry) = states.last_mut() {
        f(&mut entry.state)
    } else {
        f(&mut KvmArmVcpuSysregState::default())
    }
}

// ---------------------------------------------------------------------------
// Public register API (used by shared KVM ioctl dispatch)
// ---------------------------------------------------------------------------

pub fn read_regs_to_kvm(vcpu: &dyn VcpuObject) -> KvmRegs {
    let mut regs = [0u64; 31];
    for (i, slot) in regs.iter_mut().enumerate() {
        *slot = vcpu.get_reg(i as u32).unwrap_or(0);
    }

    let sp = vcpu.get_reg(reg::SP).unwrap_or(0);
    let pc = vcpu.get_reg(reg::PC).unwrap_or(0);
    let pstate = vcpu.get_reg(reg::PSTATE).unwrap_or(0);

    KvmRegs {
        regs,
        sp,
        pc,
        pstate,
    }
}

pub fn write_kvm_to_regs(vcpu: &dyn VcpuObject, kvm_regs: &KvmRegs) {
    for (i, value) in kvm_regs.regs.iter().enumerate() {
        let _ = vcpu.set_reg(i as u32, *value);
    }
    let _ = vcpu.set_reg(reg::SP, kvm_regs.sp);
    let _ = vcpu.set_reg(reg::PC, kvm_regs.pc);
    let _ = vcpu.set_reg(reg::PSTATE, kvm_regs.pstate);
}

pub fn complete_mmio_read(vcpu: &dyn VcpuObject, target_reg: u8, size: u8, value: u64) {
    if target_reg >= 31 {
        return;
    }

    let mask = match size {
        1 => 0xff,
        2 => 0xffff,
        4 => 0xffff_ffff,
        _ => !0,
    };
    let _ = vcpu.set_reg(target_reg as u32, value & mask);
}

pub fn get_one_reg(vcpu: &dyn VcpuObject, id: u64) -> Result<u64, ()> {
    validate_one_reg_id(id)?;

    match kvm_reg_type(id) {
        KVM_REG_ARM_CORE => get_one_core_reg(vcpu, kvm_reg_index(id)),
        KVM_REG_ARM64_SYSREG => get_one_sysreg(vcpu, id),
        KVM_REG_ARM_FW => get_one_fw_reg(id),
        _ => Err(()),
    }
}

pub fn set_one_reg(vcpu: &dyn VcpuObject, id: u64, value: u64) -> Result<(), ()> {
    validate_one_reg_id(id)?;

    match kvm_reg_type(id) {
        KVM_REG_ARM_CORE => set_one_core_reg(vcpu, kvm_reg_index(id), value),
        KVM_REG_ARM64_SYSREG => set_one_sysreg(vcpu, id, value),
        KVM_REG_ARM_FW => set_one_fw_reg(id, value),
        _ => Err(()),
    }
}

fn get_one_fw_reg(id: u64) -> Result<u64, ()> {
    match id {
        KVM_REG_ARM_PSCI_VERSION => Ok(KVM_ARM_PSCI_VERSION.load(Ordering::Acquire)),
        KVM_REG_ARM_SMCCC_ARCH_WORKAROUND_1 => Ok(KVM_REG_ARM_SMCCC_ARCH_WORKAROUND_1_NOT_REQUIRED),
        _ => Err(()),
    }
}

fn set_one_fw_reg(id: u64, value: u64) -> Result<(), ()> {
    match id {
        KVM_REG_ARM_PSCI_VERSION => {
            KVM_ARM_PSCI_VERSION.store(value, Ordering::Release);
            Ok(())
        }
        KVM_REG_ARM_SMCCC_ARCH_WORKAROUND_1 => Ok(()),
        _ => Err(()),
    }
}

fn get_one_core_reg(vcpu: &dyn VcpuObject, index: u64) -> Result<u64, ()> {
    match index {
        idx if idx <= 60 && idx % 2 == 0 => vcpu.get_reg((idx / 2) as u32).map_err(|_| ()),
        KVM_CORE_REGS_SP => Ok(with_sysreg_state(vcpu, |s| s.sp_el0)),
        KVM_CORE_REGS_PC => vcpu.get_reg(reg::PC).map_err(|_| ()),
        KVM_CORE_REGS_PSTATE => vcpu.get_reg(reg::PSTATE).map_err(|_| ()),
        KVM_CORE_SP_EL1 => vcpu.get_reg(reg::SP_EL1).map_err(|_| ()),
        KVM_CORE_ELR_EL1 => vcpu.get_reg(reg::ELR_EL1).map_err(|_| ()),
        idx if idx >= KVM_CORE_SPSR_BASE
            && idx < KVM_CORE_SPSR_BASE + (KVM_NR_SPSR as u64 * 2)
            && idx % 2 == 0 =>
        {
            let spsr_index = ((idx - KVM_CORE_SPSR_BASE) / 2) as usize;
            if spsr_index == 0 {
                vcpu.get_reg(reg::SPSR_EL1).map_err(|_| ())
            } else {
                Ok(with_sysreg_state(vcpu, |s| s.spsr[spsr_index]))
            }
        }
        _ => Err(()),
    }
}

fn set_one_core_reg(vcpu: &dyn VcpuObject, index: u64, value: u64) -> Result<(), ()> {
    match index {
        idx if idx <= 60 && idx % 2 == 0 => vcpu.set_reg((idx / 2) as u32, value).map_err(|_| ()),
        KVM_CORE_REGS_SP => {
            with_sysreg_state(vcpu, |s| s.sp_el0 = value);
            Ok(())
        }
        KVM_CORE_REGS_PC => vcpu.set_reg(reg::PC, value).map_err(|_| ()),
        KVM_CORE_REGS_PSTATE => vcpu.set_reg(reg::PSTATE, value).map_err(|_| ()),
        KVM_CORE_SP_EL1 => vcpu.set_reg(reg::SP_EL1, value).map_err(|_| ()),
        KVM_CORE_ELR_EL1 => vcpu.set_reg(reg::ELR_EL1, value).map_err(|_| ()),
        idx if idx >= KVM_CORE_SPSR_BASE
            && idx < KVM_CORE_SPSR_BASE + (KVM_NR_SPSR as u64 * 2)
            && idx % 2 == 0 =>
        {
            let spsr_index = ((idx - KVM_CORE_SPSR_BASE) / 2) as usize;
            with_sysreg_state(vcpu, |s| s.spsr[spsr_index] = value);
            if spsr_index == 0 {
                vcpu.set_reg(reg::SPSR_EL1, value).map_err(|_| ())
            } else {
                Ok(())
            }
        }
        _ => Err(()),
    }
}

/// Map a decoded sysreg ID to a readable value. Returns Err for unknown regs.
fn get_one_sysreg(vcpu: &dyn VcpuObject, id: u64) -> Result<u64, ()> {
    match id {
        SYSREG_CNTV_CTL_EL0 => vcpu.get_reg(reg::CNTV_CTL_EL0).map_err(|_| ()),
        SYSREG_CNTV_CVAL_EL0 => vcpu.get_reg(reg::CNTV_CVAL_EL0).map_err(|_| ()),
        SYSREG_CNTV_TVAL_EL0 => Ok(0),
        SYSREG_CNTVOFF_EL2 => vcpu.get_reg(reg::CNTVOFF_EL2).map_err(|_| ()),
        // EL1 system registers — accessible via vcpu get_reg for known indices
        SYSREG_MPIDR_EL1 => {
            // Default MPIDR for single CPU: Aff0=0, Aff1=0, Aff2=0, MT=0, RES1=bit31
            Ok(0x80000000)
        }
        SYSREG_SCTLR_EL1 => vcpu.get_reg(reg::SCTLR_EL1).map_err(|_| ()),
        SYSREG_VBAR_EL1 => vcpu.get_reg(reg::VBAR_EL1).map_err(|_| ()),
        SYSREG_TCR_EL1 => vcpu.get_reg(reg::TCR_EL1).map_err(|_| ()),
        SYSREG_TTBR0_EL1 => vcpu.get_reg(reg::TTBR0_EL1).map_err(|_| ()),
        SYSREG_TTBR1_EL1 => vcpu.get_reg(reg::TTBR1_EL1).map_err(|_| ()),
        SYSREG_MAIR_EL1 => vcpu.get_reg(reg::MAIR_EL1).map_err(|_| ()),
        SYSREG_AMAIR_EL1 => vcpu.get_reg(reg::AMAIR_EL1).map_err(|_| ()),
        SYSREG_ESR_EL1 => vcpu.get_reg(reg::ESR_EL1).map_err(|_| ()),
        SYSREG_FAR_EL1 => vcpu.get_reg(reg::FAR_EL1).map_err(|_| ()),
        SYSREG_ELR_EL1 => vcpu.get_reg(reg::ELR_EL1).map_err(|_| ()),
        SYSREG_SPSR_EL1 => vcpu.get_reg(reg::SPSR_EL1).map_err(|_| ()),
        SYSREG_SP_EL1 => vcpu.get_reg(reg::SP_EL1).map_err(|_| ()),
        SYSREG_CPACR_EL1 => vcpu.get_reg(reg::CPACR_EL1).map_err(|_| ()),
        SYSREG_CONTEXTIDR_EL1 => vcpu.get_reg(reg::CONTEXTIDR_EL1).map_err(|_| ()),
        SYSREG_CNTKCTL_EL1 => vcpu.get_reg(reg::CNTKCTL_EL1).map_err(|_| ()),
        // ID registers — return safe defaults
        SYSREG_ID_AA64PFR0_EL1 => Ok(0x00000011), // EL0=1,EL1=1
        SYSREG_ID_AA64DFR0_EL1 => Ok(0),
        SYSREG_ID_AA64ISAR0_EL1 => Ok(0),
        SYSREG_ID_AA64ISAR1_EL1 => Ok(0),
        SYSREG_ID_AA64MMFR0_EL1 => Ok(0x00000000), // PARange=32bit
        SYSREG_ID_AA64MMFR1_EL1 => Ok(0),
        SYSREG_ID_AA64MMFR2_EL1 => Ok(0),
        _ => Err(()),
    }
}

/// Map a decoded sysreg ID to a writable target. Returns Err for unknown/read-only regs.
fn set_one_sysreg(vcpu: &dyn VcpuObject, id: u64, value: u64) -> Result<(), ()> {
    match id {
        // Timer registers
        SYSREG_CNTV_CTL_EL0 => vcpu.set_reg(reg::CNTV_CTL_EL0, value).map_err(|_| ()),
        SYSREG_CNTV_CVAL_EL0 => vcpu.set_reg(reg::CNTV_CVAL_EL0, value).map_err(|_| ()),
        SYSREG_CNTV_TVAL_EL0 => vcpu.set_reg(reg::CNTV_CVAL_EL0, value).map_err(|_| ()),
        SYSREG_CNTVOFF_EL2 => vcpu.set_reg(reg::CNTVOFF_EL2, value).map_err(|_| ()),
        // EL1 system registers
        SYSREG_SCTLR_EL1 => vcpu.set_reg(reg::SCTLR_EL1, value).map_err(|_| ()),
        SYSREG_VBAR_EL1 => vcpu.set_reg(reg::VBAR_EL1, value).map_err(|_| ()),
        SYSREG_TCR_EL1 => vcpu.set_reg(reg::TCR_EL1, value).map_err(|_| ()),
        SYSREG_TTBR0_EL1 => vcpu.set_reg(reg::TTBR0_EL1, value).map_err(|_| ()),
        SYSREG_TTBR1_EL1 => vcpu.set_reg(reg::TTBR1_EL1, value).map_err(|_| ()),
        SYSREG_MAIR_EL1 => vcpu.set_reg(reg::MAIR_EL1, value).map_err(|_| ()),
        SYSREG_AMAIR_EL1 => vcpu.set_reg(reg::AMAIR_EL1, value).map_err(|_| ()),
        SYSREG_ESR_EL1 => vcpu.set_reg(reg::ESR_EL1, value).map_err(|_| ()),
        SYSREG_FAR_EL1 => vcpu.set_reg(reg::FAR_EL1, value).map_err(|_| ()),
        SYSREG_ELR_EL1 => vcpu.set_reg(reg::ELR_EL1, value).map_err(|_| ()),
        SYSREG_SPSR_EL1 => vcpu.set_reg(reg::SPSR_EL1, value).map_err(|_| ()),
        SYSREG_SP_EL1 => vcpu.set_reg(reg::SP_EL1, value).map_err(|_| ()),
        SYSREG_CPACR_EL1 => vcpu.set_reg(reg::CPACR_EL1, value).map_err(|_| ()),
        SYSREG_CONTEXTIDR_EL1 => vcpu.set_reg(reg::CONTEXTIDR_EL1, value).map_err(|_| ()),
        SYSREG_CNTKCTL_EL1 => vcpu.set_reg(reg::CNTKCTL_EL1, value).map_err(|_| ()),
        // Read-only registers
        SYSREG_MPIDR_EL1
        | SYSREG_ID_AA64PFR0_EL1
        | SYSREG_ID_AA64DFR0_EL1
        | SYSREG_ID_AA64ISAR0_EL1
        | SYSREG_ID_AA64ISAR1_EL1
        | SYSREG_ID_AA64MMFR0_EL1
        | SYSREG_ID_AA64MMFR1_EL1
        | SYSREG_ID_AA64MMFR2_EL1 => Err(()),
        _ => Err(()),
    }
}

// ---------------------------------------------------------------------------
// Arch hook: in-kernel firmware call handling (PSCI)
// ---------------------------------------------------------------------------

/// Handle AArch64 firmware calls (HVC/SMC → PSCI) entirely inside the kernel.
///
/// Returns `Handled` if the call was processed and guest registers updated,
/// `SystemOff` / `SystemReset` for PSCI shutdown (caller should exit to userspace),
/// or `ForwardToUserspace` if the call should be forwarded.
pub fn handle_firmware_call_in_kernel(vcpu: &dyn VcpuObject) -> FirmwareCallResult {
    let function_id = vcpu.get_reg(reg::X0).unwrap_or(0);

    match function_id {
        PSCI_FN_VERSION => {
            let _ = vcpu.set_reg(reg::X0, PSCI_VERSION_1_1);
            finish_smccc_function_call(vcpu);
            FirmwareCallResult::Handled
        }
        PSCI_FN_CPU_OFF | PSCI_FN64_CPU_OFF => {
            // Single vCPU: refuse CPU_OFF
            let _ = vcpu.set_reg(reg::X0, PSCI_RET_DENIED);
            finish_smccc_function_call(vcpu);
            FirmwareCallResult::Handled
        }
        PSCI_FN_CPU_ON | PSCI_FN64_CPU_ON => {
            // Multi-vCPU not yet supported
            let _ = vcpu.set_reg(reg::X0, PSCI_RET_NOT_SUPPORTED);
            finish_smccc_function_call(vcpu);
            FirmwareCallResult::Handled
        }
        PSCI_FN_SYSTEM_OFF | PSCI_FN64_SYSTEM_OFF => FirmwareCallResult::SystemOff,
        PSCI_FN_SYSTEM_RESET | PSCI_FN64_SYSTEM_RESET => FirmwareCallResult::SystemReset,
        _ => {
            // Unknown PSCI / SMCCC function — return NOT_SUPPORTED
            let _ = vcpu.set_reg(reg::X0, SMCCC_RET_NOT_SUPPORTED);
            finish_smccc_function_call(vcpu);
            FirmwareCallResult::Handled
        }
    }
}

// ---------------------------------------------------------------------------
// Arch hook: write firmware exit into kvm_run
// ---------------------------------------------------------------------------

/// Write architecture-specific exit data for firmware calls.
/// On AArch64, PSCI SYSTEM_OFF/RESET maps to KVM_EXIT_SYSTEM_EVENT.
pub fn write_firmware_exit(kvm_run: &mut KvmRun, exit: &VmExit, vcpu: &dyn VcpuObject) {
    if let VmExit::FirmwareCall { epc: _ } = exit {
        let function_id = vcpu.get_reg(reg::X0).unwrap_or(0);
        match function_id {
            PSCI_FN_SYSTEM_OFF | PSCI_FN64_SYSTEM_OFF => {
                kvm_run.exit_reason = KVM_EXIT_SYSTEM_EVENT;
                let sys_event = unsafe { &mut kvm_run.exit_data.system_event };
                // KVM_SYSTEM_EVENT_SHUTDOWN = 0
                sys_event.event_type = 0;
                sys_event.ndata = 0;
                sys_event.data = [0u64; 16];
            }
            PSCI_FN_SYSTEM_RESET | PSCI_FN64_SYSTEM_RESET => {
                kvm_run.exit_reason = KVM_EXIT_SYSTEM_EVENT;
                let sys_event = unsafe { &mut kvm_run.exit_data.system_event };
                // KVM_SYSTEM_EVENT_RESET = 1
                sys_event.event_type = 1;
                sys_event.ndata = 0;
                sys_event.data = [0u64; 16];
            }
            _ => {
                // Unknown firmware call — treat as shutdown
                kvm_run.exit_reason = KVM_EXIT_SHUTDOWN;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Arch hook: KVM_CHECK_EXTENSION (AArch64-specific caps)
// ---------------------------------------------------------------------------

/// Handle AArch64-specific KVM capability queries.
/// Returns `Some(value)` if recognized, `None` if not an AArch64 cap.
pub fn check_extension(cap: usize) -> Option<usize> {
    const KVM_CAP_ARM_VM_IPA_SIZE: usize = 165;
    const KVM_CAP_ARM_PSCI: usize = 87;
    const KVM_CAP_ARM_SET_DEVICE_ADDR: usize = 88;
    const KVM_CAP_ARM_PSCI_0_2: usize = 102;

    match cap {
        KVM_CAP_ARM_VM_IPA_SIZE => Some(40),
        KVM_CAP_ARM_PSCI => Some(1),
        KVM_CAP_ARM_SET_DEVICE_ADDR => Some(1),
        KVM_CAP_ARM_PSCI_0_2 => Some(1),
        _ => None,
    }
}

pub fn validate_device_type(device_type: u32) -> Result<(), ()> {
    match device_type {
        KVM_DEV_TYPE_ARM_VGIC_V3 | KVM_DEV_TYPE_ARM_VGIC_ITS => Ok(()),
        _ => Err(()),
    }
}

pub fn default_device_type() -> u32 {
    KVM_DEV_TYPE_ARM_VGIC_V3
}

pub fn irqfd_route_to_vcpu_irq(route: u32) -> u32 {
    (KVM_ARM_IRQ_TYPE_SPI << KVM_ARM_IRQ_TYPE_SHIFT) | (KVM_ARM_SPI_START + route)
}

pub fn set_device_attr(
    vm: &VmRef,
    device_type: u32,
    attr: &super::KvmDeviceAttr,
) -> Result<Option<usize>, ()> {
    match (device_type, attr.group) {
        (KVM_DEV_TYPE_ARM_VGIC_ITS, KVM_DEV_ARM_VGIC_GRP_ADDR) => {
            if attr.attr == KVM_VGIC_ITS_ADDR_TYPE {
                if attr.addr == 0 {
                    return Err(());
                }
                let task = crate::task::mytask().ok_or(())?;
                let kva = task
                    .vm_manager
                    .translate_to_kva(attr.addr as usize)
                    .ok_or(())?;
                // SAFETY: caller guarantees addr points to a valid u64
                let addr = unsafe { core::ptr::read_volatile(kva as *const u64) };
                // crate::println!("[KVM] VGICv3 ITS addr={:#x}", addr);
                Ok(Some(0))
            } else {
                Err(())
            }
        }
        (KVM_DEV_TYPE_ARM_VGIC_ITS, KVM_DEV_ARM_VGIC_GRP_CTRL) => match attr.attr {
            KVM_DEV_ARM_VGIC_CTRL_INIT => {
                // crate::println!("[KVM] VGICv3 ITS INIT");
                Ok(Some(0))
            }
            _ => Err(()),
        },
        (KVM_DEV_TYPE_ARM_VGIC_ITS, _) => Err(()),
        (_, KVM_DEV_ARM_VGIC_GRP_ADDR) => {
            if attr.addr == 0 {
                return Err(());
            }
            let task = crate::task::mytask().ok_or(())?;
            let kva = task
                .vm_manager
                .translate_to_kva(attr.addr as usize)
                .ok_or(())?;
            // SAFETY: caller guarantees addr points to a valid u64
            let addr = unsafe { core::ptr::read_volatile(kva as *const u64) };
            match attr.attr {
                KVM_VGIC_V3_ADDR_TYPE_DIST => {
                    vm.set_vgicv3_dist_addr(addr);
                    // crate::println!("[KVM] VGICv3 DIST addr={:#x}", addr);
                    Ok(Some(0))
                }
                KVM_VGIC_V3_ADDR_TYPE_REDIST => {
                    vm.set_vgicv3_redist_addr(addr);
                    // crate::println!("[KVM] VGICv3 REDIST addr={:#x}", addr);
                    Ok(Some(0))
                }
                _ => Err(()),
            }
        }
        (_, KVM_DEV_ARM_VGIC_GRP_NR_IRQS) => {
            if attr.addr == 0 {
                return Err(());
            }
            let task = crate::task::mytask().ok_or(())?;
            let kva = task
                .vm_manager
                .translate_to_kva(attr.addr as usize)
                .ok_or(())?;
            // SAFETY: caller guarantees addr points to a valid u32
            let nr_irqs = unsafe { core::ptr::read_volatile(kva as *const u32) };
            vm.set_vgic_nr_irqs(nr_irqs);
            // crate::println!("[KVM] VGICv3 NR_IRQS={}", nr_irqs);
            Ok(Some(0))
        }
        (_, KVM_DEV_ARM_VGIC_GRP_CTRL) => match attr.attr {
            KVM_DEV_ARM_VGIC_CTRL_INIT => {
                vm.vgic_init()?;
                // crate::println!("[KVM] VGICv3 INIT");
                Ok(Some(0))
            }
            _ => Err(()),
        },
        (_, _) => Err(()),
    }
}

pub fn get_device_attr(
    _vm: &VmRef,
    _device_type: u32,
    _attr: &super::KvmDeviceAttr,
) -> Result<Option<usize>, ()> {
    Err(())
}

pub fn has_device_attr(
    _vm: &VmRef,
    device_type: u32,
    attr: &super::KvmDeviceAttr,
) -> Result<Option<usize>, ()> {
    match (device_type, attr.group) {
        (KVM_DEV_TYPE_ARM_VGIC_ITS, KVM_DEV_ARM_VGIC_GRP_ADDR) => match attr.attr {
            KVM_VGIC_ITS_ADDR_TYPE => Ok(Some(0)),
            _ => Err(()),
        },
        (KVM_DEV_TYPE_ARM_VGIC_ITS, KVM_DEV_ARM_VGIC_GRP_CTRL) => match attr.attr {
            KVM_DEV_ARM_VGIC_CTRL_INIT => Ok(Some(0)),
            _ => Err(()),
        },
        (KVM_DEV_TYPE_ARM_VGIC_ITS, _) => Err(()),
        (_, KVM_DEV_ARM_VGIC_GRP_ADDR) => match attr.attr {
            KVM_VGIC_V3_ADDR_TYPE_DIST | KVM_VGIC_V3_ADDR_TYPE_REDIST => Ok(Some(0)),
            _ => Err(()),
        },
        (_, KVM_DEV_ARM_VGIC_GRP_NR_IRQS) => Ok(Some(0)),
        (_, KVM_DEV_ARM_VGIC_GRP_CTRL) => match attr.attr {
            0 => Ok(Some(0)),
            _ => Err(()),
        },
        (_, _) => Err(()),
    }
}

// ---------------------------------------------------------------------------
// AArch64-specific ioctl numbers
// ---------------------------------------------------------------------------

const KVMIO: u32 = 0xAE;

const fn io_none(ty: u32, nr: u32) -> u32 {
    (ty << 8) | nr
}
const fn io_write(ty: u32, nr: u32, size: u32) -> u32 {
    (1 << 30) | (size << 16) | (ty << 8) | nr
}
const fn io_read(ty: u32, nr: u32, size: u32) -> u32 {
    (2 << 30) | (size << 16) | (ty << 8) | nr
}

const KVM_ARM_PREFERRED_TARGET: u32 = io_read(KVMIO, 0xAF, 32);
const KVM_ARM_VCPU_INIT: u32 = io_write(KVMIO, 0xAE, 32);
const KVM_ARM_SET_DEVICE_ADDR: u32 =
    io_write(KVMIO, 0xAB, core::mem::size_of::<KvmArmDeviceAddr>() as u32);
const KVM_ARM_VCPU_FINALIZE: u32 = io_write(KVMIO, 0xC2, core::mem::size_of::<i32>() as u32);

const KVM_ARM_TARGET_GENERIC_V8: u32 = 5;
const KVM_DEV_TYPE_ARM_VGIC_V3: u32 = 7;
const KVM_DEV_TYPE_ARM_VGIC_ITS: u32 = 8;
const KVM_DEV_ARM_VGIC_GRP_ADDR: u32 = 0;
const KVM_DEV_ARM_VGIC_GRP_NR_IRQS: u32 = 3;
const KVM_DEV_ARM_VGIC_GRP_CTRL: u32 = 4;
const KVM_VGIC_V3_ADDR_TYPE_DIST: u64 = 2;
const KVM_VGIC_V3_ADDR_TYPE_REDIST: u64 = 3;
const KVM_VGIC_ITS_ADDR_TYPE: u64 = 4;
const KVM_DEV_ARM_VGIC_CTRL_INIT: u64 = 0;

const KVM_ARM_VCPU_POWER_OFF_BIT: u32 = 0;

// ---------------------------------------------------------------------------
// AArch64-specific C-compatible struct layouts
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmVcpuInit {
    pub target: u32,
    pub features: [u32; 7],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmArmDeviceAddr {
    pub id: u64,
    pub addr: u64,
}

// ---------------------------------------------------------------------------
// Per-vCPU init state tracking
// ---------------------------------------------------------------------------

struct KvmArmVcpuInitEntry {
    vcpu_key: usize,
    powered_off: bool,
}

static KVM_ARM_VCPU_INIT_STATES: Once<RwLock<Vec<KvmArmVcpuInitEntry>>> = Once::new();

fn get_vcpu_init_states() -> &'static RwLock<Vec<KvmArmVcpuInitEntry>> {
    KVM_ARM_VCPU_INIT_STATES.call_once(|| RwLock::new(Vec::new()))
}

fn set_vcpu_powered_off(vcpu: &dyn VcpuObject, powered_off: bool) {
    let key = super::vcpu_key(vcpu);
    let mut states = get_vcpu_init_states().write();
    if let Some(entry) = states.iter_mut().find(|e| e.vcpu_key == key) {
        entry.powered_off = powered_off;
    } else {
        states.push(KvmArmVcpuInitEntry {
            vcpu_key: key,
            powered_off,
        });
    }
}

pub fn free_vcpu_state(vcpu: &dyn VcpuObject) {
    let key = super::vcpu_key(vcpu);
    get_sysreg_states()
        .write()
        .retain(|entry| entry.vcpu_key != key);
    get_vcpu_init_states()
        .write()
        .retain(|entry| entry.vcpu_key != key);
}

// ---------------------------------------------------------------------------
// Arch hook: ARM-specific VM-level ioctl dispatch
// ---------------------------------------------------------------------------

pub fn handle_vm_ioctl(
    request: u32,
    arg: usize,
    _vm: &VmRef,
    _abi: &mut LinuxAbi,
) -> Result<Option<usize>, ()> {
    match request {
        KVM_ARM_PREFERRED_TARGET => {
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmVcpuInit
            let init = unsafe { &mut *(kva as *mut KvmVcpuInit) };
            init.target = KVM_ARM_TARGET_GENERIC_V8;
            init.features = [0u32; 7];
            Ok(Some(0))
        }

        KVM_ARM_SET_DEVICE_ADDR => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmArmDeviceAddr
            let dev_addr = unsafe { &*(kva as *const KvmArmDeviceAddr) };
            const KVM_ARM_DEVICE_ID_SHIFT: u64 = 16;
            let device_id = (dev_addr.id >> KVM_ARM_DEVICE_ID_SHIFT) & 0xFFFF;
            let addr_type = dev_addr.id & 0xFFFF;
            _vm.set_gic_device_addr(device_id, addr_type, dev_addr.addr);
            Ok(Some(0))
        }

        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Arch hook: ARM-specific vCPU-level ioctl dispatch
// ---------------------------------------------------------------------------

pub fn handle_vcpu_ioctl(
    request: u32,
    arg: usize,
    vcpu: &dyn VcpuObject,
) -> Result<Option<usize>, ()> {
    match request {
        KVM_ARM_VCPU_INIT => {
            if arg == 0 {
                return Err(());
            }
            let task = mytask().ok_or(())?;
            let kva = task.vm_manager.translate_to_kva(arg).ok_or(())?;
            // SAFETY: caller guarantees arg points to a valid KvmVcpuInit
            let vcpu_init = unsafe { &*(kva as *const KvmVcpuInit) };

            let powered_off = vcpu_init.features[0] & (1 << KVM_ARM_VCPU_POWER_OFF_BIT) != 0;
            set_vcpu_powered_off(vcpu, powered_off);
            Ok(Some(0))
        }

        KVM_ARM_VCPU_FINALIZE => Ok(Some(0)),

        _ => Ok(None),
    }
}
