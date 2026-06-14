//! ProgressView View - Progress indicator
//!
//! ProgressView displays the progress of a long-running task.

use crate::buffer::Buffer;
use crate::color::{Color, ColorPalette};
use crate::element::{Element, ElementRenderObject, RenderElement};
use crate::geometry::Size;
use crate::graphics;
use crate::view::View;
use alloc::boxed::Box;
use core::any::Any;

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
        let w = width as u32;
        let h = height as u32;

        // Create or resize buffer
        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.logical_width() != w || b.logical_height() != h);
        if needs_resize {
            self.buffer = Some(Buffer::from_logical_dimensions(w, h));
        }

        if let Some(ref mut buffer) = self.buffer {
            let mut canvas = graphics::Canvas::for_buffer(buffer);
            let w = canvas.width();
            let h = canvas.height();

            let palette = ColorPalette::default();
            let bg_color = palette.surface_variant();
            let fill_color = palette.primary();

            // Draw background
            canvas.fill_rect(0, 0, w, h, bg_color);

            // Draw filled portion
            let fill_width = (self.value * width as f32) as u32;
            if fill_width > 0 {
                canvas.fill_rect(0, 0, fill_width, h, fill_color);
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
        self.draw_progress();
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
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
