use crate::arch::{Arch, Trapframe};

use super::guest_vcpu::GuestVcpu;
use super::sysreg::{GuestSystemRegs, HypervisorSystemRegs};

pub struct VcpuSwitchData {
    guest_sysregs: GuestSystemRegs,
}

pub struct HypervisorSwitchData {
    hypervisor_sysregs: HypervisorSystemRegs,
}

impl HypervisorSwitchData {
    pub fn save() -> Self {
        HypervisorSwitchData {
            hypervisor_sysregs: HypervisorSystemRegs::save(),
        }
    }

    pub fn restore(&self) {
        self.hypervisor_sysregs.restore();
    }
}

impl VcpuSwitchData {
    pub fn save() -> Self {
        VcpuSwitchData {
            guest_sysregs: GuestSystemRegs::save(),
        }
    }

    pub fn restore(&self) {
        self.guest_sysregs.restore();
    }
}

pub unsafe extern "C" fn arch_run_guest_loop(
    _trapframe: *const Trapframe,
    _vcpu: *const GuestVcpu,
    _arch: *const Arch,
) {
    todo!("arch_run_guest_loop not implemented for aarch64")
}

pub extern "C" fn arch_guest_trap_exit() {
    todo!("arch_guest_trap_exit not implemented for aarch64")
}
