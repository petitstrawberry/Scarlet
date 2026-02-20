#[cfg(target_arch = "riscv64")]
pub mod sbi;

use scarlet_std::hypervisor::Vcpu;

#[derive(PartialEq)]
pub enum FirmwareAction {
    Continue,
    Shutdown,
}

pub trait Firmware {
    fn handle(&mut self, vcpu: &mut Vcpu) -> FirmwareAction;
}
