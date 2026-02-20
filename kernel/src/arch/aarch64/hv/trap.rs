use super::vm::Vm;
use crate::arch::Trapframe;
use crate::hypervisor::types::VmExit;

pub fn is_from_guest() -> bool {
    false
}

pub fn arch_guest_trap_handler(_trapframe: &mut Trapframe, _vm: &Vm) -> Option<VmExit> {
    Some(VmExit::Unknown(0))
}

pub fn clear_guest_mode() {
    todo!("clear_guest_mode not implemented for aarch64")
}
