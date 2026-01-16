//! Event types for SWS client

/// Input event from the server
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEvent {
    /// Target surface/window id
    pub surface_id: u32,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Input event (keyboard, mouse, etc.)
    Input(InputEvent),
    /// Compositor requests the surface to resize.
    ///
    /// Clients should respond by resizing the surface buffer (e.g. via
    /// `Connection::resize_window`).
    SurfaceConfigure {
        surface_id: u32,
        width: u32,
        height: u32,
    },
    /// Window was destroyed by server
    SurfaceDestroyed { surface_id: u32 },
    /// Focus changed to a different window
    FocusChanged {
        window_id: u32,
        app_id: std::string::String,
        app_name: std::string::String,
        title: std::string::String,
        menu_titles: std::string::String,
    },
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
