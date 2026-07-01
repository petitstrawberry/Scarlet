//! Input device module
//!
//! This module provides input event handling for devices like keyboards, mice,
//! touchscreens, etc. It follows the Linux input event model conceptually but
//! uses Rust-friendly types (u64 timestamps instead of timeval).

use core::mem::size_of;

pub mod event_device;

/// Input event structure
///
/// Conceptually similar to Linux's input_event, but with a Rust-friendly
/// u64 timestamp (nanoseconds since boot) instead of timeval.
/// The type/code/value model is preserved for compatibility.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct InputEvent {
    /// Timestamp in nanoseconds since boot
    pub time: u64,
    /// Event type (EV_KEY, EV_REL, etc.)
    pub type_: u16,
    /// Event code (KEY_A, REL_X, etc.)
    pub code: u16,
    /// Event value (1/0 for keys, movement delta for relative axes, etc.)
    pub value: i32,
}

impl InputEvent {
    /// Get the size of InputEvent structure for read operations
    pub const fn size() -> usize {
        size_of::<Self>()
    }

    /// Create a new input event with the current timestamp
    pub fn new(type_: u16, code: u16, value: i32) -> Self {
        Self {
            time: crate::time::current_time_ns(),
            type_,
            code,
            value,
        }
    }
}

/// Event type constants
///
/// These values match Linux's input event types for compatibility.
pub mod event_types {
    /// Synchronization events
    pub const EV_SYN: u16 = 0x00;
    /// Key/button press and release events
    pub const EV_KEY: u16 = 0x01;
    /// Relative axis movement (e.g., mouse movement)
    pub const EV_REL: u16 = 0x02;
    /// Absolute axis position (e.g., touchscreen)
    pub const EV_ABS: u16 = 0x03;
    /// Miscellaneous events
    pub const EV_MSC: u16 = 0x04;
    /// LED state changes
    pub const EV_LED: u16 = 0x11;
    /// Sound events
    pub const EV_SND: u16 = 0x12;
}

/// Synchronization event codes
pub mod syn_codes {
    /// Synchronization marker - separates event packets
    pub const SYN_REPORT: u16 = 0;
}

/// Relative axis codes
pub mod rel_codes {
    /// Relative X axis (horizontal movement)
    pub const REL_X: u16 = 0x00;
    /// Relative Y axis (vertical movement)
    pub const REL_Y: u16 = 0x01;
    /// Relative Z axis
    pub const REL_Z: u16 = 0x02;
    /// Horizontal mouse wheel
    pub const REL_HWHEEL: u16 = 0x06;
    /// Mouse wheel
    pub const REL_WHEEL: u16 = 0x08;
    /// High-resolution vertical mouse wheel (value in 1/120 units)
    pub const REL_WHEEL_HI_RES: u16 = 0x0b;
    /// High-resolution horizontal mouse wheel (value in 1/120 units)
    pub const REL_HWHEEL_HI_RES: u16 = 0x0c;
}

/// Key/button codes (selected common ones)
///
/// For a complete list, refer to Linux's input-event-codes.h
pub mod key_codes {
    // Mouse buttons
    pub const BTN_LEFT: u16 = 0x110;
    pub const BTN_RIGHT: u16 = 0x111;
    pub const BTN_MIDDLE: u16 = 0x112;

    // Keyboard keys
    pub const KEY_ESC: u16 = 1;
    pub const KEY_1: u16 = 2;
    pub const KEY_2: u16 = 3;
    pub const KEY_3: u16 = 4;
    pub const KEY_4: u16 = 5;
    pub const KEY_5: u16 = 6;
    pub const KEY_6: u16 = 7;
    pub const KEY_7: u16 = 8;
    pub const KEY_8: u16 = 9;
    pub const KEY_9: u16 = 10;
    pub const KEY_0: u16 = 11;
    pub const KEY_MINUS: u16 = 12;
    pub const KEY_EQUAL: u16 = 13;
    pub const KEY_BACKSPACE: u16 = 14;
    pub const KEY_TAB: u16 = 15;
    pub const KEY_Q: u16 = 16;
    pub const KEY_W: u16 = 17;
    pub const KEY_E: u16 = 18;
    pub const KEY_R: u16 = 19;
    pub const KEY_T: u16 = 20;
    pub const KEY_Y: u16 = 21;
    pub const KEY_U: u16 = 22;
    pub const KEY_I: u16 = 23;
    pub const KEY_O: u16 = 24;
    pub const KEY_P: u16 = 25;
    pub const KEY_LEFTBRACE: u16 = 26;
    pub const KEY_RIGHTBRACE: u16 = 27;
    pub const KEY_ENTER: u16 = 28;
    pub const KEY_LEFTCTRL: u16 = 29;
    pub const KEY_A: u16 = 30;
    pub const KEY_S: u16 = 31;
    pub const KEY_D: u16 = 32;
    pub const KEY_F: u16 = 33;
    pub const KEY_G: u16 = 34;
    pub const KEY_H: u16 = 35;
    pub const KEY_J: u16 = 36;
    pub const KEY_K: u16 = 37;
    pub const KEY_L: u16 = 38;
    pub const KEY_SEMICOLON: u16 = 39;
    pub const KEY_APOSTROPHE: u16 = 40;
    pub const KEY_GRAVE: u16 = 41;
    pub const KEY_LEFTSHIFT: u16 = 42;
    pub const KEY_BACKSLASH: u16 = 43;
    pub const KEY_Z: u16 = 44;
    pub const KEY_X: u16 = 45;
    pub const KEY_C: u16 = 46;
    pub const KEY_V: u16 = 47;
    pub const KEY_B: u16 = 48;
    pub const KEY_N: u16 = 49;
    pub const KEY_M: u16 = 50;
    pub const KEY_COMMA: u16 = 51;
    pub const KEY_DOT: u16 = 52;
    pub const KEY_SLASH: u16 = 53;
    pub const KEY_RIGHTSHIFT: u16 = 54;
    pub const KEY_SPACE: u16 = 57;
    pub const KEY_CAPSLOCK: u16 = 58;
    pub const KEY_F1: u16 = 59;
    pub const KEY_F2: u16 = 60;
    pub const KEY_F3: u16 = 61;
    pub const KEY_F4: u16 = 62;
    pub const KEY_F5: u16 = 63;
    pub const KEY_F6: u16 = 64;
    pub const KEY_F7: u16 = 65;
    pub const KEY_F8: u16 = 66;
    pub const KEY_F9: u16 = 67;
    pub const KEY_F10: u16 = 68;
    pub const KEY_F11: u16 = 87;
    pub const KEY_F12: u16 = 88;
    pub const KEY_RIGHTCTRL: u16 = 97;
    pub const KEY_RIGHTALT: u16 = 100;
    pub const KEY_RIGHTMETA: u16 = 126;
    pub const KEY_LEFTALT: u16 = 56;
    pub const KEY_LEFTMETA: u16 = 125;
}

/// Key/button state values
pub mod key_values {
    /// Key released
    pub const KEY_RELEASE: i32 = 0;
    /// Key pressed
    pub const KEY_PRESS: i32 = 1;
    /// Key held (auto-repeat)
    pub const KEY_REPEAT: i32 = 2;
}
