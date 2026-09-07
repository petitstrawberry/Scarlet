pub mod psci;

use scarlet_std::hypervisor::Vcpu;

#[derive(PartialEq)]
pub enum FirmwareAction {
    Continue,
    Shutdown,
}

pub trait Firmware {
    fn handle(&mut self, vcpu: &Vcpu) -> FirmwareAction;
}

pub use psci::PsciFirmware;
