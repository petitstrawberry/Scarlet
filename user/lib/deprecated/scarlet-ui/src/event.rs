//! Event handling

use crate::graphics::Point;

/// Mouse button
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left = 0,
    Right = 1,
    Middle = 2,
}

impl MouseButton {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(MouseButton::Left),
            1 => Some(MouseButton::Right),
            2 => Some(MouseButton::Middle),
            _ => None,
        }
    }
}

/// Event propagation state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropagationState {
    /// Event continues propagating
    Propagating,
    /// Propagation was stopped
    Stopped,
}

/// The kind of event
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// Mouse moved
    MouseMove,
    /// Mouse button pressed
    MouseDown { button: MouseButton },
    /// Mouse button released
    MouseUp { button: MouseButton },
    /// Mouse entered view bounds
    MouseEnter,
    /// Mouse left view bounds
    MouseLeave,
    /// Key pressed
    KeyDown { code: u16 },
    /// Key released
    KeyUp { code: u16 },
    /// View gained focus
    Focus,
    /// View lost focus
    Blur,
}

/// UI Event with position and propagation control
#[derive(Debug, Clone, Copy)]
pub struct Event {
    /// The kind of event
    pub kind: EventKind,
    /// Position for pointer events (window-relative)
    pub position: Point,
    /// Propagation state
    propagation: PropagationState,
}

impl Event {
    /// Create a new event
    pub fn new(kind: EventKind, position: Point) -> Self {
        Self {
            kind,
            position,
            propagation: PropagationState::Propagating,
        }
    }

    /// Create a mouse move event
    pub fn mouse_move(x: i32, y: i32) -> Self {
        Self::new(EventKind::MouseMove, Point::new(x, y))
    }

    /// Create a mouse down event
    pub fn mouse_down(x: i32, y: i32, button: MouseButton) -> Self {
        Self::new(EventKind::MouseDown { button }, Point::new(x, y))
    }

    /// Create a mouse up event
    pub fn mouse_up(x: i32, y: i32, button: MouseButton) -> Self {
        Self::new(EventKind::MouseUp { button }, Point::new(x, y))
    }

    /// Create a key down event
    pub fn key_down(code: u16) -> Self {
        Self::new(EventKind::KeyDown { code }, Point::ZERO)
    }

    /// Create a key up event
    pub fn key_up(code: u16) -> Self {
        Self::new(EventKind::KeyUp { code }, Point::ZERO)
    }

    /// Stop event propagation
    pub fn stop_propagation(&mut self) {
        self.propagation = PropagationState::Stopped;
    }

    /// Check if propagation was stopped
    pub fn is_stopped(&self) -> bool {
        self.propagation == PropagationState::Stopped
    }

    /// Get the x coordinate
    pub fn x(&self) -> i32 {
        self.position.x
    }

    /// Get the y coordinate
    pub fn y(&self) -> i32 {
        self.position.y
    }
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
