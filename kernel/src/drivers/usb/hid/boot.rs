extern crate alloc;

use alloc::sync::Arc;
use core::mem::size_of;

use crate::device::input::event_device::EventDevice;
use crate::device::input::event_types::{EV_KEY, EV_REL, EV_SYN};
use crate::device::input::key_codes::{
    BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, KEY_A, KEY_ENTER, KEY_ESC, KEY_LEFTSHIFT, KEY_RIGHTSHIFT,
    KEY_SPACE,
};
use crate::device::input::key_values::{KEY_PRESS, KEY_RELEASE};
use crate::device::input::rel_codes::{REL_X, REL_Y};
use crate::device::input::syn_codes::SYN_REPORT;
use crate::early_println;

/// USB HID boot keyboard report layout.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyboardBootReport {
    pub modifiers: u8,
    pub reserved: u8,
    pub keys: [u8; 6],
}

/// USB HID boot mouse report layout.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MouseBootReport {
    pub buttons: u8,
    pub x: i8,
    pub y: i8,
}

pub struct HidKeyboardDevice {
    event_device: Arc<EventDevice>,
    last_report: KeyboardBootReport,
}

pub struct HidMouseDevice {
    event_device: Arc<EventDevice>,
    last_report: MouseBootReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidBootProtocol {
    Keyboard,
    Mouse,
}

impl KeyboardBootReport {
    /// Returns the encoded report size in bytes.
    pub const fn encoded_size() -> usize {
        size_of::<Self>()
    }
}

impl MouseBootReport {
    /// Returns the encoded report size in bytes.
    pub const fn encoded_size() -> usize {
        size_of::<Self>()
    }
}

impl HidKeyboardDevice {
    pub fn new() -> Self {
        Self {
            event_device: Arc::new(EventDevice::new("keyboard")),
            last_report: KeyboardBootReport::default(),
        }
    }

    pub fn event_device(&self) -> Arc<EventDevice> {
        self.event_device.clone()
    }

    pub fn handle_report(&mut self, report: KeyboardBootReport) {
        if report != self.last_report {
            early_println!(
                "[usb-hid] keyboard report modifiers={:#x} keys={:02x?}",
                report.modifiers,
                report.keys
            );
        }

        for modifier in [(0x02, KEY_LEFTSHIFT), (0x20, KEY_RIGHTSHIFT)] {
            let was_pressed = (self.last_report.modifiers & modifier.0) != 0;
            let is_pressed = (report.modifiers & modifier.0) != 0;
            if was_pressed != is_pressed {
                self.event_device.push_event(
                    EV_KEY,
                    modifier.1,
                    if is_pressed { KEY_PRESS } else { KEY_RELEASE },
                );
            }
        }

        for old_key in self.last_report.keys {
            if old_key != 0 && !report.keys.contains(&old_key) {
                if let Some(code) = translate_boot_key(old_key) {
                    self.event_device.push_event(EV_KEY, code, KEY_RELEASE);
                }
            }
        }

        for new_key in report.keys {
            if new_key != 0 && !self.last_report.keys.contains(&new_key) {
                if let Some(code) = translate_boot_key(new_key) {
                    self.event_device.push_event(EV_KEY, code, KEY_PRESS);
                }
            }
        }

        self.event_device.push_event(EV_SYN, SYN_REPORT, 0);
        self.last_report = report;
    }
}

impl Default for HidKeyboardDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl HidMouseDevice {
    pub fn new() -> Self {
        Self {
            event_device: Arc::new(EventDevice::new("mouse")),
            last_report: MouseBootReport::default(),
        }
    }

    pub fn event_device(&self) -> Arc<EventDevice> {
        self.event_device.clone()
    }

    pub fn handle_report(&mut self, report: MouseBootReport) {
        if report != self.last_report {
            early_println!(
                "[usb-hid] mouse report buttons={:#x} dx={} dy={}",
                report.buttons,
                report.x,
                report.y
            );
        }

        for (mask, code) in [(0x01, BTN_LEFT), (0x02, BTN_RIGHT), (0x04, BTN_MIDDLE)] {
            let was_pressed = (self.last_report.buttons & mask) != 0;
            let is_pressed = (report.buttons & mask) != 0;
            if was_pressed != is_pressed {
                self.event_device.push_event(
                    EV_KEY,
                    code,
                    if is_pressed { KEY_PRESS } else { KEY_RELEASE },
                );
            }
        }

        if report.x != 0 {
            self.event_device.push_event(EV_REL, REL_X, report.x as i32);
        }
        if report.y != 0 {
            self.event_device.push_event(EV_REL, REL_Y, report.y as i32);
        }

        self.event_device.push_event(EV_SYN, SYN_REPORT, 0);
        self.last_report = report;
    }
}

impl Default for HidMouseDevice {
    fn default() -> Self {
        Self::new()
    }
}

fn translate_boot_key(boot_code: u8) -> Option<u16> {
    match boot_code {
        0x04 => Some(KEY_A),
        0x29 => Some(KEY_ESC),
        0x28 => Some(KEY_ENTER),
        0x2c => Some(KEY_SPACE),
        _ => None,
    }
}

pub const fn boot_protocol_for_interface(subclass: u8, protocol: u8) -> Option<HidBootProtocol> {
    if subclass != 1 {
        return None;
    }

    match protocol {
        1 => Some(HidBootProtocol::Keyboard),
        2 => Some(HidBootProtocol::Mouse),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_hid_boot_report_sizes() {
        assert_eq!(KeyboardBootReport::encoded_size(), 8);
        assert_eq!(MouseBootReport::encoded_size(), 3);
    }

    #[test_case]
    fn test_translate_boot_key() {
        assert_eq!(translate_boot_key(0x04), Some(KEY_A));
        assert_eq!(translate_boot_key(0x29), Some(KEY_ESC));
        assert_eq!(translate_boot_key(0xff), None);
    }

    #[test_case]
    fn test_boot_protocol_for_interface() {
        assert_eq!(
            boot_protocol_for_interface(1, 1),
            Some(HidBootProtocol::Keyboard)
        );
        assert_eq!(
            boot_protocol_for_interface(1, 2),
            Some(HidBootProtocol::Mouse)
        );
        assert_eq!(boot_protocol_for_interface(0, 1), None);
    }
}
