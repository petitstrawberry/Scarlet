//! Canvas View - A view that renders to a user-provided buffer callback
//!
//! This allows embedding any rendering system (Slint, custom graphics, etc.) into Scarlet UI.

use crate::buffer::Buffer;
use crate::element::{Element, ElementRenderObject, LayoutConstraints};
use crate::event::Event;
use crate::geometry::Size;
use crate::view::View;

use core::any::Any;
use std::boxed::Box;
use std::cell::RefCell;
use std::rc::Rc;

/// Callback type for canvas rendering
///
/// The callback receives:
/// - buffer: mutable slice of BGRA pixels (u8, 4 bytes per pixel)
/// - width: buffer width in pixels
/// - height: buffer height in pixels
///
/// # Example
///
/// ```rust
/// use std::rc::Rc;
/// use scarlet_ui_core::CanvasView;
///
/// let view = CanvasView::new(800.0, 600.0, Rc::new(|buffer: &mut [u8], width, height| {
///     // Draw to buffer (BGRA format)
///     for y in 0..height {
///         for x in 0..width {
///             let idx = ((y * width + x) * 4) as usize;
///             buffer[idx] = 255;     // B
///             buffer[idx + 1] = 0;   // G
///             buffer[idx + 2] = 0;   // R
///             buffer[idx + 3] = 255; // A
///         }
///     }
/// }));
/// ```
pub type CanvasRenderCallback = Rc<dyn Fn(&mut [u8], u32, u32)>;

/// Event handler type for canvas
pub type CanvasEventHandler = Rc<RefCell<dyn FnMut(&Event) -> bool>>;

/// A View that renders to a user-provided buffer via callback
#[derive(Clone)]
pub struct CanvasView {
    size: Size,
    render_callback: CanvasRenderCallback,
    event_handler: Option<CanvasEventHandler>,
}

impl CanvasView {
    /// Create a new CanvasView with the given size and render callback
    pub fn new(width: f32, height: f32, render_callback: CanvasRenderCallback) -> Self {
        Self {
            size: Size { width, height },
            render_callback,
            event_handler: None,
        }
    }

    /// Set the event handler callback
    ///
    /// The handler returns true if the event was consumed
    pub fn on_event<F>(mut self, handler: F) -> Self
    where
        F: FnMut(&Event) -> bool + 'static,
    {
        self.event_handler = Some(Rc::new(RefCell::new(handler)));
        self
    }

    /// Get the current size
    pub fn size(&self) -> Size {
        self.size
    }

    /// Set a new size
    pub fn set_size(&mut self, width: f32, height: f32) {
        self.size = Size { width, height };
    }

    /// Internal: get the render callback
    pub(crate) fn render_callback(&self) -> &CanvasRenderCallback {
        &self.render_callback
    }

    /// Internal: get the event handler
    pub(crate) fn event_handler(&self) -> Option<&CanvasEventHandler> {
        self.event_handler.as_ref()
    }
}

impl View for CanvasView {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(crate::element::RenderElement::new(
            self.clone(),
            CanvasRenderObject::new(
                self.size,
                self.render_callback.clone(),
                self.event_handler.clone(),
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

/// RenderObject for CanvasView
pub struct CanvasRenderObject {
    size: Size,
    render_callback: CanvasRenderCallback,
    event_handler: Option<CanvasEventHandler>,
    buffer: Option<Buffer>,
}

impl CanvasRenderObject {
    fn new(
        size: Size,
        render_callback: CanvasRenderCallback,
        event_handler: Option<CanvasEventHandler>,
    ) -> Self {
        Self {
            size,
            render_callback,
            event_handler,
            buffer: None,
        }
    }

    /// Get the event handler
    pub fn event_handler(&self) -> Option<&CanvasEventHandler> {
        self.event_handler.as_ref()
    }
}

impl ElementRenderObject for CanvasRenderObject {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Apply constraints
        let width = if constraints.max_width.is_finite() && constraints.max_width > 0.0 {
            constraints.max_width.max(constraints.min_width)
        } else {
            self.size.width.max(constraints.min_width)
        };

        let height = if constraints.max_height.is_finite() && constraints.max_height > 0.0 {
            constraints.max_height.max(constraints.min_height)
        } else {
            self.size.height.max(constraints.min_height)
        };

        let size = Size { width, height };
        self.size = size;

        // Create/update buffer
        let w = libm::ceilf(width) as u32;
        let h = libm::ceilf(height) as u32;
        let needs_resize = self
            .buffer
            .as_ref()
            .map_or(true, |b| b.logical_width() != w || b.logical_height() != h);
        if needs_resize {
            self.buffer = Some(Buffer::from_logical_dimensions(w, h));
        }

        size
    }

    fn size(&self) -> Size {
        self.size
    }

    fn render(&mut self) {
        if let Some(ref mut buffer) = self.buffer {
            let width = buffer.width();
            let height = buffer.height();
            let mut data = buffer.data_mut();

            // Call the user's render callback
            (self.render_callback)(&mut data, width, height);
        }
    }

    fn get_buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    fn clear_buffer(&mut self) {
        self.buffer = None;
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
