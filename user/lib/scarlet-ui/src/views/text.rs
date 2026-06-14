//! Text View - Displays text with styling options
//!
//! Text view renders text with configurable font size, color, and other styling.

use crate::buffer::Buffer;
use crate::color::{Color, ColorPalette};
use crate::element::LayoutConstraints;
use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::{Point, Size};
use crate::graphics;
use crate::view::View;
use alloc::boxed::Box;
use alloc::string::String;
use core::any::Any;

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
        let palette = ColorPalette::default();
        Self {
            content: content_str,
            font_size: 16.0,
            color: palette.text_primary(),
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
    buffer: Option<Buffer>,
}

impl TextRenderObject {
    /// Create a new TextRenderObject
    pub fn new(content: String, font_size: f32, color: Color) -> Self {
        Self {
            content,
            font_size,
            color,
            size: Size::ZERO,
            buffer: None,
        }
    }

    /// Get the buffer
    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    /// Get the buffer mutably
    pub fn buffer_mut(&mut self) -> Option<&mut Buffer> {
        self.buffer.as_mut()
    }
}

impl ElementRenderObject for TextRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        if crate::debug::is_enabled() {
            crate::logln!(
                "[TextRenderObject::layout] START: content='{}', constraints=({:?}, {:?}) -> ({:?}, {:?})",
                self.content,
                constraints.min_width,
                constraints.min_height,
                constraints.max_width,
                constraints.max_height
            );
        }
        // Use actual font measurement
        let (measured_width, measured_height) =
            graphics::measure_text_sized(&self.content, self.font_size);

        if crate::debug::is_enabled() {
            crate::logln!(
                "[TextRenderObject::layout] measured={}x{}",
                measured_width,
                measured_height
            );
        }

        // For text, use the measured size, but constrain within bounds
        // Text should NOT expand to fill min_width/min_height
        let mut width = measured_width as f32;
        let mut height = measured_height as f32;

        // Apply max constraints (don't exceed maximum)
        if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            width = width.min(constraints.max_width);
        }
        if constraints.max_height.is_finite() && constraints.max_height > 0.0 {
            height = height.min(constraints.max_height);
        }

        // Only apply min constraints if measured size is smaller
        // But for text, we generally don't want to expand it
        // So we'll just use the measured size even if it's smaller than min

        self.size = Size { width, height };

        // Create/update buffer for this text
        let w = libm::ceilf(width) as u32;
        let h = libm::ceilf(height) as u32;
        if crate::debug::is_enabled() {
            crate::logln!(
                "[TextRenderObject] layout: final size={}x{}, buffer needed={} bytes",
                w,
                h,
                w * h * 4
            );
        }

        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.logical_width() != w || b.logical_height() != h);
        if needs_resize {
            self.buffer = Some(Buffer::from_logical_dimensions(w, h));
        }

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

    fn render(&mut self) {
        // Render text to buffer
        if let Some(ref mut buffer) = self.buffer {
            let mut canvas = graphics::Canvas::for_buffer(buffer);
            let width = canvas.width();
            let height = canvas.height();

            if crate::debug::is_enabled() {
                crate::logln!(
                    "[TextRenderObject] render: buffer {}x{}, data.len()={}",
                    width,
                    height,
                    width.saturating_mul(height).saturating_mul(4)
                );
            }

            // Clear to avoid blending text on top of previous frames.
            canvas.fill_rect(0, 0, width, height, Color::TRANSPARENT);

            // Draw text
            canvas.draw_text_sized(0, 0, &self.content, self.color, self.font_size);
        } else {
            if crate::debug::is_enabled() {
                crate::logln!("[TextRenderObject] render: NO BUFFER!");
            }
        }
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
    }
}
