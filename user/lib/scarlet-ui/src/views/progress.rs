//! ProgressView View - Progress indicator
//!
//! ProgressView displays the progress of a long-running task.

use core::any::Any;
use crate::view::View;
use crate::element::{Element, RenderElement, ElementRenderObject};
use crate::geometry::Size;
use crate::color::Color;
use crate::buffer::Buffer;
use crate::graphics;
use alloc::boxed::Box;

/// ProgressView View - shows progress (0.0 to 1.0)
#[derive(Clone)]
pub struct ProgressView {
    value: f32,
}

impl ProgressView {
    /// Create a new ProgressView
    pub fn new(value: f32) -> Self {
        Self {
            value: value.clamp(0.0, 1.0),
        }
    }

    /// Set progress value (0.0 to 1.0)
    pub fn value(mut self, value: f32) -> Self {
        self.value = value.clamp(0.0, 1.0);
        self
    }

    /// Get value
    pub fn get_value(&self) -> f32 {
        self.value
    }
}

impl View for ProgressView {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::new(
            self.clone(),
            ProgressViewRenderObject::new(self.value),
        ))
    }

    fn listenables(&self) -> alloc::vec::Vec<&dyn crate::state::Listenable> {
        alloc::vec::Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// ProgressView RenderObject
///
/// Design matching macOS/iOS progress bar:
/// - Height: 4px (determinate), 20px (indeterminate)
/// - Width: flexible (at least 100px)
/// - Background: Light gray (#E5E5EA)
/// - Fill: Blue (#007AFF)
/// - Fully rounded
pub struct ProgressViewRenderObject {
    value: f32,
    size: Size,
    buffer: Option<Buffer>,
}

impl ProgressViewRenderObject {
    /// Create a new ProgressViewRenderObject
    pub fn new(value: f32) -> Self {
        Self {
            value: value.clamp(0.0, 1.0),
            size: Size::new(200.0, 4.0),
            buffer: None,
        }
    }

    /// Get value
    pub fn get_value(&self) -> f32 {
        self.value
    }

    /// Set value
    pub fn set_value(&mut self, value: f32) {
        self.value = value.clamp(0.0, 1.0);
    }

    /// Draw progress bar using Canvas API (macOS/iOS-style design)
    fn draw_progress(&mut self) {
        let width = libm::ceilf(self.size.width) as usize;
        let height = libm::ceilf(self.size.height) as usize;
        let needed = width * height;

        // Create or resize buffer
        if self.buffer.as_ref().map_or(true, |b| b.as_slice().len() < needed) {
            self.buffer = Some(Buffer::from_dimensions(width as u32, height as u32));
        }

        if let Some(ref mut buffer) = self.buffer {
            let mut canvas = graphics::Canvas::new(buffer.data_mut(), width as u32, height as u32);

            // Draw background (light gray)
            let bg_color = Color::rgb(229u8, 229u8, 234u8); // iOS gray: #E5E5EA
            canvas.fill_rect(0, 0, width as u32, height as u32, bg_color);

            // Draw filled portion (blue)
            let fill_color = Color::rgb(0u8, 122u8, 255u8); // iOS blue: #007AFF
            let fill_width = (self.value * width as f32) as u32;
            if fill_width > 0 {
                canvas.fill_rect(0, 0, fill_width, height as u32, fill_color);
            }
        }
    }
}

impl ElementRenderObject for ProgressViewRenderObject {
    fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        // ProgressView has fixed height (4px), flexible width
        let width = if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            constraints.max_width.max(constraints.min_width).max(100.0) // Min 100px width
        } else {
            constraints.min_width.max(200.0)
        };

        let height = self.size.height; // Fixed height: 4px

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
        self.draw_progress();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn update(&mut self, new_view: &dyn crate::view::View) -> crate::element::UpdateResult {
        if let Some(progress) = new_view.as_any().downcast_ref::<ProgressView>() {
            let new_value = progress.value.clamp(0.0, 1.0);
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
