use core::arch::asm;
use core::arch::naked_asm;

use crate::arch::{Arch, Trapframe};

use super::guest_vcpu::GuestVcpu;
use super::sysreg::{GuestSystemRegs, HypervisorSystemRegs};

/// When TGE=1 (host VHE mode), _EL12 registers like ESR_EL12 and FAR_EL12
/// are UNDEF. Only save/restore guest state when actually in guest context
/// (VM bit set in HCR_EL2, meaning a guest was entered).
fn is_guest_context() -> bool {
    let hcr_el2: u64;
    // SAFETY: reading HCR_EL2 at EL2 is side-effect free.
    unsafe {
        asm!("mrs {}, HCR_EL2", out(reg) hcr_el2, options(nostack));
    }
    (hcr_el2 & HCR_EL2_VM) != 0
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct HostHvContext {
    pub(crate) hcr_el2: u64,
    pub(crate) vbar_el2: u64,
    pub(crate) ich_hcr_el2: u64,
    pub(crate) ich_vmcr_el2: u64,
}

pub(crate) const HCR_EL2_VM: u64 = 1 << 0;
pub(crate) const HCR_EL2_TWI: u64 = 1 << 8;
pub(crate) const HCR_EL2_TSC: u64 = 1 << 11;
pub(crate) const HCR_EL2_TGE: u64 = 1 << 27;
pub(crate) const HCR_EL2_RW: u64 = 1 << 31;
pub(crate) const HCR_EL2_E2H: u64 = 1 << 34;

pub(crate) const HCR_EL2_HOST: u64 = HCR_EL2_E2H | HCR_EL2_TGE;
pub(crate) const HCR_EL2_GUEST: u64 =
    HCR_EL2_E2H | HCR_EL2_VM | HCR_EL2_RW | HCR_EL2_TWI | HCR_EL2_TSC;

pub(crate) static mut HOST_HV_CTX: HostHvContext = HostHvContext {
    hcr_el2: 0,
    vbar_el2: 0,
    ich_hcr_el2: 0,
    ich_vmcr_el2: 0,
};

pub(crate) static mut GUEST_TRAPFRAME_PTR: usize = 0;

pub struct VcpuSwitchData {
    guest_sysregs: GuestSystemRegs,
}

pub struct HypervisorSwitchData {
    hypervisor_sysregs: HypervisorSystemRegs,
}

impl HypervisorSwitchData {
    pub fn save() -> Self {
        if !is_guest_context() {
            return Self::default();
        }
        HypervisorSwitchData {
            hypervisor_sysregs: HypervisorSystemRegs::save(),
        }
    }

    pub fn restore(&self) {
        if !is_guest_context() {
            return;
        }
        self.hypervisor_sysregs.restore();
    }
}

impl Default for HypervisorSwitchData {
    fn default() -> Self {
        Self {
            hypervisor_sysregs: HypervisorSystemRegs::default(),
        }
    }
}

impl VcpuSwitchData {
    pub fn save() -> Self {
        if !is_guest_context() {
            return Self::default();
        }
        VcpuSwitchData {
            guest_sysregs: GuestSystemRegs::save(),
        }
    }

    pub fn restore(&self) {
        if !is_guest_context() {
            return;
        }
        self.guest_sysregs.restore();
    }
}

impl Default for VcpuSwitchData {
    fn default() -> Self {
        Self {
            guest_sysregs: GuestSystemRegs::default(),
        }
    }
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn el2_guest_exit_vector() {
    naked_asm!(
        ".balign 0x800",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b 1f",
        ".balign 0x80",
        "b 1f",
        ".balign 0x80",
        "b 1f",
        ".balign 0x80",
        "b 1f",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        ".balign 0x80",
        "b .",
        "1:",
        "str x30, [sp, #-16]!",
        "adrp x30, {trapframe_ptr}",
        "ldr x30, [x30, #:lo12:{trapframe_ptr}]",
        "stp x0, x1, [x30, #0]",
        "stp x2, x3, [x30, #16]",
        "stp x4, x5, [x30, #32]",
        "stp x6, x7, [x30, #48]",
        "stp x8, x9, [x30, #64]",
        "stp x10, x11, [x30, #80]",
        "stp x12, x13, [x30, #96]",
        "stp x14, x15, [x30, #112]",
        "stp x16, x17, [x30, #128]",
        "stp x18, x19, [x30, #144]",
        "stp x20, x21, [x30, #160]",
        "stp x22, x23, [x30, #176]",
        "stp x24, x25, [x30, #192]",
        "stp x26, x27, [x30, #208]",
        "stp x28, x29, [x30, #224]",
        "ldr x0, [sp]",
        "str x0, [x30, #240]",
        "mrs x0, sp_el1",
        "str x0, [x30, #248]",
        "mrs x0, elr_el2",
        "str x0, [x30, #256]",
        "mrs x0, spsr_el2",
        "str x0, [x30, #264]",
        "mrs x0, esr_el2",
        "str x0, [x30, #288]",
        "adrp x0, {trap_exit}",
        "add x0, x0, #:lo12:{trap_exit}",
        "br x0",
        trapframe_ptr = sym GUEST_TRAPFRAME_PTR,
        trap_exit = sym arch_guest_trap_exit,
    );
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn arch_run_guest_loop(
    _trapframe: *mut Trapframe,
    _vcpu: *const GuestVcpu,
    _arch: *const Arch,
) {
    naked_asm!(
        "sub sp, sp, #96",
        "stp x19, x20, [sp, #0]",
        "stp x21, x22, [sp, #16]",
        "stp x23, x24, [sp, #32]",
        "stp x25, x26, [sp, #48]",
        "stp x27, x28, [sp, #64]",
        "stp x29, x30, [sp, #80]",
        "adrp x3, {trapframe_ptr}",
        "str x0, [x3, #:lo12:{trapframe_ptr}]",
        "mov x3, x1",
        "ldp x19, x20, [x3, #152]",
        "ldp x21, x22, [x3, #168]",
        "ldp x23, x24, [x3, #184]",
        "ldp x25, x26, [x3, #200]",
        "ldp x27, x28, [x3, #216]",
        "ldp x29, x30, [x3, #232]",
        "ldp x8, x9, [x3, #64]",
        "ldp x10, x11, [x3, #80]",
        "ldp x12, x13, [x3, #96]",
        "ldp x14, x15, [x3, #112]",
        "ldp x16, x17, [x3, #128]",
        "ldr x18, [x3, #144]",
        "ldp x4, x5, [x3, #32]",
        "ldp x6, x7, [x3, #48]",
        "ldr x2, [x3, #248]",
        "msr elr_el2, x2",
        "ldr x2, [x3, #256]",
        "msr spsr_el2, x2",
        "ldr x2, [x3, #320]",
        "msr sp_el1, x2",
        "ldp x0, x1, [x3, #0]",
        "ldp x2, x3, [x3, #16]",
        "eret",
        trapframe_ptr = sym GUEST_TRAPFRAME_PTR,
    );
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub extern "C" fn arch_guest_trap_exit() {
    naked_asm!(
        "adrp x0, {host_ctx}",
        "add x0, x0, #:lo12:{host_ctx}",
        "ldr x1, [x0, #0]",
        "ldr x2, [x0, #8]",
        "ldr x3, [x0, #16]",
        "ldr x4, [x0, #24]",
        "msr hcr_el2, x1",
        "msr vbar_el2, x2",
        "msr ich_hcr_el2, x3",
        "msr ich_vmcr_el2, x4",
        "msr vttbr_el2, xzr",
        "isb",
        "add sp, sp, #16",
        "ldp x19, x20, [sp, #0]",
        "ldp x21, x22, [sp, #16]",
        "ldp x23, x24, [sp, #32]",
        "ldp x25, x26, [sp, #48]",
        "ldp x27, x28, [sp, #64]",
        "ldp x29, x30, [sp, #80]",
        "add sp, sp, #96",
        "ret",
        host_ctx = sym HOST_HV_CTX,
    );
}
