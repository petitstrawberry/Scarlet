use crate::arch::Trapframe;
use crate::hypervisor::types::VmExit;

use super::guest_vcpu::GuestVcpu;

pub fn is_from_guest() -> bool {
    false
}

pub fn arch_guest_trap_handler(_vcpu: &mut GuestVcpu, _trapframe: &mut Trapframe) -> VmExit {
    VmExit::Unknown(0)
}

pub fn clear_guest_mode() {
    todo!("clear_guest_mode not implemented for aarch64")
}
