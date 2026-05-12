//! AArch64 KVM register conversion and arch-specific hooks

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Once, RwLock};

use crate::arch::hv::reg_index::reg;
use crate::hypervisor::VcpuRef;
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

const KVM_REG_ARM64_TYPE_MASK: u64 = 0x0000_0000_ff00_0000;
const KVM_REG_ARM64_TYPE_SHIFT: u64 = 24;

const KVM_REG_ARM64_CORE: u64 = 0x01 << KVM_REG_ARM64_TYPE_SHIFT;
const KVM_REG_ARM64_SYSREG: u64 = 0x03 << KVM_REG_ARM64_TYPE_SHIFT;

const KVM_REG_ARM64_SYSREG_OP0_SHIFT: u64 = 20;
const KVM_REG_ARM64_SYSREG_OP0_MASK: u64 = 0x3;
const KVM_REG_ARM64_SYSREG_OP1_SHIFT: u64 = 14;
const KVM_REG_ARM64_SYSREG_OP1_MASK: u64 = 0x7;
const KVM_REG_ARM64_SYSREG_CRN_SHIFT: u64 = 10;
const KVM_REG_ARM64_SYSREG_CRN_MASK: u64 = 0xf;
const KVM_REG_ARM64_SYSREG_CRM_SHIFT: u64 = 7;
const KVM_REG_ARM64_SYSREG_CRM_MASK: u64 = 0xf;
const KVM_REG_ARM64_SYSREG_OP2_SHIFT: u64 = 3;
const KVM_REG_ARM64_SYSREG_OP2_MASK: u64 = 0x7;

// ---------------------------------------------------------------------------
// PSCI constants (ARM DEN 0022E)
// ---------------------------------------------------------------------------

const PSCI_VERSION_1_1: u64 = (1 << 16) | 1;
const PSCI_RET_NOT_SUPPORTED: u64 = 0xFFFF_FFFF_FFFF_FFFE;
const PSCI_RET_DENIED: u64 = 0xFFFF_FFFF_FFFF_FFFC;

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
    id & KVM_REG_ARM64_TYPE_MASK
}

fn kvm_reg_index(id: u64) -> u64 {
    id & 0x0000_ffff
}

const fn encode_sysreg(op0: u64, op1: u64, crn: u64, crm: u64, op2: u64) -> u64 {
    KVM_REG_ARM64_SYSREG
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
const SYSREG_SP_EL1: u64 = encode_sysreg(3, 0, 4, 1, 0);
const SYSREG_CPACR_EL1: u64 = encode_sysreg(3, 0, 1, 0, 2);
const SYSREG_CONTEXTIDR_EL1: u64 = encode_sysreg(3, 0, 13, 0, 1);
const SYSREG_CNTVOFF_EL2: u64 = encode_sysreg(3, 4, 14, 0, 3);
const SYSREG_ID_AA64DFR0_EL1: u64 = encode_sysreg(3, 0, 0, 5, 0);
const SYSREG_ID_AA64ISAR0_EL1: u64 = encode_sysreg(3, 0, 0, 6, 0);
const SYSREG_ID_AA64ISAR1_EL1: u64 = encode_sysreg(3, 0, 0, 6, 1);
const SYSREG_ID_AA64MMFR0_EL1: u64 = encode_sysreg(3, 0, 0, 7, 0);
const SYSREG_ID_AA64MMFR1_EL1: u64 = encode_sysreg(3, 0, 0, 7, 1);
const SYSREG_ID_AA64MMFR2_EL1: u64 = encode_sysreg(3, 0, 0, 7, 2);
const SYSREG_ID_AA64PFR0_EL1: u64 = encode_sysreg(3, 0, 0, 4, 0);

/// Per-vCPU sysreg state for registers that cannot be accessed via the
/// generic VcpuRef::get_reg / set_reg interface. Stored separately so
/// KVM_GET_ONE_REG / KVM_SET_ONE_REG can read/write timer registers etc.
#[derive(Clone, Copy)]
struct KvmArmVcpuSysregState {
    cntv_ctl_el0: u64,
    cntv_cval_el0: u64,
    cntvoff_el2: u64,
}

impl Default for KvmArmVcpuSysregState {
    fn default() -> Self {
        Self {
            cntv_ctl_el0: 0,
            cntv_cval_el0: 0,
            cntvoff_el2: 0,
        }
    }
}

struct KvmArmVcpuSysregEntry {
    vcpu: VcpuRef,
    state: KvmArmVcpuSysregState,
}

static KVM_ARM_SYSREG_STATES: Once<RwLock<Vec<KvmArmVcpuSysregEntry>>> = Once::new();

fn get_sysreg_states() -> &'static RwLock<Vec<KvmArmVcpuSysregEntry>> {
    KVM_ARM_SYSREG_STATES.call_once(|| RwLock::new(Vec::new()))
}

fn with_sysreg_state<R>(vcpu: &VcpuRef, f: impl FnOnce(&mut KvmArmVcpuSysregState) -> R) -> R {
    let mut states = get_sysreg_states().write();
    if let Some(entry) = states.iter_mut().find(|e| Arc::ptr_eq(&e.vcpu, vcpu)) {
        return f(&mut entry.state);
    }

    states.push(KvmArmVcpuSysregEntry {
        vcpu: Arc::clone(vcpu),
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

pub fn read_regs_to_kvm(vcpu: &VcpuRef) -> KvmRegs {
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

pub fn write_kvm_to_regs(vcpu: &VcpuRef, kvm_regs: &KvmRegs) {
    for (i, value) in kvm_regs.regs.iter().enumerate() {
        let _ = vcpu.set_reg(i as u32, *value);
    }
    let _ = vcpu.set_reg(reg::SP, kvm_regs.sp);
    let _ = vcpu.set_reg(reg::PC, kvm_regs.pc);
    let _ = vcpu.set_reg(reg::PSTATE, kvm_regs.pstate);
}

pub fn get_one_reg(vcpu: &VcpuRef, id: u64) -> Result<u64, ()> {
    validate_one_reg_id(id)?;

    match kvm_reg_type(id) {
        KVM_REG_ARM64_CORE => match kvm_reg_index(id) {
            idx @ 0..=30 => vcpu.get_reg(idx as u32).map_err(|_| ()),
            31 => vcpu.get_reg(reg::SP).map_err(|_| ()),
            32 => vcpu.get_reg(reg::PC).map_err(|_| ()),
            33 => vcpu.get_reg(reg::PSTATE).map_err(|_| ()),
            _ => Err(()),
        },
        KVM_REG_ARM64_SYSREG => get_one_sysreg(vcpu, id),
        _ => Err(()),
    }
}

pub fn set_one_reg(vcpu: &VcpuRef, id: u64, value: u64) -> Result<(), ()> {
    validate_one_reg_id(id)?;

    match kvm_reg_type(id) {
        KVM_REG_ARM64_CORE => match kvm_reg_index(id) {
            idx @ 0..=30 => vcpu.set_reg(idx as u32, value).map_err(|_| ()),
            31 => vcpu.set_reg(reg::SP, value).map_err(|_| ()),
            32 => vcpu.set_reg(reg::PC, value).map_err(|_| ()),
            33 => vcpu.set_reg(reg::PSTATE, value).map_err(|_| ()),
            _ => Err(()),
        },
        KVM_REG_ARM64_SYSREG => set_one_sysreg(vcpu, id, value),
        _ => Err(()),
    }
}

/// Map a decoded sysreg ID to a readable value. Returns Err for unknown regs.
fn get_one_sysreg(vcpu: &VcpuRef, id: u64) -> Result<u64, ()> {
    match id {
        // Timer registers — stored in per-vCPU sysreg state
        SYSREG_CNTV_CTL_EL0 => Ok(with_sysreg_state(vcpu, |s| s.cntv_ctl_el0)),
        SYSREG_CNTV_CVAL_EL0 => Ok(with_sysreg_state(vcpu, |s| s.cntv_cval_el0)),
        SYSREG_CNTV_TVAL_EL0 => {
            // Read CVAL and compute TVAL relative to current counter
            let cval = with_sysreg_state(vcpu, |s| s.cntv_cval_el0);
            // Return 0 as placeholder — guest can re-compute
            let _ = cval;
            Ok(0)
        }
        SYSREG_CNTVOFF_EL2 => Ok(with_sysreg_state(vcpu, |s| s.cntvoff_el2)),
        // EL1 system registers — accessible via vcpu get_reg for known indices
        SYSREG_MPIDR_EL1 => {
            // Default MPIDR for single CPU: Aff0=0, Aff1=0, Aff2=0, MT=0, RES1=bit31
            Ok(0x80000000)
        }
        SYSREG_SCTLR_EL1 => Ok(vcpu.get_reg(34).unwrap_or(0)), // Use extended reg space
        SYSREG_VBAR_EL1 => Ok(vcpu.get_reg(35).unwrap_or(0)),
        SYSREG_TCR_EL1 => Ok(vcpu.get_reg(36).unwrap_or(0)),
        SYSREG_TTBR0_EL1 => Ok(vcpu.get_reg(37).unwrap_or(0)),
        SYSREG_TTBR1_EL1 => Ok(vcpu.get_reg(38).unwrap_or(0)),
        SYSREG_MAIR_EL1 => Ok(vcpu.get_reg(39).unwrap_or(0)),
        SYSREG_AMAIR_EL1 => Ok(vcpu.get_reg(40).unwrap_or(0)),
        SYSREG_ESR_EL1 => Ok(vcpu.get_reg(41).unwrap_or(0)),
        SYSREG_FAR_EL1 => Ok(vcpu.get_reg(42).unwrap_or(0)),
        SYSREG_ELR_EL1 => Ok(vcpu.get_reg(43).unwrap_or(0)),
        SYSREG_SPSR_EL1 => Ok(vcpu.get_reg(44).unwrap_or(0)),
        SYSREG_SP_EL1 => Ok(vcpu.get_reg(45).unwrap_or(0)),
        SYSREG_CPACR_EL1 => Ok(vcpu.get_reg(46).unwrap_or(0)),
        SYSREG_CONTEXTIDR_EL1 => Ok(vcpu.get_reg(47).unwrap_or(0)),
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
fn set_one_sysreg(vcpu: &VcpuRef, id: u64, value: u64) -> Result<(), ()> {
    match id {
        // Timer registers
        SYSREG_CNTV_CTL_EL0 => {
            with_sysreg_state(vcpu, |s| s.cntv_ctl_el0 = value);
            Ok(())
        }
        SYSREG_CNTV_CVAL_EL0 => {
            with_sysreg_state(vcpu, |s| s.cntv_cval_el0 = value);
            Ok(())
        }
        SYSREG_CNTV_TVAL_EL0 => {
            // TVAL writes are relative; store as CVAL would need current counter.
            // For now, accept and store 0 to CVAL.
            let _ = value;
            with_sysreg_state(vcpu, |s| s.cntv_cval_el0 = 0);
            Ok(())
        }
        SYSREG_CNTVOFF_EL2 => {
            with_sysreg_state(vcpu, |s| s.cntvoff_el2 = value);
            Ok(())
        }
        // EL1 system registers
        SYSREG_SCTLR_EL1 => {
            let _ = vcpu.set_reg(34, value);
            Ok(())
        }
        SYSREG_VBAR_EL1 => {
            let _ = vcpu.set_reg(35, value);
            Ok(())
        }
        SYSREG_TCR_EL1 => {
            let _ = vcpu.set_reg(36, value);
            Ok(())
        }
        SYSREG_TTBR0_EL1 => {
            let _ = vcpu.set_reg(37, value);
            Ok(())
        }
        SYSREG_TTBR1_EL1 => {
            let _ = vcpu.set_reg(38, value);
            Ok(())
        }
        SYSREG_MAIR_EL1 => {
            let _ = vcpu.set_reg(39, value);
            Ok(())
        }
        SYSREG_AMAIR_EL1 => {
            let _ = vcpu.set_reg(40, value);
            Ok(())
        }
        SYSREG_ESR_EL1 => {
            let _ = vcpu.set_reg(41, value);
            Ok(())
        }
        SYSREG_FAR_EL1 => {
            let _ = vcpu.set_reg(42, value);
            Ok(())
        }
        SYSREG_ELR_EL1 => {
            let _ = vcpu.set_reg(43, value);
            Ok(())
        }
        SYSREG_SPSR_EL1 => {
            let _ = vcpu.set_reg(44, value);
            Ok(())
        }
        SYSREG_SP_EL1 => {
            let _ = vcpu.set_reg(45, value);
            Ok(())
        }
        SYSREG_CPACR_EL1 => {
            let _ = vcpu.set_reg(46, value);
            Ok(())
        }
        SYSREG_CONTEXTIDR_EL1 => {
            let _ = vcpu.set_reg(47, value);
            Ok(())
        }
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
pub fn handle_firmware_call_in_kernel(vcpu: &VcpuRef) -> FirmwareCallResult {
    let function_id = vcpu.get_reg(reg::X0).unwrap_or(0);

    match function_id {
        PSCI_FN_VERSION => {
            let _ = vcpu.set_reg(reg::X0, PSCI_VERSION_1_1);
            FirmwareCallResult::Handled
        }
        PSCI_FN_CPU_OFF | PSCI_FN64_CPU_OFF => {
            // Single vCPU: refuse CPU_OFF
            let _ = vcpu.set_reg(reg::X0, PSCI_RET_DENIED);
            FirmwareCallResult::Handled
        }
        PSCI_FN_CPU_ON | PSCI_FN64_CPU_ON => {
            // Multi-vCPU not yet supported
            let _ = vcpu.set_reg(reg::X0, PSCI_RET_NOT_SUPPORTED);
            FirmwareCallResult::Handled
        }
        PSCI_FN_SYSTEM_OFF | PSCI_FN64_SYSTEM_OFF => FirmwareCallResult::SystemOff,
        PSCI_FN_SYSTEM_RESET | PSCI_FN64_SYSTEM_RESET => FirmwareCallResult::SystemReset,
        _ => {
            // Unknown PSCI / SMCCC function — return NOT_SUPPORTED
            let _ = vcpu.set_reg(reg::X0, SMCCC_RET_NOT_SUPPORTED);
            FirmwareCallResult::Handled
        }
    }
}

// ---------------------------------------------------------------------------
// Arch hook: write firmware exit into kvm_run
// ---------------------------------------------------------------------------

/// Write architecture-specific exit data for firmware calls.
/// On AArch64, PSCI SYSTEM_OFF/RESET maps to KVM_EXIT_SYSTEM_EVENT.
pub fn write_firmware_exit(kvm_run: &mut KvmRun, exit: &VmExit, vcpu: &VcpuRef) {
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
    const KVM_CAP_ARM_PSCI: usize = 182;
    const KVM_CAP_ARM_SET_DEVICE_ADDR: usize = 177;
    const KVM_CAP_ARM_PSCI_0_2: usize = 102;

    match cap {
        KVM_CAP_ARM_VM_IPA_SIZE => Some(40),
        KVM_CAP_ARM_PSCI => Some(1),
        KVM_CAP_ARM_SET_DEVICE_ADDR => Some(1),
        KVM_CAP_ARM_PSCI_0_2 => Some(1),
        _ => None,
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
const KVM_ARM_SET_DEVICE_ADDR: u32 = io_write(KVMIO, 0xB0, 16);
const KVM_ARM_VCPU_FINALIZE: u32 = io_none(KVMIO, 0x85);

const KVM_ARM_TARGET_GENERIC_V8: u32 = 5;

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
    vcpu: VcpuRef,
    powered_off: bool,
}

static KVM_ARM_VCPU_INIT_STATES: Once<RwLock<Vec<KvmArmVcpuInitEntry>>> = Once::new();

fn get_vcpu_init_states() -> &'static RwLock<Vec<KvmArmVcpuInitEntry>> {
    KVM_ARM_VCPU_INIT_STATES.call_once(|| RwLock::new(Vec::new()))
}

fn set_vcpu_powered_off(vcpu: &VcpuRef, powered_off: bool) {
    let mut states = get_vcpu_init_states().write();
    if let Some(entry) = states.iter_mut().find(|e| Arc::ptr_eq(&e.vcpu, vcpu)) {
        entry.powered_off = powered_off;
    } else {
        states.push(KvmArmVcpuInitEntry {
            vcpu: Arc::clone(vcpu),
            powered_off,
        });
    }
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
            let _dev_addr = unsafe { &*(kva as *const KvmArmDeviceAddr) };
            Ok(Some(0))
        }

        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Arch hook: ARM-specific vCPU-level ioctl dispatch
// ---------------------------------------------------------------------------

pub fn handle_vcpu_ioctl(request: u32, arg: usize, vcpu: &VcpuRef) -> Result<Option<usize>, ()> {
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
