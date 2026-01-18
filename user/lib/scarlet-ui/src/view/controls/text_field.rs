//! TextField control
//!
//! A text input field for user text entry.

extern crate alloc;
use alloc::string::String;
use alloc::sync::Arc;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::{Event, EventKind, MouseButton};
use crate::graphics::Rect;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::traits::View;
use crate::color::Color;
use crate::view::controls::text::Text;
use scarlet_std::fmt;

/// Text field view
pub struct TextField {
    id: ViewId,
    text: Arc<String>,
    placeholder: Arc<String>,
    font_size: u32,
    text_color: Color,
    placeholder_color: Color,
    background_color: Color,
    border_color: Color,
    is_focused: bool,
    cursor_position: usize,
    cached_size: Size,
}

impl TextField {
    /// Create a new text field
    pub fn new() -> Self {
        Self {
            id: ViewId::new(),
            text: Arc::new(String::new()),
            placeholder: Arc::new(String::new()),
            font_size: 14,
            text_color: Color::TEXT,
            placeholder_color: Color::GRAY,
            background_color: Color::WHITE,
            border_color: Color::BORDER,
            is_focused: false,
            cursor_position: 0,
            cached_size: Size::ZERO,
        }
    }

    /// Create a text field with placeholder text
    pub fn with_placeholder(placeholder: impl Into<Arc<String>>) -> Self {
        let mut tf = Self::new();
        tf.placeholder = placeholder.into();
        tf
    }

    /// Create a text field with initial text
    pub fn with_text(text: impl Into<Arc<String>>) -> Self {
        let text = text.into();
        let mut tf = Self::new();
        tf.text = text.clone();
        tf.cursor_position = text.len();
        tf
    }

    /// Get the current text
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Set the text
    pub fn set_text(&mut self, text: impl Into<Arc<String>>) {
        self.text = text.into();
    }

    /// Get the placeholder text
    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    /// Set the placeholder text
    pub fn set_placeholder(&mut self, placeholder: impl Into<Arc<String>>) {
        self.placeholder = placeholder.into();
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
                        if self.cursor_position > 0 {
                            let mut text = (*self.text).clone();
                            text.remove(self.cursor_position - 1);
                            self.text = Arc::new(text);
                            self.cursor_position -= 1;
                            return true;
                        }
                    }
                    0x04 => { // Delete (simplified)
                        if self.cursor_position < self.text.len() {
                            let mut text = (*self.text).clone();
                            text.remove(self.cursor_position);
                            self.text = Arc::new(text);
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

impl Default for TextField {
    fn default() -> Self {
        Self::new()
    }
}

impl View for TextField {
    fn id(&self) -> ViewId {
        self.id
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
        // TODO: Implement actual text field rendering
        let _ = (ctx, frame);

        // Draw background
        // Draw border (thicker if focused)
        // Draw text or placeholder
        // Draw cursor if focused
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
            .field("text", &self.text.as_str())
            .field("placeholder", &self.placeholder.as_str())
            .field("is_focused", &self.is_focused)
            .field("cursor_position", &self.cursor_position)
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
        assert_eq!(tf.cursor_position, 5);
    }

    #[test]
    fn test_text_field_with_placeholder() {
        let tf = TextField::with_placeholder("Enter text");
        assert_eq!(tf.placeholder(), "Enter text");
    }

    #[test]
    fn test_text_field_set_text() {
        let mut tf = TextField::new();
        tf.set_text(Arc::new("New text".into()));
        assert_eq!(tf.text(), "New text");
    }

    #[test]
    fn test_text_field_set_focused() {
        let mut tf = TextField::new();
        tf.set_focused(true);
        assert!(tf.is_focused());
    }
}
