//! Text control
//!
//! Displays text with configurable font, color, and alignment.

extern crate alloc;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::boxed::Box;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::Event;
use crate::graphics::{Canvas, Rect};
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::traits::View;
use crate::color::Color;
use crate::DataContext;
use scarlet_ui_macros::bindable;
use crate::view::controls::{FontConfig, TextAlignment};
use scarlet_std::fmt;

/// Text view for displaying text
#[bindable]
pub struct Text {
    id: ViewId,
    #[bind]
    text: String,
    font: FontConfig,
    color: Color,
    alignment: TextAlignment,
    cached_size: Size,
}

impl Text {
    /// Create a new text view
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    /// Set the text content
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }

    /// Get the text content
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Set the font configuration
    pub fn set_font(&mut self, font: FontConfig) {
        self.font = font;
    }

    /// Get the font configuration
    pub fn font(&self) -> &FontConfig {
        &self.font
    }

    /// Set the text color
    pub fn set_color(&mut self, color: Color) {
        self.color = color;
    }

    /// Get the text color
    pub fn get_color(&self) -> Color {
        self.color
    }

    /// Set the text alignment
    pub fn set_alignment(&mut self, alignment: TextAlignment) {
        self.alignment = alignment;
    }

    /// Get the text alignment
    pub fn alignment(&self) -> TextAlignment {
        self.alignment
    }

    /// Set the font size (chainable builder method)
    pub fn font_size(mut self, size: u32) -> Self {
        self.font.size = size;
        self
    }

    /// Set the text color (chainable builder method)
    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Calculate text size
    fn calculate_text_size(&self, constraints: LayoutConstraints) -> Size {
        // Approximate text size calculation
        // TODO: Use actual font metrics
        let char_width = self.font.size as u32 * 8 / 12; // Rough approximation
        let line_height = self.font.size as u32 * 12 / 10; // 1.2x line height

        let text_width = if self.text.len() > 0 {
            self.text.len() as u32 * char_width
        } else {
            0
        };

        // Constrain to max width
        let width = text_width.min(constraints.max_width);

        // Calculate number of lines based on width constraint
        let max_chars_per_line = if constraints.max_width > 0 && char_width > 0 {
            constraints.max_width / char_width
        } else {
            self.text.len() as u32
        };

        let lines = if max_chars_per_line > 0 {
            (self.text.len() as u32 + max_chars_per_line - 1) / max_chars_per_line
        } else {
            1
        };

        let height = lines * line_height;

        // Ensure at least min dimensions
        let width = width.max(constraints.min_width);
        let height = height.max(constraints.min_height);

        Size::new(width, height)
    }
}

impl crate::view::render::RenderObject for Text {
    fn id(&self) -> ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        let size = self.calculate_text_size(constraints);
        self.cached_size = size;
        size
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        // Draw text using canvas
        ctx.canvas.draw_text_sized(
            frame.x,
            frame.y,
            &self.text,
            self.color,
            self.font.size as f32,
        );
    }

    fn event(&mut self, _ctx: &mut EventCtx, _event: &Event) -> ControlFlow {
        ControlFlow::Continue
    }

    fn update(&mut self, _ctx: &mut UpdateCtx) {
        // Text doesn't need periodic updates
    }
}

// View is auto-implemented since View: RenderObject

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Text")
            .field("text", &self.text)
            .field("font", &self.font)
            .field("color", &self.color)
            .field("alignment", &self.alignment)
            .field("cached_size", &self.cached_size)
            .field("has_text_binding", &self.text_data.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_new() {
        let text = Text::new("Hello, World!");
        assert_eq!(text.text(), "Hello, World!");
    }

    #[test]
    fn test_text_set_text() {
        let mut text = Text::new("Hello");
        text.set_text("Goodbye".to_string());
        assert_eq!(text.text(), "Goodbye");
    }

    #[test]
    fn test_text_set_color() {
        let mut text = Text::new("Test");
        text.set_color(Color::rgb(255, 0, 0));
        assert_eq!(text.color(), Color::rgb(255, 0, 0));
    }

    #[test]
    fn test_text_set_alignment() {
        let mut text = Text::new("Test");
        text.set_alignment(TextAlignment::Center);
        assert_eq!(text.alignment(), TextAlignment::Center);
    }
}
