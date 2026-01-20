use crate::geometry::Point;
use crate::node_id::NodeId;

mod dispatcher;
mod focus;

pub use dispatcher::EventDispatcher;
pub use focus::FocusManager;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Mouse(MouseEvent),
    Key(KeyEvent),
    Focus(bool),
}

impl Event {
    pub fn is_keyboard(&self) -> bool {
        matches!(self, Event::Key(_))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MouseEvent {
    pub position: Point,
    pub buttons: MouseButtons,
    pub kind: MouseEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MouseButtons(pub u8);

impl MouseButtons {
    pub const LEFT: Self = Self(0x01);
    pub const RIGHT: Self = Self(0x02);
    pub const MIDDLE: Self = Self(0x04);

    pub fn is_left_pressed(&self) -> bool {
        self.0 & 0x01 != 0
    }

    pub fn is_right_pressed(&self) -> bool {
        self.0 & 0x02 != 0
    }

    pub fn is_middle_pressed(&self) -> bool {
        self.0 & 0x04 != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MouseEventKind {
    Press,
    Release,
    Move,
    Scroll { delta: Point },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    Char(char),
    Enter,
    Backspace,
    Escape,
    Tab,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EventPhase {
    #[default]
    Capture,
    Target,
    Bubble,
}

#[derive(Debug, Clone, Default)]
pub struct EventContext {
    pub phase: EventPhase,
    pub stop_propagation: bool,
    pub stop_immediate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitResult {
    Handled(NodeId),
    Passthrough,
    Stop,
}
