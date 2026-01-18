//! View modifiers - SwiftUI-style view extensions
//!
//! This module provides view modifiers that allow chaining view transformations
//! in a SwiftUI-like syntax. Modifiers are implemented using the ViewExt trait.

extern crate alloc;
use alloc::boxed::Box;

pub mod padding;
pub mod frame;
pub mod background;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::Event;
use crate::graphics::Rect;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::traits::View;
use crate::view::boundary::RepaintBoundary;
use crate::color::Color;

pub use padding::Padding;
pub use frame::Frame;
pub use background::Background;

/// Extension trait providing SwiftUI-style view modifiers
///
/// This trait is implemented for all types that implement View,
/// providing a fluent API for view modification.
pub trait ViewExt: View + Sized where Self: 'static {
    /// Add padding to this view
    ///
    /// # Example
    ///
    /// ```ignore
    /// let padded = view.padding(10);
    /// ```
    fn padding(self, padding: u32) -> Padding<Self> {
        Padding::new(self, padding)
    }

    /// Add different padding for each edge
    ///
    /// # Example
    ///
    /// ```ignore
    /// let padded = view.padding_insets(10, 20, 10, 20);
    /// ```
    fn padding_insets(self, top: u32, right: u32, bottom: u32, left: u32) -> Padding<Self> {
        Padding::with_insets(self, top, right, bottom, left)
    }

    /// Set the frame size for this view
    ///
    /// # Example
    ///
    /// ```ignore
    /// let framed = view.frame(100, 100);
    /// ```
    fn frame(self, width: u32, height: u32) -> Frame<Self> {
        Frame::new(self, width, height)
    }

    /// Set the frame with constraints
    ///
    /// # Example
    ///
    /// ```ignore
    /// let framed = view.frame_constraints(100..=200, 50..=100);
    /// ```
    fn frame_constraints(
        self,
        min_width: u32,
        max_width: u32,
        min_height: u32,
        max_height: u32,
    ) -> Frame<Self> {
        Frame::with_constraints(self, min_width, max_width, min_height, max_height)
    }

    /// Set the background color
    ///
    /// # Example
    ///
    /// ```ignore
    /// let colored = view.background(Color::rgb(255, 0, 0));
    /// ```
    fn background(self, color: Color) -> Background<Self> {
        Background::new(self, color)
    }

    /// Wrap this view in a repaint boundary
    ///
    /// This isolates repaints to this view, preventing the parent
    /// from being repainted when this view changes.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let isolated = animated_view.repaint_boundary();
    /// ```
    fn repaint_boundary(self) -> RepaintBoundaryWrapper<Self> {
        RepaintBoundaryWrapper::new(self)
    }

    /// Wrap this view in an opaque repaint boundary
    ///
    /// This is an optimization hint that tells the framework this view
    /// is opaque, allowing the parent background to be skipped.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let isolated = solid_view.repaint_boundary_opaque();
    /// ```
    fn repaint_boundary_opaque(self) -> RepaintBoundaryWrapper<Self> {
        RepaintBoundaryWrapper::new_opaque(self)
    }
}

impl<T: View + 'static> ViewExt for T {}

/// Wrapper for RepaintBoundary that implements View
pub struct RepaintBoundaryWrapper<T: View> {
    inner: RepaintBoundary,
    _phantom: core::marker::PhantomData<T>,
}

impl<T: View + 'static> RepaintBoundaryWrapper<T> {
    pub fn new(child: T) -> Self {
        let inner = RepaintBoundary::new(Box::new(child));
        Self {
            inner,
            _phantom: core::marker::PhantomData,
        }
    }

    pub fn new_opaque(child: T) -> Self {
        let inner = RepaintBoundary::new(Box::new(child)).with_opaque(true);
        Self {
            inner,
            _phantom: core::marker::PhantomData,
        }
    }
}

impl<T: View + 'static> View for RepaintBoundaryWrapper<T> {
    fn id(&self) -> ViewId {
        self.inner.id()
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        self.inner.layout(ctx, constraints)
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        self.inner.draw(ctx, frame)
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        self.inner.event(ctx, event)
    }

    fn update(&mut self, ctx: &mut UpdateCtx) {
        self.inner.update(ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestView {
        id: ViewId,
    }

    impl TestView {
        fn new() -> Self {
            Self {
                id: ViewId::new(),
            }
        }
    }

    impl View for TestView {
        fn id(&self) -> ViewId {
            self.id
        }

        fn layout(&mut self, _ctx: &mut LayoutCtx, _constraints: LayoutConstraints) -> Size {
            Size::new(100, 100)
        }

        fn draw(&self, _ctx: &mut PaintCtx, _frame: Rect) {}

        fn event(&mut self, _ctx: &mut EventCtx, _event: &Event) -> ControlFlow {
            ControlFlow::Continue
        }

        fn update(&mut self, _ctx: &mut UpdateCtx) {}
    }

    #[test]
    fn test_view_ext_padding() {
        let view = TestView::new();
        let _padded = view.padding(10);
    }

    #[test]
    fn test_view_ext_frame() {
        let view = TestView::new();
        let _framed = view.frame(100, 100);
    }

    #[test]
    fn test_view_ext_background() {
        let view = TestView::new();
        let _colored = view.background(Color::rgb(255, 0, 0));
    }

    #[test]
    fn test_view_ext_repaint_boundary() {
        let view = TestView::new();
        let _isolated = view.repaint_boundary();
    }
}
