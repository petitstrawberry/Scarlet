//! AArch64 KVM register conversion (stub)
//!
//! AArch64 hypervisor support is not yet implemented.

use crate::hypervisor::VcpuRef;

/// `struct kvm_regs` for AArch64 (placeholder)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmRegs {
    pub regs: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub pstate: u64,
}

pub fn read_regs_to_kvm(_vcpu: &VcpuRef) -> KvmRegs {
    KvmRegs {
        regs: [0; 31],
        sp: 0,
        pc: 0,
        pstate: 0,
    }
}

pub fn write_kvm_to_regs(_vcpu: &VcpuRef, _kvm_regs: &KvmRegs) {
    // AArch64 hypervisor not yet supported
}
