//! Event handling

/// Mouse button
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// High-level UI event
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Mouse moved to position
    MouseMove { x: i32, y: i32 },
    /// Mouse button pressed
    MouseDown(MouseButton),
    /// Mouse button released
    MouseUp(MouseButton),
    /// Key pressed/released
    Key { code: u16, pressed: bool },
    /// Window close requested
    WindowClose,
}

/// Legacy event type (for compatibility)
#[derive(Debug, Clone, Copy)]
pub enum EventType {
    MouseMove { x: i32, y: i32 },
    MouseDown { button: MouseButton, x: i32, y: i32 },
    MouseUp { button: MouseButton, x: i32, y: i32 },
    KeyDown { code: u16 },
    KeyUp { code: u16 },
}
