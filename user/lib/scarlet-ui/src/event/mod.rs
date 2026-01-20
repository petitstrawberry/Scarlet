use crate::geometry::Point;
use crate::node_id::NodeId;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Mouse(MouseEvent),
    Key(KeyEvent),
    Focus(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MouseEvent {
    pub position: Point,
    pub buttons: MouseButtons,
    pub kind: MouseEventKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MouseButtons;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MouseEventKind {
    Press,
    Release,
    Move,
    Scroll { delta: Point },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    // TODO: Implement
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
