//! Core View trait and related types

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::view::render::RenderObject;
use crate::view::id::ViewId;

/// View trait - main UI abstraction
///
/// View extends RenderObject. Provides structure (body, children)
/// while inheriting behavior (layout, draw, event, update) from RenderObject.
pub trait View: RenderObject {
    // ===== Structure =====

    /// Body content - child view definition
    fn body(&self) -> Option<&dyn View> {
        None
    }

    /// Get child views (for containers)
    fn children(&self) -> &[ChildView] {
        &[]
    }

    /// Get mutable child views (for containers)
    fn children_mut(&mut self) -> &mut [ChildView] {
        &mut []
    }
}

/// A child view with its frame
///
/// This is used by container views to track their children and their
/// positions within the container.
pub struct ChildView {
    /// The child view
    pub view: Box<dyn View>,
    /// The frame (position and size) allocated to this child
    pub frame: crate::graphics::Rect,
}

impl ChildView {
    /// Create a new ChildView
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
