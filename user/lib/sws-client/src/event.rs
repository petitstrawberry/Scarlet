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

/// Text-input context state sent to an input method service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImeContextState {
    pub context_id: u32,
    pub window_id: u32,
    pub serial: u32,
    pub cursor_x: i32,
    pub cursor_y: i32,
    pub cursor_width: u32,
    pub cursor_height: u32,
    pub content_hint: u32,
    pub content_purpose: u32,
    pub text_change_cause: u32,
    pub cursor_byte: u32,
    pub anchor_byte: u32,
    pub surrounding_text: std::string::String,
}

/// Events from the SWS server
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Input event (keyboard, mouse, etc.)
    Input(InputEvent),
    /// IME preedit text for a text-input context.
    TextInputPreedit {
        context_id: u32,
        serial: u32,
        cursor_byte: u32,
        anchor_byte: u32,
        text: std::string::String,
        spans: std::vec::Vec<u8>,
    },
    /// IME committed text for a text-input context.
    TextInputCommit {
        context_id: u32,
        serial: u32,
        text: std::string::String,
    },
    /// Request to delete surrounding text before applying the next commit.
    TextInputDeleteSurroundingText {
        context_id: u32,
        serial: u32,
        before_bytes: u32,
        after_bytes: u32,
    },
    /// End of a text-input update batch.
    TextInputDone { context_id: u32, serial: u32 },
    /// Structured candidate list for a text-input context.
    TextInputCandidates {
        context_id: u32,
        serial: u32,
        selected_index: u32,
        page_start: u32,
        page_size: u32,
        anchor_byte: u32,
        candidates: std::vec::Vec<u8>,
    },
    /// Hide candidate UI for a text-input context.
    TextInputHideCandidates { context_id: u32, serial: u32 },
    /// IME status for a text-input context.
    TextInputStatus {
        context_id: u32,
        serial: u32,
        state: u32,
        mode_id: u32,
        flags: u32,
        mode_label: std::string::String,
    },
    /// A text-input context became active for this IME service.
    ImeActivate(ImeContextState),
    /// A text-input context became inactive for this IME service.
    ImeDeactivate { context_id: u32, serial: u32 },
    /// A text-input context updated state for this IME service.
    ImeContextState(ImeContextState),
    /// Key event routed to this IME service.
    ImeKeyEvent {
        context_id: u32,
        key_serial: u32,
        window_id: u32,
        time: u64,
        type_: u16,
        code: u16,
        value: i32,
    },
    /// Reset request for this IME service.
    ImeReset { context_id: u32, serial: u32 },
    /// Trigger key or compositor action addressed to this IME service.
    ImeTrigger {
        context_id: u32,
        serial: u32,
        trigger_id: u32,
        code: u16,
        time: u64,
    },
    /// Compositor requests the surface to resize.
    ///
    /// Clients should respond by resizing the surface buffer (e.g. via
    /// `Connection::resize_window`).
    SurfaceConfigure {
        surface_id: u32,
        width: u32,
        height: u32,
    },
    /// Display size changed.
    ScreenSizeChanged { width: u32, height: u32 },
    /// Output scale changed.
    OutputScaleChanged { scale_milli: u32 },
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
    /// Active application changed (normal window gained focus)
    /// This is separate from FocusChanged because TaskBar/Desktop/etc
    /// can receive focus without changing the active application
    ActiveAppChanged {
        window_id: u32,
        app_id: std::string::String,
        app_name: std::string::String,
        title: std::string::String,
        menu_titles: std::string::String,
    },
    /// Menu item activation for a window
    MenuItemActivated {
        window_id: u32,
        menu_item_id: std::string::String,
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

// Common key codes (from linux/input-event-codes.h)
pub mod key_code {
    // Mouse buttons
    pub const BTN_LEFT: u16 = 0x110;
    pub const BTN_RIGHT: u16 = 0x111;
    pub const BTN_MIDDLE: u16 = 0x112;

    // Keyboard
    pub const KEY_ESC: u16 = 0x01;
    pub const KEY_1: u16 = 0x02;
    pub const KEY_2: u16 = 0x03;
    pub const KEY_3: u16 = 0x04;
    pub const KEY_4: u16 = 0x05;
    pub const KEY_5: u16 = 0x06;
    pub const KEY_6: u16 = 0x07;
    pub const KEY_7: u16 = 0x08;
    pub const KEY_8: u16 = 0x09;
    pub const KEY_9: u16 = 0x0a;
    pub const KEY_0: u16 = 0x0b;
    pub const KEY_ENTER: u16 = 0x1c;
    pub const KEY_BACKSPACE: u16 = 0x0e;
    pub const KEY_TAB: u16 = 0x0f;
    pub const KEY_SPACE: u16 = 0x39;
    pub const KEY_Q: u16 = 0x10;
    pub const KEY_W: u16 = 0x11;
    pub const KEY_E: u16 = 0x12;
    pub const KEY_R: u16 = 0x13;
    pub const KEY_T: u16 = 0x14;
    pub const KEY_Y: u16 = 0x15;
    pub const KEY_U: u16 = 0x16;
    pub const KEY_I: u16 = 0x17;
    pub const KEY_O: u16 = 0x18;
    pub const KEY_P: u16 = 0x19;
    pub const KEY_A: u16 = 0x1e;
    pub const KEY_S: u16 = 0x1f;
    pub const KEY_D: u16 = 0x20;
    pub const KEY_F: u16 = 0x21;
    pub const KEY_G: u16 = 0x22;
    pub const KEY_H: u16 = 0x23;
    pub const KEY_J: u16 = 0x24;
    pub const KEY_K: u16 = 0x25;
    pub const KEY_L: u16 = 0x26;
    pub const KEY_Z: u16 = 0x2c;
    pub const KEY_X: u16 = 0x2d;
    pub const KEY_C: u16 = 0x2e;
    pub const KEY_V: u16 = 0x2f;
    pub const KEY_B: u16 = 0x30;
    pub const KEY_N: u16 = 0x31;
    pub const KEY_M: u16 = 0x32;
    pub const KEY_COMMA: u16 = 0x33;
    pub const KEY_DOT: u16 = 0x34;
    pub const KEY_SLASH: u16 = 0x35;
    pub const KEY_SEMICOLON: u16 = 0x27;
    pub const KEY_APOSTROPHE: u16 = 0x28;
    pub const KEY_LEFTBRACE: u16 = 0x1a;
    pub const KEY_RIGHTBRACE: u16 = 0x1b;
    pub const KEY_BACKSLASH: u16 = 0x2b;
    pub const KEY_MINUS: u16 = 0x0c;
    pub const KEY_EQUAL: u16 = 0x0d;
    pub const KEY_HOME: u16 = 0x66;
    pub const KEY_UP: u16 = 0x67;
    pub const KEY_PAGEUP: u16 = 0x68;
    pub const KEY_LEFT: u16 = 0x69;
    pub const KEY_RIGHT: u16 = 0x6a;
    pub const KEY_END: u16 = 0x6b;
    pub const KEY_DOWN: u16 = 0x6c;
    pub const KEY_PAGEDOWN: u16 = 0x6d;
    pub const KEY_INSERT: u16 = 0x6e;
    pub const KEY_DELETE: u16 = 0x6f;
    pub const KEY_F1: u16 = 0x3b;
    pub const KEY_F2: u16 = 0x3c;
    pub const KEY_F3: u16 = 0x3d;
    pub const KEY_F4: u16 = 0x3e;
    pub const KEY_F5: u16 = 0x3f;
    pub const KEY_F6: u16 = 0x40;
    pub const KEY_F7: u16 = 0x41;
    pub const KEY_F8: u16 = 0x42;
    pub const KEY_F9: u16 = 0x43;
    pub const KEY_F10: u16 = 0x44;
    pub const KEY_F11: u16 = 0x57;
    pub const KEY_F12: u16 = 0x58;
}
