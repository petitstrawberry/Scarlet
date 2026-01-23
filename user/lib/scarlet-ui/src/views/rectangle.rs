//! Rectangle View - Displays a filled rectangle
//!
//! Rectangle is a basic shape primitive that fills its frame with a solid color.

use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::Size;
use crate::color::Color;
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
}

impl RectangleRenderObject {
    /// Create a new RectangleRenderObject
    pub fn new(color: Color, corner_radius: f32) -> Self {
        Self {
            color,
            corner_radius,
            size: Size::ZERO,
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
        // Rectangle takes the full available space, or min_size if specified
        let width = if constraints.max_width > 0.0 {
            constraints.max_width.max(constraints.min_width)
        } else {
            constraints.min_width.max(1.0)
        };

        let height = if constraints.max_height > 0.0 {
            constraints.max_height.max(constraints.min_height)
        } else {
            constraints.min_height.max(1.0)
        };

        self.size = Size { width, height };
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
}
