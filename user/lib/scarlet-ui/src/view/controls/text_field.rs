//! TextField control
//!
//! A text input field for user text entry.

extern crate alloc;
use alloc::string::String;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::{Event, EventKind, MouseButton};
use crate::graphics::Rect;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::traits::View;
use crate::color::Color;
use crate::state::data::DataContext;
use scarlet_ui_macros::bindable;
use scarlet_std::fmt;

/// Text field view
#[bindable]
pub struct TextField {
    id: ViewId,
    #[bind]
    text: String,
    placeholder: String,
    font_size: u32,
    text_color: Color,
    placeholder_color: Color,
    background_color: Color,
    border_color: Color,
    corner_radius: u32,
    padding: u32,
    is_focused: bool,
    cursor_pos: usize,
    cached_size: Size,
}

impl TextField {
    /// Create a new text field
    pub fn new() -> Self {
        Self {
            corner_radius: 4,
            padding: 8,
            ..Self::default()
        }
    }

    /// Create a text field with placeholder text
    pub fn with_placeholder(placeholder: impl Into<String>) -> Self {
        let mut tf = Self::new();
        tf.placeholder = placeholder.into();
        tf
    }

    /// Create a text field with initial text
    pub fn with_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let mut tf = Self::new();
        tf.text = text.clone();
        tf.cursor_pos = text.len();
        tf
    }

    /// Get the current text
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Set the text
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        // Update bound DataContext if present
        if let Some(ref data) = self.text_data {
            data.set(self.text.clone());
        }
    }

    /// Get the placeholder text
    pub fn get_placeholder(&self) -> &str {
        &self.placeholder
    }

    /// Set the placeholder text
    pub fn set_placeholder(&mut self, placeholder: impl Into<String>) {
        self.placeholder = placeholder.into();
    }

    /// Set the placeholder text (chainable)
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Get whether the field is focused
    pub fn is_focused(&self) -> bool {
        self.is_focused
    }

    /// Set focus state
    pub fn set_focused(&mut self, focused: bool) {
        self.is_focused = focused;
    }

    /// Handle keyboard input
    fn handle_key_event(&mut self, event: &Event) -> bool {
        match &event.kind {
            EventKind::KeyDown { code } => {
                // Handle special keys by scancode
                // TODO: Implement proper key mapping from scancode
                // For now, just note that we received a key event
                match code {
                    0x0E => { // Backspace
                        if self.cursor_pos > 0 {
                            self.text.remove(self.cursor_pos - 1);
                            self.cursor_pos -= 1;
                            // Update bound DataContext
                            if let Some(ref data) = self.text_data {
                                data.set(self.text.clone());
                            }
                            return true;
                        }
                    }
                    0x04 => { // Delete (simplified)
                        if self.cursor_pos < self.text.len() {
                            self.text.remove(self.cursor_pos);
                            // Update bound DataContext
                            if let Some(ref data) = self.text_data {
                                data.set(self.text.clone());
                            }
                            return true;
                        }
                    }
                    _ => {
                        // For now, ignore other keys
                        // TODO: Implement character input handling
                    }
                }
            }
            _ => {}
        }
        false
    }

    /// Handle mouse events
    fn handle_mouse_event(&mut self, event: &Event) -> bool {
        match &event.kind {
            EventKind::MouseDown { button, .. } => {
                if *button == MouseButton::Left {
                    // Focus on click
                    let was_focused = self.is_focused;
                    self.is_focused = true;
                    return !was_focused;
                }
            }
            _ => {}
        }
        false
    }

    /// Calculate text field height
    fn calculate_height(&self) -> u32 {
        // Height based on font size + padding
        self.font_size * 12 / 10 + 10 // Line height + padding
    }
}

impl crate::view::render::RenderObject for TextField {
    fn id(&self) -> ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        let height = self.calculate_height();

        let width = if constraints.min_width > 0 {
            constraints.min_width
        } else if constraints.max_width > 0 {
            constraints.max_width
        } else {
            200 // Default width
        };

        let size = Size::new(
            width.min(constraints.max_width),
            height.max(constraints.min_height).min(constraints.max_height),
        );

        self.cached_size = size;
        size
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        // Background with rounded corners
        ctx.canvas.fill_rounded_rect(
            frame.x,
            frame.y,
            frame.width,
            frame.height,
            self.corner_radius,
            self.background_color,
        );

        // Border (thicker if focused)
        let border_color = if self.is_focused {
            self.border_color
        } else {
            Color::rgb(180, 180, 180)
        };

        let border_width = if self.is_focused { 2 } else { 1 };
        for i in 0..border_width {
            ctx.canvas.draw_rounded_rect(
                frame.x + i as i32,
                frame.y + i as i32,
                frame.width.saturating_sub(i * 2),
                frame.height.saturating_sub(i * 2),
                self.corner_radius.saturating_sub(i),
                border_color,
            );
        }

        // Display text (or placeholder if empty)
        let display_text = if self.text.is_empty() {
            &self.placeholder
        } else {
            &self.text
        };

        let text_color = if self.text.is_empty() {
            self.placeholder_color
        } else {
            self.text_color
        };

        // Draw text with padding
        ctx.canvas.draw_text(
            frame.x + self.padding as i32,
            frame.y + (frame.height as i32 - 16) / 2,
            display_text,
            text_color,
        );

        // Draw caret if focused
        if self.is_focused {
            let before_cursor = &self.text[..self.cursor_pos.min(self.text.len())];
            let (text_width, _) = crate::graphics::measure_text_sized(before_cursor, 16.0);
            let caret_x = frame.x as i32 + self.padding as i32 + text_width as i32;
            let caret_y = frame.y + 6;

            ctx.canvas.fill_rect(
                caret_x,
                caret_y,
                2,
                frame.height.saturating_sub(12),
                Color::rgb(50, 150, 255),
            );
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        let needs_repaint = self.handle_key_event(event) || self.handle_mouse_event(event);

        if needs_repaint {
            ctx.request_paint();
        }

        ControlFlow::Continue
    }

    fn update(&mut self, _ctx: &mut UpdateCtx) {
        // TextField doesn't need periodic updates
    }
}

impl fmt::Debug for TextField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextField")
            .field("text", &self.text)
            .field("placeholder", &self.placeholder.as_str())
            .field("is_focused", &self.is_focused)
            .field("cursor_pos", &self.cursor_pos)
            .field("has_text_binding", &self.text_data.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_field_new() {
        let tf = TextField::new();
        assert_eq!(tf.text(), "");
        assert!(!tf.is_focused());
    }

    #[test]
    fn test_text_field_with_text() {
        let tf = TextField::with_text("Hello");
        assert_eq!(tf.text(), "Hello");
        assert_eq!(tf.cursor_pos, 5);
    }

    #[test]
    fn test_text_field_with_placeholder() {
        let tf = TextField::with_placeholder("Enter text");
        assert_eq!(tf.placeholder(), "Enter text");
    }

    #[test]
    fn test_text_field_set_text() {
        let mut tf = TextField::new();
        tf.set_text("New text".to_string());
        assert_eq!(tf.text(), "New text");
    }

    #[test]
    fn test_text_field_set_focused() {
        let mut tf = TextField::new();
        tf.set_focused(true);
        assert!(tf.is_focused());
    }
}
