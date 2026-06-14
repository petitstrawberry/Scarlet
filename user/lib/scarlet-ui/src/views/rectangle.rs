//! Rectangle View - Displays a filled rectangle
//!
//! Rectangle is a basic shape primitive that fills its frame with a solid color.

use crate::buffer::Buffer;
use crate::color::Color;
use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::Size;
use crate::graphics;
use crate::view::View;
use alloc::boxed::Box;
use core::any::Any;

/// Rectangle View - displays a filled rectangle
#[derive(Clone)]
pub struct Rectangle {
    color: Color,
    corner_radius: f32,
    border_width: f32,
    border_color: Option<Color>,
}

impl Rectangle {
    /// Create a new Rectangle filled with the given color
    pub fn new() -> Self {
        Self {
            color: Color::BLACK,
            corner_radius: 0.0,
            border_width: 0.0,
            border_color: None,
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

    /// Set the border width and color
    pub fn border(mut self, width: f32, color: Color) -> Self {
        self.border_width = width;
        self.border_color = Some(color);
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
            RectangleRenderObject::new(
                self.color,
                self.corner_radius,
                self.border_width,
                self.border_color,
            ),
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
    border_width: f32,
    border_color: Option<Color>,
    size: Size,
    buffer: Option<Buffer>,
}

impl RectangleRenderObject {
    /// Create a new RectangleRenderObject
    pub fn new(
        color: Color,
        corner_radius: f32,
        border_width: f32,
        border_color: Option<Color>,
    ) -> Self {
        Self {
            color,
            corner_radius,
            border_width,
            border_color,
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

    fn fill_rounded_rect(
        canvas: &mut graphics::Canvas<'_>,
        x0: i32,
        y0: i32,
        width: u32,
        height: u32,
        radius: f32,
        color: Color,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let max_radius = (width.min(height) as f32) / 2.0;
        let radius = radius.max(0.0).min(max_radius);
        if radius <= 0.0 {
            canvas.fill_rect(x0, y0, width, height, color);
            return;
        }

        let left = radius;
        let right = width as f32 - radius;
        let top = radius;
        let bottom = height as f32 - radius;
        let radius_sq = radius * radius;

        for y in 0..height as i32 {
            let fy = y as f32;
            for x in 0..width as i32 {
                let fx = x as f32;
                let in_center = (fx >= left && fx < right) || (fy >= top && fy < bottom);
                if !in_center {
                    let corner_x = if fx < left { left } else { right };
                    let corner_y = if fy < top { top } else { bottom };
                    let dx = fx - corner_x;
                    let dy = fy - corner_y;
                    if dx * dx + dy * dy > radius_sq {
                        continue;
                    }
                }
                canvas.put_pixel(x + x0, y + y0, color);
            }
        }
    }
}

impl ElementRenderObject for RectangleRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        if crate::debug::is_enabled() {
            crate::logln!(
                "[RectangleRenderObject::layout] START: constraints=({:?}, {:?}) -> ({:?}, {:?})",
                constraints.min_width,
                constraints.min_height,
                constraints.max_width,
                constraints.max_height
            );
        }
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
        if crate::debug::is_enabled() {
            crate::logln!(
                "[RectangleRenderObject::layout] calculated size={}x{}",
                width,
                height
            );
        }

        // Create buffer for this rectangle
        let w = libm::ceilf(width) as u32;
        let h = libm::ceilf(height) as u32;

        // Sanity check to prevent overflow
        if w > 10000 || h > 10000 {
            if crate::debug::is_enabled() {
                crate::logln!(
                    "[RectangleRenderObject] layout: WARNING calculated size {}x{} is too large, using min constraints",
                    w,
                    h
                );
            }
            // Use min constraints as fallback
            let w2 = libm::ceilf(constraints.min_width.max(1.0)) as u32;
            let h2 = libm::ceilf(constraints.min_height.max(1.0)) as u32;
            let needs_resize = self.buffer.as_ref().map_or(true, |b| {
                b.logical_width() != w2 || b.logical_height() != h2
            });
            if needs_resize {
                self.buffer = Some(Buffer::from_logical_dimensions(w2, h2));
            }
            self.size = Size {
                width: constraints.min_width.max(1.0),
                height: constraints.min_height.max(1.0),
            };
            return self.size;
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

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn render(&mut self) {
        // Render rectangle to buffer
        if crate::debug::is_enabled() {
            crate::logln!(
                "[RectangleRenderObject] render START: color={:?}, buffer={}",
                self.color,
                self.buffer.is_some()
            );
        }
        if let Some(ref mut buffer) = self.buffer {
            let mut canvas = graphics::Canvas::for_buffer(buffer);
            let width = canvas.width();
            let height = canvas.height();
            if crate::debug::is_enabled() {
                crate::logln!("[RectangleRenderObject] buffer {}x{}", width, height);
            }
            if crate::debug::is_enabled() {
                crate::logln!("[RectangleRenderObject] creating canvas...");
            }
            if crate::debug::is_enabled() {
                crate::logln!("[RectangleRenderObject] filling rect...");
            }

            if self.border_width > 0.0 {
                if let Some(border_color) = self.border_color {
                    let bw = self
                        .border_width
                        .max(1.0)
                        .min((width.min(height) as f32) / 2.0);
                    Self::fill_rounded_rect(
                        &mut canvas,
                        0,
                        0,
                        width,
                        height,
                        self.corner_radius,
                        border_color,
                    );
                    let inner_width = (width as f32 - 2.0 * bw).max(0.0) as u32;
                    let inner_height = (height as f32 - 2.0 * bw).max(0.0) as u32;
                    if inner_width > 0 && inner_height > 0 {
                        let inner_radius = (self.corner_radius - bw).max(0.0);
                        Self::fill_rounded_rect(
                            &mut canvas,
                            bw as i32,
                            bw as i32,
                            inner_width,
                            inner_height,
                            inner_radius,
                            self.color,
                        );
                    }
                } else {
                    Self::fill_rounded_rect(
                        &mut canvas,
                        0,
                        0,
                        width,
                        height,
                        self.corner_radius,
                        self.color,
                    );
                }
            } else {
                Self::fill_rounded_rect(
                    &mut canvas,
                    0,
                    0,
                    width,
                    height,
                    self.corner_radius,
                    self.color,
                );
            }

            if crate::debug::is_enabled() {
                crate::logln!("[RectangleRenderObject] render DONE");
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
