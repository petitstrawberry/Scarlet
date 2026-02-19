use super::{Firmware, FirmwareAction};
use scarlet_std::hypervisor::{Vcpu, reg};

pub mod extension {
    pub const BASE: u64 = 0x10;
    pub const SET_TIMER: u64 = 0x54494D45;
    pub const CONSOLE_PUTCHAR: u64 = 0x01;
    pub const CONSOLE_GETCHAR: u64 = 0x02;
    pub const SHUTDOWN: u64 = 0x53525354;
}

mod error {
    pub const SUCCESS: i64 = 0;
    pub const FAILED: i64 = -1;
    pub const NOT_SUPPORTED: i64 = -2;
}

pub struct SbiFirmware;

impl SbiFirmware {
    pub fn new() -> Self {
        Self
    }
}

impl Firmware for SbiFirmware {
    fn handle(&mut self, vcpu: &mut Vcpu) -> FirmwareAction {
        let extension = vcpu.get_reg(reg::A7).unwrap_or(0);
        let function = vcpu.get_reg(reg::A6).unwrap_or(0);
        let a0 = vcpu.get_reg(reg::A0).unwrap_or(0);

        let ((error, value), action) = match extension {
            extension::BASE => (self.handle_base(function), FirmwareAction::Continue),
            extension::CONSOLE_PUTCHAR => {
                (self.handle_console_putchar(a0), FirmwareAction::Continue)
            }
            extension::CONSOLE_GETCHAR => ((error::FAILED, 0), FirmwareAction::Continue),
            extension::SET_TIMER => ((error::SUCCESS, 0), FirmwareAction::Continue),
            extension::SHUTDOWN => ((error::SUCCESS, 0), FirmwareAction::Shutdown),
            _ => ((error::NOT_SUPPORTED, 0), FirmwareAction::Continue),
        };

        let _ = vcpu.set_reg(reg::A0, error as u64);
        let _ = vcpu.set_reg(reg::A1, value);
        action
    }
}

impl SbiFirmware {
    fn handle_base(&mut self, function: u64) -> (i64, u64) {
        match function {
            0 => (error::SUCCESS, 3),
            1 => (error::SUCCESS, 0),
            2 => (error::SUCCESS, 0),
            _ => (error::NOT_SUPPORTED, 0),
        }
    }

    fn handle_console_putchar(&mut self, arg0: u64) -> (i64, u64) {
        let ch = arg0 as u8 as char;
        scarlet_std::print!("{}", ch);
        (error::SUCCESS, 0)
    }
}

impl Default for SbiFirmware {
    fn default() -> Self {
        Self::new()
    }
}
