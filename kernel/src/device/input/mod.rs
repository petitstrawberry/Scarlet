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
    /// Mouse wheel
    pub const REL_WHEEL: u16 = 0x08;
}

/// Key/button codes (selected common ones)
///
/// For a complete list, refer to Linux's input-event-codes.h
pub mod key_codes {
    // Mouse buttons
    pub const BTN_LEFT: u16 = 0x110;
    pub const BTN_RIGHT: u16 = 0x111;
    pub const BTN_MIDDLE: u16 = 0x112;

    // Keyboard keys (examples)
    pub const KEY_ESC: u16 = 1;
    pub const KEY_1: u16 = 2;
    pub const KEY_2: u16 = 3;
    pub const KEY_A: u16 = 30;
    pub const KEY_B: u16 = 48;
    pub const KEY_SPACE: u16 = 57;
    pub const KEY_ENTER: u16 = 28;
    pub const KEY_LEFTSHIFT: u16 = 42;
    pub const KEY_RIGHTSHIFT: u16 = 54;
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
