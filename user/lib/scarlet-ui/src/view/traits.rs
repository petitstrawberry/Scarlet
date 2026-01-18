//! Core View trait and related types
//!
//! This module defines the View trait, which is the foundation of the view system.
//! Views participate in layout, drawing, and event handling.

extern crate alloc;
use alloc::boxed::Box;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::Event;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;

/// Core trait for all views
///
/// The View trait defines the interface that all UI components must implement.
/// Views participate in three main phases:
///
/// 1. **Event handling** (`event`): Respond to user input
/// 2. **Layout** (`layout`): Calculate size and position
/// 3. **Drawing** (`draw`): Render the view
///
/// This design is inspired by Druid's Widget trait and provides a clean
/// separation of concerns.
///
/// # Lifecycle
///
/// Views are created, participate in layout/draw cycles, and are eventually
/// dropped. The framework manages the view lifecycle.
///
/// # Example
///
/// ```ignore
/// struct MyView {
///     id: ViewId,
///     text: &'static str,
/// }
///
/// impl MyView {
///     fn new(text: &'static str) -> Self {
///         Self {
///             id: ViewId::new(),
///             text,
///         }
///     }
/// }
///
/// impl View for MyView {
///     fn id(&self) -> ViewId {
///         self.id
///     }
///
///     fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
///         // Return the size this view wants to be
///         Size::new(100, 20)
///     }
///
///     fn draw(&self, ctx: &mut PaintCtx, frame: crate::graphics::Rect) {
///         // Draw the view
///         // ctx.canvas.draw_text(frame.x, frame.y, self.text, Color::BLACK);
///     }
///
///     fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
///         // Handle events
///         ControlFlow::Continue
///     }
///
///     fn update(&mut self, ctx: &mut UpdateCtx) {
///         // Called when observed data changes
///     }
/// }
/// ```
pub trait View {
    /// Get the unique identifier for this view
    ///
    /// Each view instance must have a unique ViewId that is assigned
    /// when the view is created and never changes.
    fn id(&self) -> ViewId;

    /// Calculate the desired size given constraints
    ///
    /// This is called during the layout phase. The view should return
    /// the size it wants to be, constrained by the given constraints.
    ///
    /// The framework may call this multiple times with different constraints
    /// during a single layout pass, so views should cache calculations
    /// when possible.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Layout context for utilities and information
    /// * `constraints` - Size constraints (min/max width and height)
    ///
    /// # Returns
    ///
    /// The size this view wants to be, constrained by `constraints`.
    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size;

    /// Draw the view within the given frame
    ///
    /// This is called during the paint phase. The view should render itself
    /// into the provided canvas.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Paint context with canvas and drawing utilities
    /// * `frame` - The rectangle this view should draw within
    fn draw(&self, ctx: &mut PaintCtx, frame: crate::graphics::Rect);

    /// Handle an event
    ///
    /// This is called during the event phase. The view should handle the
    /// event if it's relevant, and return whether it consumed the event.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Event context for requesting layout/paint
    /// * `event` - The event to handle
    ///
    /// # Returns
    ///
    /// - `ControlFlow::Continue` - Continue propagating the event
    /// - `ControlFlow::Stop` - Stop propagation (event was consumed)
    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        ControlFlow::Continue
    }

    /// Update the view when observed data changes
    ///
    /// This is called when data that this view observes has changed.
    /// Views can use this to update their internal state or request redraws.
    ///
    /// # Arguments
    ///
    /// * `ctx` - Update context for requesting layout/paint
    fn update(&mut self, ctx: &mut UpdateCtx) {
        let _ = ctx;
        // Default: do nothing
    }

    /// Get child views (for containers)
    ///
    /// Container views should override this to return their children.
    /// Leaf views should return an empty slice (default).
    ///
    /// This is used for event propagation and layout.
    fn children(&self) -> &[ViewChild] {
        &[]
    }

    /// Get mutable child views (for containers)
    ///
    /// Container views should override this to return mutable references
    /// to their children. Leaf views should return an empty slice (default).
    fn children_mut(&mut self) -> &mut [ViewChild] {
        &mut []
    }
}

/// A child view with its frame
///
/// This is used by container views to track their children and their
/// positions within the container.
pub struct ViewChild {
    /// The child view
    pub view: Box<dyn View>,
    /// The frame (position and size) allocated to this child
    pub frame: crate::graphics::Rect,
}

impl ViewChild {
    /// Create a new ViewChild
    pub fn new(view: Box<dyn View>, frame: crate::graphics::Rect) -> Self {
        Self { view, frame }
    }
}

/// Marker trait for views that can contain other views
///
/// Container views should implement this trait to advertise that they
/// have children. This allows the framework to optimize event handling
/// and layout.
pub trait Container: View {
    /// Add a child view
    ///
    /// The container is responsible for positioning the child appropriately.
    fn add_child(&mut self, child: Box<dyn View>);

    /// Remove all children
    fn clear_children(&mut self);
}

/// Marker trait for views that are opaque
///
/// Opaque views draw an opaque background, which allows the framework
/// to skip drawing the background behind them. This is an important
/// optimization for performance.
///
/// # Example
///
/// A solid color rectangle is opaque:
/// ```ignore
/// impl Opaque for ColorRect {
///     fn is_opaque(&self) -> bool {
///         self.color.a == 255 // Fully opaque
///     }
/// }
/// ```
pub trait Opaque {
    /// Check if this view is opaque
    ///
    /// Returns `true` if the view draws an opaque background (no transparency).
    fn is_opaque(&self) -> bool;
}

/// Marker trait for views that form a repaint boundary
///
/// Repaint boundaries are views that should be isolated into their own
/// rendering layer. This is useful for:
/// - Animated content (isolates redraws to just the animating part)
/// - Frequent updates (prevents redrawing the entire screen)
///
/// Views that implement this trait will be allocated their own buffer
/// and will be composited into their parent.
pub trait RepaintBoundary {
    /// Check if this view should form a repaint boundary
    ///
    /// Returns `true` if this view should have its own buffer.
    fn is_repaint_boundary(&self) -> bool;
}

/// Marker trait for views that form a layout boundary
///
/// Layout boundaries prevent layout changes from propagating beyond
/// this view. This is useful for performance when:
/// - A view's children change size frequently
/// - The parent doesn't need to know about child size changes
pub trait LayoutBoundary {
    /// Check if this view should form a layout boundary
    ///
    /// Returns `true` if layout changes shouldn't propagate past this view.
    fn is_layout_boundary(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestView {
        id: ViewId,
    }

    impl View for TestView {
        fn id(&self) -> ViewId {
            self.id
        }

        fn layout(&mut self, _ctx: &mut LayoutCtx, _constraints: LayoutConstraints) -> Size {
            Size::new(100, 100)
        }

        fn draw(&self, _ctx: &mut PaintCtx, _frame: crate::graphics::Rect) {
            // Do nothing for test
        }
    }

    #[test]
    fn test_view_id() {
        let view = TestView { id: ViewId::new() };
        let _id = view.id();
    }
}
