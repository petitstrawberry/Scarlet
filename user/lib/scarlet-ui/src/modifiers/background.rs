//! Background modifier
//!
//! This module provides the Background modifier, which sets a background color for a view.

extern crate alloc;

use crate::color::Color;
use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::Event;
use crate::graphics::Rect;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::View;
use scarlet_std::fmt;

/// Background modifier
///
/// Sets a background color that is drawn before the child view.
pub struct Background<T> {
    /// Child view
    child: T,
    /// Background color
    color: Color,
    /// Cached size from last layout
    cached_size: Size,
    /// Cached frame from last layout
    cached_frame: Option<Rect>,
    /// Layout constraints from last layout
    constraints: Option<LayoutConstraints>,
}

impl<T> Background<T> {
    /// Create a new background modifier
    pub fn new(child: T, color: Color) -> Self {
        Self {
            child,
            color,
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

    /// Get the background color
    pub fn color(&self) -> Color {
        self.color
    }

    /// Set a new background color
    pub fn set_color(&mut self, color: Color) {
        self.color = color;
    }
}

impl<T: crate::view::render::RenderObject + 'static> crate::view::render::RenderObject for Background<T> {
    fn id(&self) -> crate::view::id::ViewId {
        self.child.id()
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        self.constraints = Some(constraints);

        // Layout the child
        let size = self.child.layout(ctx, constraints);
        self.cached_size = size;
        size
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        // Draw background first
        // TODO: Implement actual background drawing
        let _ = (frame, self.color);

        // Then draw the child
        self.child.draw(ctx, frame)
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        self.child.event(ctx, event)
    }

    fn update(&mut self, ctx: &mut UpdateCtx) {
        self.child.update(ctx)
    }
}

impl<T: crate::view::render::RenderObject + 'static> View for Background<T> {
    // as_any, id, layout, draw, event, update are inherited from RenderObject impl
}

impl<T: fmt::Debug> fmt::Debug for Background<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Background")
            .field("child", &self.child)
            .field("color", &self.color)
            .field("cached_size", &self.cached_size)
            .finish()
    }
}
