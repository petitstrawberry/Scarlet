//! Repaint boundary for isolating view updates
//!
//! This module provides RepaintBoundary, which is a marker trait and view
//! that isolates repaints to a specific subtree. This is useful for:
//! - Animated content (isolates redraws to just the animating part)
//! - Frequent updates (prevents redrawing the entire screen)
//!
//! Views that implement this trait will be allocated their own buffer
//! and will be composited into their parent.

extern crate alloc;
use alloc::boxed::Box;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::Event;
use crate::graphics::Rect;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::render::RenderObject;
use crate::view::buffer::ViewBuffer;
use scarlet_std::fmt;

/// Repaint boundary view
///
/// RepaintBoundary wraps a child view and provides an isolated buffer
/// for rendering. This allows the child to be repainted independently
/// of its parent.
pub struct RepaintBoundary {
    /// Unique identifier for this view
    id: ViewId,
    /// Child view
    child: Box<dyn RenderObject>,
    /// Buffer for isolated rendering
    buffer: Option<ViewBuffer>,
    /// Whether this view is opaque (optimization hint)
    opaque: bool,
    /// Cached size from last layout
    cached_size: Size,
    /// Cached frame from last layout
    cached_frame: Option<Rect>,
    /// Layout constraints from last layout
    constraints: Option<LayoutConstraints>,
}

impl RepaintBoundary {
    /// Create a new repaint boundary
    pub fn new(child: Box<dyn RenderObject>) -> Self {
        let id = ViewId::new();
        Self {
            id,
            child,
            buffer: None,
            opaque: false, // Default to not opaque
            cached_size: Size::ZERO,
            cached_frame: None,
            constraints: None,
        }
    }

    /// Set whether this boundary is opaque
    ///
    /// Opaque boundaries can be optimized by skipping background drawing.
    pub fn with_opaque(mut self, opaque: bool) -> Self {
        self.opaque = opaque;
        self
    }

    /// Check if this boundary is opaque
    pub fn is_opaque(&self) -> bool {
        self.opaque
    }

    /// Get the child view
    pub fn child(&self) -> &dyn RenderObject {
        self.child.as_ref()
    }

    /// Get mutable access to the child view
    pub fn child_mut(&mut self) -> &mut dyn RenderObject {
        self.child.as_mut()
    }

    /// Get the buffer if it exists
    pub fn buffer(&self) -> Option<&ViewBuffer> {
        self.buffer.as_ref()
    }

    /// Get mutable access to the buffer
    pub fn buffer_mut(&mut self) -> Option<&mut ViewBuffer> {
        self.buffer.as_mut()
    }

    /// Ensure a buffer exists for this boundary
    ///
    /// Creates a buffer if one doesn't exist, or resizes the existing
    /// buffer if it's too small.
    pub fn ensure_buffer(&mut self) -> &mut ViewBuffer {
        let size = self.cached_size;

        if self.buffer.is_none() {
            self.buffer = Some(ViewBuffer::new(size));
        } else if let Some(buffer) = &self.buffer {
            if !buffer.can_fit(size) {
                self.buffer = Some(ViewBuffer::new(size));
            }
        }

        self.buffer.as_mut().unwrap()
    }

    /// Clear the buffer (if it exists)
    pub fn clear_buffer(&mut self) {
        if let Some(buffer) = &mut self.buffer {
            buffer.clear();
        }
    }

    /// Remove the buffer
    pub fn remove_buffer(&mut self) -> Option<ViewBuffer> {
        self.buffer.take()
    }
}

impl crate::view::render::RenderObject for RepaintBoundary {
    fn id(&self) -> ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        self.constraints = Some(constraints);

        // Layout the child
        let size = self.child.layout(ctx, constraints);
        self.cached_size = size;

        // Resize or create buffer if needed
        if self.buffer.is_some() {
            self.ensure_buffer();
        }

        size
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        // If we have a buffer, draw to it instead of directly to the canvas
        if let Some(buffer) = &self.buffer {
            // TODO: Draw child to buffer
            // This would require creating a sub-canvas or render target
            let _ = (buffer, frame, ctx);

            // Then composite the buffer to the main canvas
            // TODO: Implement blitting
        } else {
            // No buffer, draw child directly
            self.child.draw(ctx, frame);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        self.child.event(ctx, event)
    }

    fn update(&mut self, ctx: &mut UpdateCtx) {
        self.child.update(ctx)
    }
}

impl fmt::Debug for RepaintBoundary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RepaintBoundary")
            .field("id", &self.id)
            .field("child_id", &self.child.id())
            .field("has_buffer", &self.buffer.is_some())
            .field("opaque", &self.opaque)
            .field("cached_size", &self.cached_size)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestView {
        id: ViewId,
        size: Size,
    }

    impl TestView {
        fn new(size: Size) -> Self {
            Self {
                id: ViewId::new(),
                size,
            }
        }
    }

    impl RenderObject for TestView {
        fn id(&self) -> ViewId {
            self.id
        }

        fn layout(&mut self, _ctx: &mut LayoutCtx, _constraints: LayoutConstraints) -> Size {
            self.size
        }

        fn draw(&self, _ctx: &mut PaintCtx, _frame: Rect) {}

        fn event(&mut self, _ctx: &mut EventCtx, _event: &Event) -> ControlFlow {
            ControlFlow::Continue
        }

        fn update(&mut self, _ctx: &mut UpdateCtx) {}
    }

    #[test]
    fn test_repaint_boundary_new() {
        let child = Box::new(TestView::new(Size::new(100, 100)));
        let boundary = RepaintBoundary::new(child);

        assert!(!boundary.is_opaque());
        assert!(boundary.buffer().is_none());
    }

    #[test]
    fn test_repaint_boundary_with_opaque() {
        let child = Box::new(TestView::new(Size::new(100, 100)));
        let boundary = RepaintBoundary::new(child)
            .with_opaque(true);

        assert!(boundary.is_opaque());
    }

    #[test]
    fn test_repaint_boundary_ensure_buffer() {
        let child = Box::new(TestView::new(Size::new(100, 100)));
        let mut boundary = RepaintBoundary::new(child);

        // Initially no buffer
        assert!(boundary.buffer().is_none());

        // Ensure buffer creates one
        boundary.ensure_buffer();
        assert!(boundary.buffer().is_some());
        assert_eq!(boundary.buffer().unwrap().size(), Size::ZERO);

        // Update cached size and ensure buffer again
        boundary.cached_size = Size::new(50, 50);
        boundary.ensure_buffer();
        assert_eq!(boundary.buffer().unwrap().size(), Size::new(50, 50));
    }

    #[test]
    fn test_repaint_boundary_clear_buffer() {
        let child = Box::new(TestView::new(Size::new(100, 100)));
        let mut boundary = RepaintBoundary::new(child);

        boundary.ensure_buffer();
        boundary.buffer_mut().unwrap().fill([255, 0, 0, 255]);

        boundary.clear_buffer();
        assert!(boundary.buffer().unwrap().data().iter().all(|&x| x == 0));
    }

    #[test]
    fn test_repaint_boundary_remove_buffer() {
        let child = Box::new(TestView::new(Size::new(100, 100)));
        let mut boundary = RepaintBoundary::new(child);

        boundary.ensure_buffer();
        assert!(boundary.buffer().is_some());

        boundary.remove_buffer();
        assert!(boundary.buffer().is_none());
    }
}
