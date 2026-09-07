use scarlet_std::hypervisor::Vcpu;
use scarlet_std::println;

use super::{Firmware, FirmwareAction};

pub struct PsciFirmware;

impl PsciFirmware {
    pub fn new() -> Self {
        Self
    }
}

const PSCI_VERSION: u64 = 0x8400_0000;
const PSCI_CPU_OFF: u64 = 0x8400_0002;
const PSCI_CPU_ON: u64 = 0xC400_0003;
const PSCI_SYSTEM_OFF: u64 = 0x8400_0008;
const PSCI_SYSTEM_RESET: u64 = 0x8400_0009;

impl Firmware for PsciFirmware {
    fn handle(&mut self, vcpu: &Vcpu) -> FirmwareAction {
        use scarlet_std::hypervisor::arch::reg;

        let func_id = vcpu.get_reg(reg::X0).unwrap_or(0);

        match func_id {
            PSCI_VERSION => {
                let _ = vcpu.set_reg(reg::X0, 0x0001_0000);
                FirmwareAction::Continue
            }
            PSCI_CPU_OFF => {
                let _ = vcpu.set_reg(reg::X0, 0);
                FirmwareAction::Continue
            }
            PSCI_CPU_ON => {
                let _ = vcpu.set_reg(reg::X0, 3);
                FirmwareAction::Continue
            }
            PSCI_SYSTEM_OFF => {
                println!("[ushv] PSCI_SYSTEM_OFF");
                FirmwareAction::Shutdown
            }
            PSCI_SYSTEM_RESET => {
                println!("[ushv] PSCI_SYSTEM_RESET");
                FirmwareAction::Shutdown
            }
            _ => {
                println!("[ushv] Unknown PSCI call: {:#x}", func_id);
                let _ = vcpu.set_reg(reg::X0, u64::MAX);
                FirmwareAction::Continue
            }
        }
    }
}

impl Default for PsciFirmware {
    fn default() -> Self {
        Self::new()
    }
}
