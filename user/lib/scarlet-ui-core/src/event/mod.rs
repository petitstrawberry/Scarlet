//! Event types and handling for ScarletUI

use alloc::string::String;
use alloc::vec::Vec;

mod dispatcher;
mod gesture;

pub use dispatcher::{EventDispatcher, HitResult, Phase};
pub use gesture::{
    DragGestureRecognizer, Gesture, GestureManager, GestureRecognizer, LongPressGestureRecognizer,
    TapGestureRecognizer,
};

/// UI Events
#[derive(Clone, Debug)]
pub enum Event {
    /// Quit event - application should exit
    Quit,

    /// Window resize event
    Resize { width: u32, height: u32 },

    /// Screen size changed
    ScreenSizeChanged { width: u32, height: u32 },

    /// Mouse event
    Mouse(MouseEvent),

    /// Keyboard event
    Keyboard(KeyEvent),

    /// Input event (from SWS)
    Input(InputEvent),

    /// Focus event
    Focus(FocusEvent),

    /// Lifecycle event
    Lifecycle(LifecycleEvent),

    /// Custom event with user data
    Custom { event_type: u32, data: Vec<u8> },

    /// Window control event (from Window titlebar buttons)
    Window(WindowEvent),

    /// Menu item activation (from SWS)
    MenuItemActivated {
        window_id: u32,
        menu_item_id: String,
    },

    /// IME preedit text for a text-input context.
    TextInputPreedit {
        context_id: u32,
        serial: u32,
        cursor_byte: u32,
        anchor_byte: u32,
        text: String,
        spans: Vec<u8>,
    },

    /// IME committed text for a text-input context.
    TextInputCommit {
        context_id: u32,
        serial: u32,
        text: String,
    },

    /// Request to delete surrounding text for a text-input context.
    TextInputDeleteSurroundingText {
        context_id: u32,
        serial: u32,
        before_bytes: u32,
        after_bytes: u32,
    },

    /// End of a text-input update batch.
    TextInputDone { context_id: u32, serial: u32 },
}

/// Mouse events
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MouseEvent {
    /// Mouse moved
    Moved { x: i32, y: i32 },
    /// Mouse entered an element
    Entered { x: i32, y: i32 },
    /// Mouse exited an element
    Exited { x: i32, y: i32 },

    /// Mouse button pressed
    ButtonPressed { button: MouseButton, x: i32, y: i32 },

    /// Mouse button released
    ButtonReleased { button: MouseButton, x: i32, y: i32 },

    /// Mouse wheel scrolled
    Wheel { delta_x: i32, delta_y: i32 },
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
    Pressed { keycode: KeyCode },

    /// Key released
    Released { keycode: KeyCode },

    /// Character received (Unicode)
    Char { c: char },
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

/// Window control events
///
/// Fired when user interacts with window titlebar controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowEvent {
    /// Close button was clicked
    CloseRequested,
    /// Maximize button was clicked (expand to screen)
    MaximizeRequested,
    /// Restore button was clicked (restore from maximized/minimized)
    RestoreRequested,
    /// Minimize button was clicked
    MinimizeRequested,
    /// Titlebar was pressed to start interactive move
    MoveRequested,
}

/// Focus events
///
/// Fired when an element gains or loses keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusEvent {
    /// Element gained focus
    Gained,
    /// Element lost focus
    Lost,
}

/// Lifecycle events
///
/// Fired during element lifecycle: mount, unmount, appear, disappear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleEvent {
    /// Element was mounted to the tree
    Mount,
    /// Element will be unmounted from the tree
    Unmount,
    /// Element became visible on screen
    Appear,
    /// Element is no longer visible on screen
    Disappear,
}
