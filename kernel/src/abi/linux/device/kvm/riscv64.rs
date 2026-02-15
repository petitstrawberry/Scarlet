//! RISC-V KVM register conversion

use crate::arch::hv::reg_index::reg;
use crate::hypervisor::VcpuRef;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct KvmRegs {
    pub pc: u64,
    pub ra: u64,
    pub sp: u64,
    pub gp: u64,
    pub tp: u64,
    pub t0: u64,
    pub t1: u64,
    pub t2: u64,
    pub s0: u64,
    pub s1: u64,
    pub a0: u64,
    pub a1: u64,
    pub a2: u64,
    pub a3: u64,
    pub a4: u64,
    pub a5: u64,
    pub a6: u64,
    pub a7: u64,
    pub s2: u64,
    pub s3: u64,
    pub s4: u64,
    pub s5: u64,
    pub s6: u64,
    pub s7: u64,
    pub s8: u64,
    pub s9: u64,
    pub s10: u64,
    pub s11: u64,
    pub t3: u64,
    pub t4: u64,
    pub t5: u64,
    pub t6: u64,
}

const PTRACE_REG_INDEX: [u32; 32] = [
    reg::PC,
    reg::RA,
    reg::SP,
    reg::GP,
    reg::TP,
    reg::T0,
    reg::T1,
    reg::T2,
    reg::S0,
    reg::S1,
    reg::A0,
    reg::A1,
    reg::A2,
    reg::A3,
    reg::A4,
    reg::A5,
    reg::A6,
    reg::A7,
    reg::S2,
    reg::S3,
    reg::S4,
    reg::S5,
    reg::S6,
    reg::S7,
    reg::S8,
    reg::S9,
    reg::S10,
    reg::S11,
    reg::T3,
    reg::T4,
    reg::T5,
    reg::T6,
];

pub fn read_regs_to_kvm(vcpu: &VcpuRef) -> KvmRegs {
    let mut buf = [0u64; 32];
    for (slot, &idx) in PTRACE_REG_INDEX.iter().enumerate() {
        buf[slot] = vcpu.get_reg(idx).unwrap_or(0);
    }
    unsafe { core::mem::transmute(buf) }
}

pub fn write_kvm_to_regs(vcpu: &VcpuRef, kvm_regs: &KvmRegs) {
    let buf: &[u64; 32] = unsafe { core::mem::transmute(kvm_regs) };
    for (&idx, &val) in PTRACE_REG_INDEX.iter().zip(buf.iter()) {
        let _ = vcpu.set_reg(idx, val);
    }
}
