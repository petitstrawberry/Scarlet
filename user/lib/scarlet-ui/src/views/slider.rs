//! Slider View - Slider control for selecting a value from a range
//!
//! Slider is a control that allows selecting a value from a continuous range.

use alloc::boxed::Box;
use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::Size;
use crate::color::{Color, ColorPalette};
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
    dragging: bool,
}

impl SliderRenderObject {
    fn thumb_diameter(&self) -> f32 {
        self.size.height
    }

    fn track_metrics(&self) -> (f32, f32) {
        let thumb_diameter = self.thumb_diameter().max(1.0);
        let track_start = thumb_diameter / 2.0;
        let track_width = (self.size.width - thumb_diameter).max(1.0);
        (track_start, track_width)
    }

    fn fill_circle(
        canvas: &mut graphics::Canvas<'_>,
        center_x: i32,
        center_y: i32,
        radius: i32,
        color: Color,
    ) {
        if radius <= 0 {
            return;
        }
        let r_sq = radius * radius;
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy <= r_sq {
                    canvas.put_pixel(center_x + dx, center_y + dy, color);
                }
            }
        }
    }

    /// Create a new SliderRenderObject
    pub fn new(value: f32, min: f32, max: f32) -> Self {
        Self {
            value: value.clamp(min, max),
            min,
            max,
            size: Size::new(200.0, 20.0),
            buffer: None,
            dragging: false,
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

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    pub fn set_dragging(&mut self, dragging: bool) {
        self.dragging = dragging;
    }

    pub fn value_from_local_x(&self, local_x: f32) -> f32 {
        let (track_start, track_width) = self.track_metrics();
        let normalized = ((local_x - track_start) / track_width).clamp(0.0, 1.0);
        self.min + (self.max - self.min) * normalized
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
        let w = width as u32;
        let h = height as u32;
        let (track_start, track_width) = self.track_metrics();
        let thumb_diameter = self.thumb_diameter().max(1.0) as u32;

        // Create or resize buffer
        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.width() != w || b.height() != h);
        if needs_resize {
            self.buffer = Some(Buffer::from_dimensions(w, h));
        }

        if let Some(ref mut buffer) = self.buffer {
            let mut canvas = graphics::Canvas::new(buffer.data_mut(), w, h);

            let center_y = (height as f32 / 2.0) as i32;

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
            let fill_width = (normalized_value * track_width) as u32;

            let palette = ColorPalette::default();
            let track_color = palette.surface_variant();
            let fill_color = palette.primary();
            let thumb_color = palette.surface();
            let thumb_border = palette.border();

            // Draw track (background)
            canvas.fill_rect(
                track_start as i32,
                track_y,
                track_width as u32,
                track_thickness,
                track_color,
            );

            // Draw filled portion
            if fill_width > 0 {
                canvas.fill_rect(
                    track_start as i32,
                    track_y,
                    fill_width,
                    track_thickness,
                    fill_color,
                );
            }

            // Draw thumb
            let thumb_x = track_start as i32 + fill_width as i32;
            let thumb_y = center_y - (thumb_diameter as i32 / 2);
            let radius = thumb_diameter as i32 / 2;
            let center_x = thumb_x;
            let center_y = thumb_y + radius;

            Self::fill_circle(&mut canvas, center_x, center_y, radius, thumb_border);
            Self::fill_circle(&mut canvas, center_x, center_y, radius - 1, thumb_color);
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
        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.width() != w || b.height() != h);
        if needs_resize {
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

    fn clear_buffer(&mut self) {
        self.buffer = None;
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
