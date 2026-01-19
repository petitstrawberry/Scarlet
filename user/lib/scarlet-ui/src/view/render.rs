//! RenderObject trait - Actual rendering and event handling
//!
//! This trait provides the actual rendering, event handling, and layout
//! implementation for views. The View trait is now a marker trait that
//! only provides structural information, while RenderObject does the real work.

extern crate alloc;
use alloc::boxed::Box;
use core::any::Any;

use crate::context::{ControlFlow, EventCtx, LayoutCtx, PaintCtx, UpdateCtx};
use crate::event::Event;
use crate::layout::{LayoutConstraints, Size};
use crate::view::id::ViewId;
use crate::graphics::Rect;

/// RenderObject trait - actual rendering and event handling
///
/// This trait is implemented by all views that need to be rendered.
/// It provides the actual implementation of layout, drawing, and event handling.
///
/// # Design
///
/// - **View**: Marker trait for declarative UI structure
/// - **RenderObject**: Actual rendering implementation
///
/// # Event Handling (Flutter-style)
///
/// System events (mouse click)
///     ↓
/// RenderObject::event() receives
///     ↓
/// Calls View properties (action, on_click)
pub trait RenderObject {
    /// Get the unique identifier for this render object
    fn id(&self) -> ViewId;

    /// Get `Any` reference for downcasting
    fn as_any(&self) -> &dyn Any;

    /// Calculate the desired size given constraints
    ///
    /// Called during layout phase. Should return the size this view wants to be,
    /// constrained by the given constraints.
    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size;

    /// Draw the view within the given frame
    ///
    /// Called during paint phase. Should render itself into the provided canvas.
    fn draw(&self, ctx: &mut PaintCtx, frame: Rect);

    /// Handle an event
    ///
    /// Called during event phase. Should handle the event if relevant.
    /// Returns whether the event was consumed.
    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        let _ = (ctx, event);
        ControlFlow::Continue
    }

    /// Update the view when observed data changes
    ///
    /// Called when data that this view observes has changed.
    fn update(&mut self, ctx: &mut UpdateCtx) {
        let _ = ctx;
        // Default: do nothing
    }
}
