//! Event types for SWS client

/// Input event from the server
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEvent {
    /// Timestamp in microseconds
    pub time: u64,
    /// Event type (EV_KEY, EV_REL, EV_ABS, etc.)
    pub type_: u16,
    /// Event code (KEY_A, REL_X, ABS_X, etc.)
    pub code: u16,
    /// Event value
    pub value: i32,
}

/// Events from the SWS server
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Input event (keyboard, mouse, etc.)
    Input(InputEvent),
    /// Window was destroyed by server
    SurfaceDestroyed { surface_id: u32 },
    /// Error from server
    Error { code: u32 },
}

// Linux input event types (from linux/input-event-codes.h)
pub mod event_type {
    pub const EV_SYN: u16 = 0x00;
    pub const EV_KEY: u16 = 0x01;
    pub const EV_REL: u16 = 0x02;
    pub const EV_ABS: u16 = 0x03;
}

// Linux absolute axis codes
pub mod abs_code {
    pub const ABS_X: u16 = 0x00;
    pub const ABS_Y: u16 = 0x01;
}

// Linux relative axis codes
pub mod rel_code {
    pub const REL_X: u16 = 0x00;
    pub const REL_Y: u16 = 0x01;
}

// Common key codes
pub mod key_code {
    pub const BTN_LEFT: u16 = 0x110;
    pub const BTN_RIGHT: u16 = 0x111;
    pub const BTN_MIDDLE: u16 = 0x112;
}
