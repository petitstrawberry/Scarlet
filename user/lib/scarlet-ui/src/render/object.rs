//! RenderObject trait - core rendering abstraction
//!
//! RenderObjects are responsible for layout, rendering to buffers,
//! and managing their frame (position and size).

use alloc::boxed::Box;
use core::any::Any;
use crate::buffer::Buffer;
use crate::geometry::{Point, Rect, Size};
use crate::element::{ElementId, LayoutConstraints, UpdateResult, DirtyFlags};
use crate::view::View;

/// RenderObject trait for leaf and container rendering nodes
///
/// RenderObjects are responsible for:
/// - Computing layout within constraints
/// - Rendering to a buffer (leaf nodes only)
/// - Managing children (container nodes only)
/// - Hit testing
pub trait RenderObject: Any {
    /// Layout this RenderObject and return its size
    fn layout(&mut self, constraints: LayoutConstraints) -> Size;

    /// Render to buffer
    ///
    /// For leaf nodes, this renders content to the buffer.
    /// For container nodes, this typically just propagates render to children.
    fn render(&mut self);

    /// Get the current frame (position and size)
    fn frame(&self) -> Rect;

    /// Set the frame (position and size)
    fn set_frame(&mut self, frame: Rect);

    /// Get the buffer (for leaf nodes)
    ///
    /// Container nodes return None.
    fn get_buffer(&self) -> Option<&Buffer>;

    /// Get the buffer mutably (for leaf nodes)
    ///
    /// Container nodes return None.
    fn get_buffer_mut(&mut self) -> Option<&mut Buffer>;

    /// Get child RenderObjects (for container nodes)
    ///
    /// Leaf nodes return an empty slice.
    fn children(&self) -> &[Box<dyn RenderObject>];

    /// Get mutable child RenderObjects (for container nodes)
    ///
    /// Leaf nodes return an empty slice.
    fn children_mut(&mut self) -> &mut [Box<dyn RenderObject>];

    /// Hit test - check if a point is within this RenderObject
    fn hit_test(&self, point: Point) -> bool {
        self.frame().contains(point)
    }

    /// Get the opacity for compositing (0.0 - 1.0)
    ///
    /// Default is 1.0 (fully opaque)
    fn opacity(&self) -> f32 {
        1.0
    }

    /// Get as Any for downcasting
    fn as_any(&self) -> &dyn Any;

    /// Get as Any mut for downcasting
    fn as_any_mut(&mut self) -> &mut dyn Any;

    // ===== Identity methods =====

    /// Get the unique Element ID of this RenderObject
    fn id(&self) -> ElementId;

    /// Get the parent Element ID (if any)
    fn parent(&self) -> Option<ElementId>;

    /// Set the parent Element ID
    fn set_parent(&mut self, parent: ElementId);

    // ===== Update methods =====

    /// Update this RenderObject from a new View
    ///
    /// This is called when the View has changed and the RenderObject
    /// should update its properties to match.
    ///
    /// Returns UpdateResult indicating success, replacement needed, or failure.
    fn update(&mut self, new_view: &dyn View) -> UpdateResult;

    // ===== Dirty tracking methods =====

    /// Mark this RenderObject as dirty with the given flags
    fn mark_dirty(&mut self, flags: DirtyFlags);

    /// Check if this RenderObject needs re-rendering
    fn is_dirty(&self) -> bool {
        false
    }

    /// Clear the dirty flag
    fn clear_dirty(&mut self) {}
}
