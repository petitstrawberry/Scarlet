//! Rectangle View - Displays a filled rectangle
//!
//! Rectangle is a basic shape primitive that fills its frame with a solid color.

use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::Size;
use crate::color::Color;
use crate::buffer::Buffer;
use crate::graphics;
use alloc::boxed::Box;

/// Rectangle View - displays a filled rectangle
#[derive(Clone)]
pub struct Rectangle {
    color: Color,
    corner_radius: f32,
}

impl Rectangle {
    /// Create a new Rectangle filled with the given color
    pub fn new() -> Self {
        Self {
            color: Color::BLACK,
            corner_radius: 0.0,
        }
    }

    /// Set the fill color
    pub fn fill(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Set the corner radius
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Get the fill color
    pub fn get_color(&self) -> Color {
        self.color
    }

    /// Get the corner radius
    pub fn get_corner_radius(&self) -> f32 {
        self.corner_radius
    }
}

impl Default for Rectangle {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Rectangle {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            RectangleRenderObject::new(self.color, self.corner_radius),
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        alloc::vec::Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Rectangle RenderObject - handles rectangle rendering
pub struct RectangleRenderObject {
    color: Color,
    corner_radius: f32,
    size: Size,
    buffer: Option<Buffer>,
}

impl RectangleRenderObject {
    /// Create a new RectangleRenderObject
    pub fn new(color: Color, corner_radius: f32) -> Self {
        Self {
            color,
            corner_radius,
            size: Size::ZERO,
            buffer: None,
        }
    }

    /// Get the fill color
    pub fn get_color(&self) -> Color {
        self.color
    }

    /// Get the corner radius
    pub fn get_corner_radius(&self) -> f32 {
        self.corner_radius
    }
}

impl ElementRenderObject for RectangleRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        scarlet_std::println!("[RectangleRenderObject::layout] START: constraints=({:?}, {:?}) -> ({:?}, {:?})",
            constraints.min_width, constraints.min_height, constraints.max_width, constraints.max_height);
        // Rectangle takes the full available space, or min_size if specified
        // For inf constraints, use min_width/min_height
        let width = if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            constraints.max_width.max(constraints.min_width)
        } else {
            constraints.min_width.max(1.0)
        };

        let height = if constraints.max_height.is_finite() && constraints.max_height > 0.0 {
            constraints.max_height.max(constraints.min_height)
        } else {
            constraints.min_height.max(1.0)
        };

        self.size = Size { width, height };
        scarlet_std::println!("[RectangleRenderObject::layout] calculated size={}x{}", width, height);

        // Create buffer for this rectangle
        let w = libm::ceilf(width) as u32;
        let h = libm::ceilf(height) as u32;

        // Sanity check to prevent overflow
        if w > 10000 || h > 10000 {
            scarlet_std::println!("[RectangleRenderObject] layout: WARNING calculated size {}x{} is too large, using min constraints",
                w, h);
            // Use min constraints as fallback
            let w2 = libm::ceilf(constraints.min_width.max(1.0)) as u32;
            let h2 = libm::ceilf(constraints.min_height.max(1.0)) as u32;
            if self.buffer.as_ref().map_or(true, |b| b.data().len() < (w2 * h2 * 4) as usize) {
                self.buffer = Some(Buffer::from_dimensions(w2, h2));
            }
            self.size = Size { width: constraints.min_width.max(1.0), height: constraints.min_height.max(1.0) };
            return self.size;
        }

        let needed = (w * h * 4) as usize;

        if self.buffer.as_ref().map_or(true, |b| b.data().len() < needed) {
            self.buffer = Some(Buffer::from_dimensions(w, h));
        }

        self.size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn render(&mut self) {
        // Render rectangle to buffer
        scarlet_std::println!("[RectangleRenderObject] render START: color={:?}, buffer={}",
            self.color, self.buffer.is_some());
        if let Some(ref mut buffer) = self.buffer {
            let width = buffer.width();
            let height = buffer.height();
            scarlet_std::println!("[RectangleRenderObject] buffer {}x{}", width, height);
            let mut data = buffer.data_mut();
            scarlet_std::println!("[RectangleRenderObject] creating canvas...");
            let mut canvas = graphics::Canvas::new(&mut data, width, height);
            scarlet_std::println!("[RectangleRenderObject] filling rect...");

            // Fill with solid color
            canvas.fill_rect(0, 0, width, height, self.color);
            scarlet_std::println!("[RectangleRenderObject] render DONE");
        }
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }
}
