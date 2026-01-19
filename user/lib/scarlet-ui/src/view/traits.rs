//! Core View trait and related types
//!
//! This module defines the View trait, which is a marker trait for views.
//! The actual rendering implementation is in the RenderObject trait.

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;

use crate::context::UpdateCtx;
use crate::layout::Size;
use crate::view::id::ViewId;
use crate::view::render::RenderObject;

/// View marker trait
///
/// This is a marker trait that all views implement. The actual rendering
/// implementation is in the RenderObject trait.
///
/// # Design
///
/// - **View**: Marker trait for declarative UI structure
///   - `children()`, `children_mut()` - Child view structure
///   - Property setters (`action()`, `text()`, `color()` etc.)
///
/// - **RenderObject**: Actual rendering implementation
///   - `layout()` - Size calculation
///   - `draw()` - Drawing
///   - `event()` - System event handling
///   - `update()` - Periodic updates
///
/// # Blanket Implementation
///
/// All types that implement RenderObject automatically get View implementation:
///
/// ```ignore
/// impl<T> View for T where T: RenderObject {}
/// ```
pub trait View: RenderObject {
    /// Get child views (for containers)
    ///
    /// Container views should override this to return their children.
    /// Leaf views should return an empty slice (default).
    ///
    /// This is used for event propagation and layout.
    fn children(&self) -> &[ChildView] {
        &[]
    }

    /// Get mutable child views (for containers)
    ///
    /// Container views should override this to return mutable references
    /// to their children. Leaf views should return an empty slice (default).
    fn children_mut(&mut self) -> &mut [ChildView] {
        &mut []
    }
}

/// Blanket implementation: all RenderObjects are Views
impl<T> View for T where T: RenderObject {}

/// A child view with its frame
///
/// This is used by container views to track their children and their
/// positions within the container.
pub struct ChildView {
    /// The child view (render object)
    pub view: Box<dyn RenderObject>,
    /// The frame (position and size) allocated to this child
    pub frame: crate::graphics::Rect,
}

impl ChildView {
    /// Create a new ChildView
    pub fn new(view: Box<dyn RenderObject>, frame: crate::graphics::Rect) -> Self {
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
    fn add_child(&mut self, child: Box<dyn RenderObject>);

    /// Remove all children
    fn clear_children(&mut self);
}

/// Marker trait for views that are opaque
///
/// Opaque views are guaranteed to draw every pixel within their frame,
/// allowing the framework to skip drawing views behind them.
pub trait Opaque: View {}

/// Marker trait for views that have a stable identity
///
/// Views implementing this trait maintain their identity across
/// rebuilds, allowing the framework to preserve state.
pub trait Identifiable: View {
    /// Get the stable identifier for this view
    fn stable_id(&self) -> Option<ViewId> {
        None
    }
}
