//! Input event handling module

use std::fs::File;
use std::println;

/// Input event structure (16 bytes, matches kernel InputEvent)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct InputEvent {
    pub time: u64,  // 8 bytes - timestamp in nanoseconds
    pub type_: u16, // 2 bytes - event type
    pub code: u16,  // 2 bytes - event code
    pub value: i32, // 4 bytes - event value
}

impl InputEvent {
    pub const SIZE: usize = core::mem::size_of::<Self>();
}

/// Event types
pub mod event_types {
    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const EV_REL: u16 = 0x02;
    pub const EV_ABS: u16 = 0x03;
}

/// Relative axis codes
pub mod rel_codes {
    pub const REL_X: u16 = 0x00;
    pub const REL_Y: u16 = 0x01;
    pub const REL_WHEEL: u16 = 0x08;
}

/// Absolute axis codes
pub mod abs_codes {
    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
}

/// Key codes
pub mod key_codes {
    pub const BTN_LEFT: u16 = 0x110;
    pub const BTN_RIGHT: u16 = 0x111;
    pub const BTN_MIDDLE: u16 = 0x112;
}

/// Input manager - handles input devices and event reading
pub struct InputManager {
    mouse_file: File,
    /// Maximum value for tablet absolute coordinates (typically 32767)
    tablet_max: i32,
    /// Current accumulated position for absolute positioning
    pub abs_x: Option<i32>,
    pub abs_y: Option<i32>,
}

impl InputManager {
    /// Create a new input manager
    pub fn new() -> Result<Self, &'static str> {
        // Try to open tablet device first (absolute positioning), fallback to mouse (relative)
        let mouse_file = match File::open("/dev/tablet0") {
            Ok(file) => {
                println!("[InputManager] Opened tablet device (absolute positioning)");
                file
            }
            Err(_) => {
                println!("[InputManager] Tablet device not found, trying mouse device...");
                File::open("/dev/mouse0").map_err(|_| "Failed to open mouse or tablet device")?
            }
        };

        Ok(Self {
            mouse_file,
            tablet_max: 32767, // Standard virtio-tablet range
            abs_x: None,
            abs_y: None,
        })
    }

    /// Read a single input event
    pub fn read_event(&mut self) -> Result<Option<InputEvent>, &'static str> {
        let mut buffer = [0u8; InputEvent::SIZE];

        let bytes_read = self.mouse_file.read(&mut buffer).map_err(|e| {
            println!("[InputManager] Read error: {:?}", e);
            "Failed to read input event"
        })?;

        if bytes_read != InputEvent::SIZE {
            return Ok(None); // No complete event available
        }

        // Parse event
        let event = unsafe { core::ptr::read(buffer.as_ptr() as *const InputEvent) };

        Ok(Some(event))
    }

    /// Scale tablet coordinates to screen coordinates
    pub fn scale_tablet_coord(&self, value: i32, screen_dimension: u32) -> i32 {
        ((value as i64 * screen_dimension as i64) / self.tablet_max as i64) as i32
    }
}
