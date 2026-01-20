//! Window delegate trait for event handling
//!
//! Inspired by AppKit's NSWindowDelegate pattern.

use crate::{Canvas, MouseButton};

/// Window delegate trait for handling window events
///
/// Implement this trait to respond to window lifecycle and input events.
/// All methods have default implementations that do nothing.
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{WindowDelegate, Canvas, Color, MouseButton};
///
/// struct MyWindowDelegate {
///     click_count: u32,
/// }
///
/// impl WindowDelegate for MyWindowDelegate {
///     fn on_draw(&mut self, canvas: &mut Canvas) {
///         canvas.fill(Color::WHITE);
///         canvas.draw_text(10, 10, "Hello!", Color::BLACK);
///     }
///
///     fn on_click(&mut self, button: MouseButton, x: i32, y: i32) {
///         self.click_count += 1;
///     }
///
///     fn should_close(&mut self) -> bool {
///         true // Allow close
///     }
/// }
/// ```
pub trait WindowDelegate {
    /// Called when the window content needs to be drawn
    ///
    /// The canvas covers the content area (excluding window decorations).
    /// After this method returns, the window is automatically committed.
    fn on_draw(&mut self, _canvas: &mut Canvas) {}

    /// Called when the mouse moves within the window
    fn on_mouse_move(&mut self, _x: i32, _y: i32) {}

    /// Called when a mouse button is pressed
    fn on_mouse_down(&mut self, _button: MouseButton, _x: i32, _y: i32) {}

    /// Called when a mouse button is released
    fn on_mouse_up(&mut self, _button: MouseButton, _x: i32, _y: i32) {}

    /// Called when a mouse button is clicked (pressed and released)
    fn on_click(&mut self, _button: MouseButton, _x: i32, _y: i32) {}

    /// Called when a key is pressed
    fn on_key_down(&mut self, _key_code: u16) {}

    /// Called when a key is released
    fn on_key_up(&mut self, _key_code: u16) {}

    /// Called when the window close button is clicked
    ///
    /// Return `true` to allow the close, `false` to cancel.
    fn should_close(&mut self) -> bool {
        true
    }

    /// Called when the window is about to close
    fn on_close(&mut self) {}

    /// Called periodically in the event loop (for animations, etc.)
    fn on_idle(&mut self) {}
}

/// Empty delegate that does nothing (for windows without event handling)
pub struct EmptyDelegate;

impl WindowDelegate for EmptyDelegate {}
