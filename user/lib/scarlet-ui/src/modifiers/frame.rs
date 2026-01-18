//! Frame modifier
//!
//! This module provides the Frame modifier, which sets a fixed size for a view.

extern crate alloc;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::Event;
use crate::graphics::Rect;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::traits::View;
use scarlet_std::fmt;

/// Frame modifier
///
/// Sets a fixed or constrained size for a child view.
pub struct Frame<T> {
    /// Child view
    child: T,
    /// Minimum width
    min_width: u32,
    /// Maximum width
    max_width: u32,
    /// Minimum height
    min_height: u32,
    /// Maximum height
    max_height: u32,
    /// Cached size from last layout
    cached_size: Size,
    /// Cached frame from last layout
    cached_frame: Option<Rect>,
    /// Layout constraints from last layout
    constraints: Option<LayoutConstraints>,
}

impl<T> Frame<T> {
    /// Create a new frame modifier with a fixed size
    pub fn new(child: T, width: u32, height: u32) -> Self {
        Self {
            child,
            min_width: width,
            max_width: width,
            min_height: height,
            max_height: height,
            cached_size: Size::ZERO,
            cached_frame: None,
            constraints: None,
        }
    }

    /// Create a new frame modifier with constraints
    pub fn with_constraints(
        child: T,
        min_width: u32,
        max_width: u32,
        min_height: u32,
        max_height: u32,
    ) -> Self {
        Self {
            child,
            min_width,
            max_width,
            min_height,
            max_height,
            cached_size: Size::ZERO,
            cached_frame: None,
            constraints: None,
        }
    }

    /// Get the child view
    pub fn child(&self) -> &T {
        &self.child
    }

    /// Get mutable access to the child view
    pub fn child_mut(&mut self) -> &mut T {
        &mut self.child
    }

    /// Get the width constraints
    pub fn width_range(&self) -> (u32, u32) {
        (self.min_width, self.max_width)
    }

    /// Get the height constraints
    pub fn height_range(&self) -> (u32, u32) {
        (self.min_height, self.max_height)
    }
}

impl<T: View> View for Frame<T> {
    fn id(&self) -> ViewId {
        self.child.id()
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        self.constraints = Some(constraints);

        // Intersect parent constraints with frame constraints
        let min_width = self.min_width.max(constraints.min_width);
        let max_width = self.max_width.min(constraints.max_width);
        let min_height = self.min_height.max(constraints.min_height);
        let max_height = self.max_height.min(constraints.max_height);

        // Ensure min <= max
        let max_width = max_width.max(min_width);
        let max_height = max_height.max(min_height);

        // Create tight constraints for the child
        let child_constraints = LayoutConstraints::new(
            min_width,
            max_width,
            min_height,
            max_height,
        );

        // Layout the child
        let child_size = self.child.layout(ctx, child_constraints);

        self.cached_size = child_size;
        self.cached_size
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        // Child should fill the frame
        self.child.draw(ctx, frame)
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        self.child.event(ctx, event)
    }

    fn update(&mut self, ctx: &mut UpdateCtx) {
        self.child.update(ctx)
    }
}

impl<T: fmt::Debug> fmt::Debug for Frame<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("child", &self.child)
            .field("min_width", &self.min_width)
            .field("max_width", &self.max_width)
            .field("min_height", &self.min_height)
            .field("max_height", &self.max_height)
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

    impl View for TestView {
        fn id(&self) -> ViewId {
            self.id
        }

        fn layout(&mut self, _ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
            // Return the size constrained by parent
            Size::new(
                constraints.min_width.max(0),
                constraints.min_height.max(0),
            )
        }

        fn draw(&self, _ctx: &mut PaintCtx, _frame: Rect) {}

        fn event(&mut self, _ctx: &mut EventCtx, _event: &Event) -> ControlFlow {
            ControlFlow::Continue
        }

        fn update(&mut self, _ctx: &mut UpdateCtx) {}
    }

    #[test]
    fn test_frame_new() {
        let child = TestView::new(Size::new(50, 50));
        let frame = Frame::new(child, 100, 100);

        assert_eq!(frame.min_width, 100);
        assert_eq!(frame.max_width, 100);
        assert_eq!(frame.min_height, 100);
        assert_eq!(frame.max_height, 100);
    }

    #[test]
    fn test_frame_with_constraints() {
        let child = TestView::new(Size::new(50, 50));
        let frame = Frame::with_constraints(child, 50, 150, 75, 125);

        assert_eq!(frame.min_width, 50);
        assert_eq!(frame.max_width, 150);
        assert_eq!(frame.min_height, 75);
        assert_eq!(frame.max_height, 125);
    }

    #[test]
    fn test_frame_width_height_range() {
        let child = TestView::new(Size::new(50, 50));
        let frame = Frame::with_constraints(child, 50, 150, 75, 125);

        let (min_w, max_w) = frame.width_range();
        assert_eq!(min_w, 50);
        assert_eq!(max_w, 150);

        let (min_h, max_h) = frame.height_range();
        assert_eq!(min_h, 75);
        assert_eq!(max_h, 125);
    }
}
