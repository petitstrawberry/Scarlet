//! Native virtual gamepad publication.
//!
//! Open /dev/uinput for writing. CREATE takes a pointer to one GamepadSpec
//! and returns the assigned /dev/gamepadN index. Each subsequent write is
//! one complete snapshot of 16-byte native input records, including each
//! declared button and axis exactly once and a final SYN_REPORT. The kernel
//! supplies timestamps. The last close publishes neutral and removes the pad.
//! Creation and writes require physical-seat management authority.
//! Each published gamepad reader starts at the latest complete
//! snapshot with its own cursor. A reader that falls behind receives
//! SYN_DROPPED, SYN_REPORT and a complete current snapshot.

pub const SCTL_GAMEPAD_CREATE: u32 = 0x4750_0001;
pub const MAX_BUTTONS: usize = 32;
pub const MAX_AXES: usize = 8;

/// Inclusive limits and the value to publish on producer close.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Axis {
    pub code: u16,
    pub reserved: u16,
    pub minimum: i32,
    pub maximum: i32,
    pub neutral: i32,
}

/// Initialize with Default, then fill the declared prefixes. Unused entries
/// and reserved fields must be zero. All integers use native byte order.
/// Supported axes are 0..=0x0a or 0x10..=0x17. Button codes may be joystick/gamepad
/// (0x120..=0x13f), D-pad (0x220..=0x223), or trigger-happy (0x2c0..=0x2e7).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GamepadSpec {
    pub button_count: u16,
    pub axis_count: u16,
    pub buttons: [u16; MAX_BUTTONS],
    pub axes: [Axis; MAX_AXES],
}

const _: () = assert!(core::mem::size_of::<Axis>() == 16);
const _: () = assert!(core::mem::size_of::<GamepadSpec>() == 196);
