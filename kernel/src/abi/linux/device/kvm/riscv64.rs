//! RISC-V KVM register conversion
//!
//! Maps between the Scarlet hypervisor's GuestRegisters (index-based) and
//! Linux KVM's `struct kvm_regs` in RISC-V ptrace order.

use crate::hypervisor::VcpuRef;

/// `struct kvm_regs` for RISC-V in Linux ptrace order
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

/// RISC-V register index → KvmRegs field mapping (ptrace order).
///
/// Index 0 (x0) is hardwired zero and excluded from the mapping.
const PTRACE_GPR_ORDER: [usize; 31] = [
    1,  // ra
    2,  // sp
    3,  // gp
    4,  // tp
    5,  // t0
    6,  // t1
    7,  // t2
    8,  // s0/fp
    9,  // s1
    10, // a0
    11, // a1
    12, // a2
    13, // a3
    14, // a4
    15, // a5
    16, // a6
    17, // a7
    18, // s2
    19, // s3
    20, // s4
    21, // s5
    22, // s6
    23, // s7
    24, // s8
    25, // s9
    26, // s10
    27, // s11
    28, // t3
    29, // t4
    30, // t5
    31, // t6
];

/// Read vCPU registers into a Linux KvmRegs struct.
pub fn read_regs_to_kvm(vcpu: &VcpuRef) -> KvmRegs {
    let mut buf = [0u64; 32];
    buf[0] = vcpu.get_pc();
    for (slot, &reg_idx) in PTRACE_GPR_ORDER.iter().enumerate() {
        buf[slot + 1] = vcpu.get_gpr(reg_idx);
    }

    // SAFETY: KvmRegs is repr(C) with 32 consecutive u64 fields,
    // identical in layout to [u64; 32].
    unsafe { core::mem::transmute(buf) }
}

/// Write a Linux KvmRegs struct into the vCPU registers.
pub fn write_kvm_to_regs(vcpu: &VcpuRef, kvm_regs: &KvmRegs) {
    // SAFETY: same layout reasoning as read_regs_to_kvm.
    let buf: &[u64; 32] = unsafe { core::mem::transmute(kvm_regs) };

    vcpu.set_pc(buf[0]);
    for (slot, &reg_idx) in PTRACE_GPR_ORDER.iter().enumerate() {
        vcpu.set_gpr(reg_idx, buf[slot + 1]);
    }
}
