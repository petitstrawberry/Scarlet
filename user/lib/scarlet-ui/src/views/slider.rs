//! Slider View - Slider control for selecting a value from a range
//!
//! Slider is a control that allows selecting a value from a continuous range.

use alloc::boxed::Box;
use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::Size;
use crate::color::Color;
use crate::buffer::Buffer;
use crate::graphics;
use crate::state::State;
use alloc::vec::Vec;

/// Slider View
#[derive(Clone)]
pub struct Slider {
    value: State<f32>,
    min: f32,
    max: f32,
}

impl Slider {
    /// Create a new Slider
    pub fn new(value: State<f32>) -> Self {
        Self {
            value,
            min: 0.0,
            max: 1.0,
        }
    }

    /// Set minimum value
    pub fn min(mut self, min: f32) -> Self {
        self.min = min;
        self
    }

    /// Set maximum value
    pub fn max(mut self, max: f32) -> Self {
        self.max = max;
        self
    }

    /// Get value state
    pub fn get_value(&self) -> &State<f32> {
        &self.value
    }

    /// Get min
    pub fn get_min(&self) -> f32 {
        self.min
    }

    /// Get max
    pub fn get_max(&self) -> f32 {
        self.max
    }
}

impl View for Slider {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            SliderRenderObject::new(self.value.get(), self.min, self.max),
        ))
    }

    fn listenables(&self) -> Vec<&dyn crate::state::Listenable> {
        alloc::vec![&self.value]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Slider RenderObject
///
/// Design matching macOS/iOS slider:
/// - Height: 20px (track is 4px thick)
/// - Width: flexible (at least 100px)
/// - Track: Light gray (#C5C5C7)
/// - Fill: Blue (#007AFF) for filled portion
/// - Thumb: White circle, 20px diameter with shadow
pub struct SliderRenderObject {
    value: f32,
    min: f32,
    max: f32,
    size: Size,
    buffer: Option<Buffer>,
}

impl SliderRenderObject {
    /// Create a new SliderRenderObject
    pub fn new(value: f32, min: f32, max: f32) -> Self {
        Self {
            value: value.clamp(min, max),
            min,
            max,
            size: Size::new(200.0, 20.0),
            buffer: None,
        }
    }

    /// Get value
    pub fn get_value(&self) -> f32 {
        self.value
    }

    /// Set value
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(self.min, self.max);
    }

    /// Get min
    pub fn get_min(&self) -> f32 {
        self.min
    }

    /// Get max
    pub fn get_max(&self) -> f32 {
        self.max
    }

    /// Draw slider using Canvas API (macOS/iOS-style design)
    fn draw_slider(&mut self) {
        let width = libm::ceilf(self.size.width) as usize;
        let height = libm::ceilf(self.size.height) as usize;
        let needed = width * height;

        // Create or resize buffer
        if self.buffer.as_ref().map_or(true, |b| b.as_slice().len() < needed) {
            self.buffer = Some(Buffer::from_dimensions(width as u32, height as u32));
        }

        if let Some(ref mut buffer) = self.buffer {
            let mut canvas = graphics::Canvas::new(buffer.data_mut(), width as u32, height as u32);

            let center_y = (height / 2) as i32;

            // Track dimensions
            let track_thickness = 4u32;
            let track_y = center_y - (track_thickness as i32 / 2);

            // Calculate fill width based on value
            let range = self.max - self.min;
            let normalized_value = if range > 0.0 {
                (self.value - self.min) / range
            } else {
                0.0
            };
            let fill_width = (normalized_value * (width as f32 - 20.0)) as u32; // 20px for thumb

            // Draw track (light gray background)
            let track_color = Color::rgb(197u8, 197u8, 199u8); // macOS gray: #C5C5C7
            canvas.fill_rect(
                10, // Start 10px from left
                track_y,
                width as u32 - 20, // End 10px from right
                track_thickness,
                track_color,
            );

            // Draw filled portion (blue)
            let fill_color = Color::rgb(0u8, 122u8, 255u8); // iOS blue: #007AFF
            if fill_width > 0 {
                canvas.fill_rect(
                    10,
                    track_y,
                    fill_width,
                    track_thickness,
                    fill_color,
                );
            }

            // Draw thumb (white circle)
            let thumb_x = (10i32 + fill_width as i32);
            let thumb_diameter = 20u32;
            let thumb_y = center_y - (thumb_diameter as i32 / 2);

            // Draw thumb (for now, just a square)
            let thumb_color = Color::WHITE;
            let radius = thumb_diameter as i32 / 2;
            let thumb_rect_x = thumb_x - radius;
            let thumb_rect_y = thumb_y - radius;
            canvas.fill_rect(thumb_rect_x, thumb_rect_y, thumb_diameter, thumb_diameter, thumb_color);
        }
    }
}

impl ElementRenderObject for SliderRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        // Slider has fixed height (20px), flexible width
        let width = if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            constraints.max_width.max(constraints.min_width).max(100.0) // Min 100px width
        } else {
            constraints.min_width.max(200.0)
        };

        let height = self.size.height; // Fixed height: 20px

        self.size = Size { width, height };

        // Create buffer
        let w = libm::ceilf(width) as u32;
        let h = libm::ceilf(height) as u32;
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
        self.draw_slider();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn update(&mut self, new_view: &dyn crate::view::View) -> crate::element::UpdateResult {
        if let Some(slider) = new_view.as_any().downcast_ref::<Slider>() {
            let new_value = slider.value.get().clamp(self.min, self.max);
            if (self.value - new_value).abs() > 0.001 {
                self.value = new_value;
                crate::element::UpdateResult::Updated
            } else {
                crate::element::UpdateResult::NoChange
            }
        } else {
            crate::element::UpdateResult::Replaced
        }
    }
}
