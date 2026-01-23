//! Event types and handling for ScarletUI

use alloc::vec::Vec;

/// UI Events
#[derive(Clone, Debug)]
pub enum Event {
    /// Quit event - application should exit
    Quit,

    /// Window resize event
    Resize {
        width: u32,
        height: u32,
    },

    /// Mouse event
    Mouse(MouseEvent),

    /// Keyboard event
    Keyboard(KeyEvent),

    /// Input event (from SWS)
    Input(InputEvent),

    /// Custom event with user data
    Custom {
        event_type: u32,
        data: Vec<u8>,
    },
}

/// Mouse events
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MouseEvent {
    /// Mouse moved
    Moved {
        x: i32,
        y: i32,
    },

    /// Mouse button pressed
    ButtonPressed {
        button: MouseButton,
        x: i32,
        y: i32,
    },

    /// Mouse button released
    ButtonReleased {
        button: MouseButton,
        x: i32,
        y: i32,
    },

    /// Mouse wheel scrolled
    Wheel {
        delta_x: i32,
        delta_y: i32,
    },
}

/// Mouse button
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// Keyboard events
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyEvent {
    /// Key pressed
    Pressed {
        keycode: KeyCode,
    },

    /// Key released
    Released {
        keycode: KeyCode,
    },

    /// Character received (Unicode)
    Char {
        c: char,
    },
}

/// Key codes
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCode {
    Unknown,
    Escape,
    Enter,
    Tab,
    Backspace,
    Space,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
    Char(char),
}

/// Input event (from SWS input system)
#[derive(Clone, Copy, Debug)]
pub struct InputEvent {
    pub timestamp: u64,
    pub event_type: u16,
    pub code: u16,
    pub value: i32,
}


