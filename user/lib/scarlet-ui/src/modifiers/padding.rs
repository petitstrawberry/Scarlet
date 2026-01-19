//! Padding modifier
//!
//! This module provides the Padding modifier, which adds padding around a view.

extern crate alloc;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::Event;
use crate::graphics::Rect;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::view::View;
use scarlet_std::fmt;

/// Padding modifier
///
/// Adds padding (empty space) around a child view.
pub struct Padding<T> {
    /// Child view
    child: T,
    /// Top padding
    top: u32,
    /// Right padding
    right: u32,
    /// Bottom padding
    bottom: u32,
    /// Left padding
    left: u32,
    /// Cached size from last layout
    cached_size: Size,
    /// Cached frame from last layout
    cached_frame: Option<Rect>,
    /// Layout constraints from last layout
    constraints: Option<LayoutConstraints>,
}

impl<T> Padding<T> {
    /// Create a new padding modifier with uniform padding
    pub fn new(child: T, padding: u32) -> Self {
        Self {
            child,
            top: padding,
            right: padding,
            bottom: padding,
            left: padding,
            cached_size: Size::ZERO,
            cached_frame: None,
            constraints: None,
        }
    }

    /// Create a new padding modifier with different padding for each edge
    pub fn with_insets(child: T, top: u32, right: u32, bottom: u32, left: u32) -> Self {
        Self {
            child,
            top,
            right,
            bottom,
            left,
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

    /// Get the total horizontal padding
    pub fn horizontal_padding(&self) -> u32 {
        self.left + self.right
    }

    /// Get the total vertical padding
    pub fn vertical_padding(&self) -> u32 {
        self.top + self.bottom
    }

    /// Calculate the child's frame within the padded frame
    pub fn child_frame(&self, frame: Rect) -> Rect {
        Rect::new(
            frame.x + self.left as i32,
            frame.y + self.top as i32,
            (frame.width as u32).saturating_sub(self.horizontal_padding()),
            (frame.height as u32).saturating_sub(self.vertical_padding()),
        )
    }
}

impl<T: crate::view::render::RenderObject + 'static> crate::view::render::RenderObject for Padding<T> {
    fn id(&self) -> ViewId {
        self.child.id()
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        self.constraints = Some(constraints);

        // Calculate child constraints (subtract padding)
        let horizontal_padding = self.horizontal_padding();
        let vertical_padding = self.vertical_padding();

        let child_min_width = constraints.min_width.saturating_sub(horizontal_padding);
        let child_max_width = constraints.max_width.saturating_sub(horizontal_padding);
        let child_min_height = constraints.min_height.saturating_sub(vertical_padding);
        let child_max_height = constraints.max_height.saturating_sub(vertical_padding);

        // Ensure min <= max
        let child_max_width = child_max_width.max(child_min_width);
        let child_max_height = child_max_height.max(child_min_height);

        let child_constraints = LayoutConstraints::new(
            child_min_width,
            child_max_width,
            child_min_height,
            child_max_height,
        );

        // Layout the child
        let child_size = self.child.layout(ctx, child_constraints);

        // Add padding to get total size
        let width = child_size.width.saturating_add(horizontal_padding);
        let height = child_size.height.saturating_add(vertical_padding);

        self.cached_size = Size::new(width, height);
        self.cached_size
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: Rect) {
        // Calculate child frame and draw
        let child_frame = self.child_frame(frame);

        // Only draw if child frame is valid
        if child_frame.width > 0 && child_frame.height > 0 {
            self.child.draw(ctx, child_frame);
        }
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        self.child.event(ctx, event)
    }

    fn update(&mut self, ctx: &mut UpdateCtx) {
        self.child.update(ctx)
    }
}

impl<T: crate::view::render::RenderObject + 'static> View for Padding<T> {
    // as_any, id, layout, draw, event, update are inherited from RenderObject impl
}

impl<T: fmt::Debug> fmt::Debug for Padding<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Padding")
            .field("child", &self.child)
            .field("top", &self.top)
            .field("right", &self.right)
            .field("bottom", &self.bottom)
            .field("left", &self.left)
            .field("cached_size", &self.cached_size)
            .finish()
    }
}
