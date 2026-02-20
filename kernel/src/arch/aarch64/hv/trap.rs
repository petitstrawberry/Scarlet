use crate::arch::Trapframe;
use crate::hypervisor::types::VmExit;
use crate::hypervisor::vm::VmObject;

pub fn is_from_guest() -> bool {
    false
}

pub fn arch_guest_trap_handler(_trapframe: &mut Trapframe, _vm: &VmObject) -> Option<VmExit> {
    Some(VmExit::Unknown(0))
}

pub fn clear_guest_mode() {
    todo!("clear_guest_mode not implemented for aarch64")
}
