extern crate alloc;

use alloc::sync::Arc;
use core::mem::size_of;

use crate::device::input::event_device::EventDevice;
use crate::device::input::event_types::{EV_KEY, EV_REL, EV_SYN};
use crate::device::input::key_codes;
use crate::device::input::key_values::{KEY_PRESS, KEY_RELEASE};
use crate::device::input::rel_codes::{REL_X, REL_Y};
use crate::device::input::syn_codes::SYN_REPORT;

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
        for modifier in [
            (0x02, key_codes::KEY_LEFTSHIFT),
            (0x20, key_codes::KEY_RIGHTSHIFT),
        ] {
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
        for (mask, code) in [
            (0x01, key_codes::BTN_LEFT),
            (0x02, key_codes::BTN_RIGHT),
            (0x04, key_codes::BTN_MIDDLE),
        ] {
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
        0x04 => Some(key_codes::KEY_A),
        0x05 => Some(key_codes::KEY_B),
        0x06 => Some(key_codes::KEY_C),
        0x07 => Some(key_codes::KEY_D),
        0x08 => Some(key_codes::KEY_E),
        0x09 => Some(key_codes::KEY_F),
        0x0a => Some(key_codes::KEY_G),
        0x0b => Some(key_codes::KEY_H),
        0x0c => Some(key_codes::KEY_I),
        0x0d => Some(key_codes::KEY_J),
        0x0e => Some(key_codes::KEY_K),
        0x0f => Some(key_codes::KEY_L),
        0x10 => Some(key_codes::KEY_M),
        0x11 => Some(key_codes::KEY_N),
        0x12 => Some(key_codes::KEY_O),
        0x13 => Some(key_codes::KEY_P),
        0x14 => Some(key_codes::KEY_Q),
        0x15 => Some(key_codes::KEY_R),
        0x16 => Some(key_codes::KEY_S),
        0x17 => Some(key_codes::KEY_T),
        0x18 => Some(key_codes::KEY_U),
        0x19 => Some(key_codes::KEY_V),
        0x1a => Some(key_codes::KEY_W),
        0x1b => Some(key_codes::KEY_X),
        0x1c => Some(key_codes::KEY_Y),
        0x1d => Some(key_codes::KEY_Z),
        0x1e => Some(key_codes::KEY_1),
        0x1f => Some(key_codes::KEY_2),
        0x20 => Some(key_codes::KEY_3),
        0x21 => Some(key_codes::KEY_4),
        0x22 => Some(key_codes::KEY_5),
        0x23 => Some(key_codes::KEY_6),
        0x24 => Some(key_codes::KEY_7),
        0x25 => Some(key_codes::KEY_8),
        0x26 => Some(key_codes::KEY_9),
        0x27 => Some(key_codes::KEY_0),
        0x28 => Some(key_codes::KEY_ENTER),
        0x29 => Some(key_codes::KEY_ESC),
        0x2a => Some(key_codes::KEY_BACKSPACE),
        0x2b => Some(key_codes::KEY_TAB),
        0x2c => Some(key_codes::KEY_SPACE),
        0x2d => Some(key_codes::KEY_MINUS),
        0x2e => Some(key_codes::KEY_EQUAL),
        0x2f => Some(key_codes::KEY_LEFTBRACE),
        0x30 => Some(key_codes::KEY_RIGHTBRACE),
        0x31 => Some(key_codes::KEY_BACKSLASH),
        0x33 => Some(key_codes::KEY_SEMICOLON),
        0x34 => Some(key_codes::KEY_APOSTROPHE),
        0x35 => Some(key_codes::KEY_GRAVE),
        0x36 => Some(key_codes::KEY_COMMA),
        0x37 => Some(key_codes::KEY_DOT),
        0x38 => Some(key_codes::KEY_SLASH),
        0x39 => Some(key_codes::KEY_CAPSLOCK),
        0x3a => Some(key_codes::KEY_F1),
        0x3b => Some(key_codes::KEY_F2),
        0x3c => Some(key_codes::KEY_F3),
        0x3d => Some(key_codes::KEY_F4),
        0x3e => Some(key_codes::KEY_F5),
        0x3f => Some(key_codes::KEY_F6),
        0x40 => Some(key_codes::KEY_F7),
        0x41 => Some(key_codes::KEY_F8),
        0x42 => Some(key_codes::KEY_F9),
        0x43 => Some(key_codes::KEY_F10),
        0x44 => Some(key_codes::KEY_F11),
        0x45 => Some(key_codes::KEY_F12),
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
        assert_eq!(translate_boot_key(0x04), Some(key_codes::KEY_A));
        assert_eq!(translate_boot_key(0x29), Some(key_codes::KEY_ESC));
        assert_eq!(translate_boot_key(0x2d), Some(key_codes::KEY_MINUS));
        assert_eq!(translate_boot_key(0x35), Some(key_codes::KEY_GRAVE));
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
