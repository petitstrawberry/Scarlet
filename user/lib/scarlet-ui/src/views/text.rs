//! Text View - Displays text with styling options
//!
//! Text view renders text with configurable font size, color, and other styling.

use alloc::string::String;
use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::{Size, Point};
use crate::element::LayoutConstraints;
use crate::color::Color;
use alloc::boxed::Box;

/// Text View - displays a string of text
#[derive(Clone)]
pub struct Text {
    content: String,
    font_size: f32,
    color: Color,
}

impl Text {
    /// Create a new Text view with the given content
    pub fn new(content: impl Into<String>) -> Self {
        let content_str = content.into();
        Self {
            content: content_str,
            font_size: 16.0,
            color: Color::BLACK,
        }
    }

    /// Set the font size in points
    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    /// Set the text color
    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Get the text content
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Get the font size
    pub fn font_size_value(&self) -> f32 {
        self.font_size
    }

    /// Get the text color
    pub fn text_color(&self) -> Color {
        self.color
    }
}

impl View for Text {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            TextRenderObject::new(self.content.clone(), self.font_size, self.color),
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        alloc::vec::Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Text RenderObject - handles text rendering
pub struct TextRenderObject {
    content: String,
    font_size: f32,
    color: Color,
    size: Size,
}

impl TextRenderObject {
    /// Create a new TextRenderObject
    pub fn new(content: String, font_size: f32, color: Color) -> Self {
        Self {
            content,
            font_size,
            color,
            size: Size::ZERO,
        }
    }

    /// Estimate text size based on content and font size
    ///
    /// This is a simplified implementation. In a real system, this would
    /// use font metrics to measure the actual text dimensions.
    fn estimate_size(&self) -> Size {
        let char_width = self.font_size * 0.6; // Approximate character width
        let line_height = self.font_size * 1.2; // Line height

        // Find the longest line for width
        let max_line_width = self
            .content
            .lines()
            .map(|line| line.len() as f32 * char_width)
            .reduce(|a, b| a.max(b))
            .unwrap_or(0.0);

        let line_count = self.content.lines().count() as f32;

        Size {
            width: max_line_width,
            height: line_count * line_height,
        }
    }
}

impl ElementRenderObject for TextRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Calculate intrinsic size
        let intrinsic = self.estimate_size();

        // Apply constraints
        let width = if constraints.max_width > 0.0 {
            intrinsic.width.min(constraints.max_width).max(constraints.min_width)
        } else {
            intrinsic.width
        };

        let height = if constraints.max_height > 0.0 {
            intrinsic.height.min(constraints.max_height).max(constraints.min_height)
        } else {
            intrinsic.height
        };

        self.size = Size { width, height };
        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn hit_test(&self, point: Point) -> bool {
        let bounds = crate::geometry::Rect {
            origin: Point::ZERO,
            size: self.size,
        };
        bounds.contains(point)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
