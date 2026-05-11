//! AArch64 KVM register conversion

use crate::arch::hv::reg_index::reg;
use crate::hypervisor::VcpuRef;

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
        KVM_REG_ARM64_SYSREG => {
            let _op0 = (id >> KVM_REG_ARM64_SYSREG_OP0_SHIFT) & KVM_REG_ARM64_SYSREG_OP0_MASK;
            let _op1 = (id >> KVM_REG_ARM64_SYSREG_OP1_SHIFT) & KVM_REG_ARM64_SYSREG_OP1_MASK;
            let _crn = (id >> KVM_REG_ARM64_SYSREG_CRN_SHIFT) & KVM_REG_ARM64_SYSREG_CRN_MASK;
            let _crm = (id >> KVM_REG_ARM64_SYSREG_CRM_SHIFT) & KVM_REG_ARM64_SYSREG_CRM_MASK;
            let _op2 = (id >> KVM_REG_ARM64_SYSREG_OP2_SHIFT) & KVM_REG_ARM64_SYSREG_OP2_MASK;
            Ok(0)
        }
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
        KVM_REG_ARM64_SYSREG => {
            let _op0 = (id >> KVM_REG_ARM64_SYSREG_OP0_SHIFT) & KVM_REG_ARM64_SYSREG_OP0_MASK;
            let _op1 = (id >> KVM_REG_ARM64_SYSREG_OP1_SHIFT) & KVM_REG_ARM64_SYSREG_OP1_MASK;
            let _crn = (id >> KVM_REG_ARM64_SYSREG_CRN_SHIFT) & KVM_REG_ARM64_SYSREG_CRN_MASK;
            let _crm = (id >> KVM_REG_ARM64_SYSREG_CRM_SHIFT) & KVM_REG_ARM64_SYSREG_CRM_MASK;
            let _op2 = (id >> KVM_REG_ARM64_SYSREG_OP2_SHIFT) & KVM_REG_ARM64_SYSREG_OP2_MASK;
            Ok(())
        }
        _ => Err(()),
    }
}
