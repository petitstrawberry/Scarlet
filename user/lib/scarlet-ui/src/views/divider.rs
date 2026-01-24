//! Divider View - Horizontal or vertical line for separating content
//!
//! Divider is a visual separator that can be horizontal or vertical.

use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::Size;
use crate::color::Color;
use crate::buffer::Buffer;
use crate::graphics;
use alloc::boxed::Box;

/// Divider View - displays a horizontal or vertical line
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum DividerOrientation {
    Horizontal,
    Vertical,
}

impl Default for DividerOrientation {
    fn default() -> Self {
        Self::Horizontal
    }
}

/// Divider View
#[derive(Clone)]
pub struct Divider {
    orientation: DividerOrientation,
    color: Color,
    thickness: f32,
}

impl Divider {
    /// Create a new horizontal Divider
    pub fn new() -> Self {
        Self {
            orientation: DividerOrientation::Horizontal,
            color: Color::rgba(0u8, 0u8, 0u8, 20u8), // Semi-transparent black (20/255 alpha)
            thickness: 1.0,
        }
    }

    /// Set orientation
    pub fn orientation(mut self, orientation: DividerOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Set color
    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    /// Set thickness
    pub fn thickness(mut self, thickness: f32) -> Self {
        self.thickness = thickness;
        self
    }

    /// Get orientation
    pub fn get_orientation(&self) -> DividerOrientation {
        self.orientation
    }

    /// Get color
    pub fn get_color(&self) -> Color {
        self.color
    }

    /// Get thickness
    pub fn get_thickness(&self) -> f32 {
        self.thickness
    }
}

impl Default for Divider {
    fn default() -> Self {
        Self::new()
    }
}

impl View for Divider {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            DividerRenderObject::new(self.orientation, self.color, self.thickness),
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        alloc::vec::Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Divider RenderObject
pub struct DividerRenderObject {
    orientation: DividerOrientation,
    color: Color,
    thickness: f32,
    size: Size,
    buffer: Option<Buffer>,
}

impl DividerRenderObject {
    /// Create a new DividerRenderObject
    pub fn new(orientation: DividerOrientation, color: Color, thickness: f32) -> Self {
        Self {
            orientation,
            color,
            thickness,
            size: Size::ZERO,
            buffer: None,
        }
    }

    /// Get orientation
    pub fn get_orientation(&self) -> DividerOrientation {
        self.orientation
    }

    /// Get color
    pub fn get_color(&self) -> Color {
        self.color
    }

    /// Get thickness
    pub fn get_thickness(&self) -> f32 {
        self.thickness
    }
}

impl ElementRenderObject for DividerRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        match self.orientation {
            DividerOrientation::Horizontal => {
                // Horizontal divider: fill width, use thickness for height
                let width = if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
                    constraints.max_width.max(constraints.min_width)
                } else {
                    constraints.min_width.max(1.0)
                };

                let height = self.thickness.max(constraints.min_height);

                self.size = Size { width, height };

                // Create buffer
                let w = libm::ceilf(width) as u32;
                let h = libm::ceilf(height) as u32;
                let needed = (w * h * 4) as usize;

                if self.buffer.as_ref().map_or(true, |b| b.data().len() < needed) {
                    self.buffer = Some(Buffer::from_dimensions(w, h));
                }
            }
            DividerOrientation::Vertical => {
                // Vertical divider: use thickness for width, fill height
                let width = self.thickness.max(constraints.min_width);

                let height = if constraints.max_height.is_finite() && constraints.max_height > 0.0 {
                    constraints.max_height.max(constraints.min_height)
                } else {
                    constraints.min_height.max(1.0)
                };

                self.size = Size { width, height };

                // Create buffer
                let w = libm::ceilf(width) as u32;
                let h = libm::ceilf(height) as u32;
                let needed = (w * h * 4) as usize;

                if self.buffer.as_ref().map_or(true, |b| b.data().len() < needed) {
                    self.buffer = Some(Buffer::from_dimensions(w, h));
                }
            }
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
        if let Some(ref mut buffer) = self.buffer {
            let width = buffer.width();
            let height = buffer.height();
            let mut data = buffer.data_mut();
            let mut canvas = graphics::Canvas::new(&mut data, width, height);

            // Fill with the divider color
            canvas.fill_rect(0, 0, width, height, self.color);
        }
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }
}
