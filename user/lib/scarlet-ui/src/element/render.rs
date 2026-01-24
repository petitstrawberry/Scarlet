//! RenderElement - wraps RenderObjects for leaf elements
//!
//! RenderElement represents leaf nodes in the element tree that directly
//! render content (text, rectangles, images, etc.).

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;

use crate::element::{Element, ElementId, LayoutConstraints, UpdateResult};
use crate::geometry::{Point, Rect, Size};
use crate::view::View;

/// RenderObject trait for leaf rendering nodes
///
/// RenderObjects are responsible for:
/// - Computing layout within constraints
/// - Rendering to a buffer
/// - Hit testing
pub trait RenderObject: Any {
    /// Layout this RenderObject and return its size
    fn layout(&mut self, constraints: LayoutConstraints) -> Size;

    /// Get the current size
    fn size(&self) -> Size;

    /// Hit test - check if a point is within this RenderObject
    fn hit_test(&self, point: Point) -> bool {
        let bounds = Rect {
            origin: Point::ZERO,
            size: self.size(),
        };
        bounds.contains(point)
    }

    /// Get as Any for downcasting
    fn as_any(&self) -> &dyn Any;

    /// Get as Any mut for downcasting
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Update this RenderObject from a new View
    ///
    /// This is called when the View has changed and the RenderObject
    /// should update its properties to match.
    ///
    /// Returns UpdateResult indicating whether the RenderObject was updated,
    /// needs replacement, or has no changes.
    ///
    /// Default implementation returns Replaced (requires full rebuild).
    /// Implementations should override this to provide efficient updates.
    fn update(&mut self, _new_view: &dyn View) -> UpdateResult {
        // Default: cannot update, need to replace
        UpdateResult::Replaced
    }
}

/// Element that wraps a RenderObject
///
/// RenderElement holds both a View and its corresponding RenderObject,
/// enabling reconciliation during updates.
///
/// # Type Parameters
/// * `V` - The View type that created this element (must be Clone)
/// * `R` - The RenderObject type that handles rendering
pub struct RenderElement<V: View + Clone, R: RenderObject> {
    id: ElementId,
    view: V,
    render_object: R,
    children: Vec<Box<dyn Element>>,
    position: Point,
}

impl<V: View + Clone, R: RenderObject> RenderElement<V, R> {
    /// Create a new RenderElement with a View and RenderObject
    pub fn new(view: V, render_object: R) -> Self {
        Self {
            id: ElementId::generate(),
            view,
            render_object,
            children: Vec::new(),
            position: Point::ZERO,
        }
    }

    /// Create a new RenderElement with a View, RenderObject, and children
    pub fn with_children(view: V, render_object: R, children: Vec<Box<dyn Element>>) -> Self {
        Self {
            id: ElementId::generate(),
            view,
            render_object,
            children,
            position: Point::ZERO,
        }
    }

    /// Get the View
    pub fn view(&self) -> &V {
        &self.view
    }

    /// Get mutable reference to the View
    pub fn view_mut(&mut self) -> &mut V {
        &mut self.view
    }

    /// Get the RenderObject
    pub fn render_object(&self) -> &R {
        &self.render_object
    }

    /// Get mutable reference to the RenderObject
    pub fn render_object_mut(&mut self) -> &mut R {
        &mut self.render_object
    }

    /// Add a child element
    pub fn add_child(&mut self, child: Box<dyn Element>) {
        self.children.push(child);
    }
}

impl<V: View + Clone, R: RenderObject> Element for RenderElement<V, R> {
    fn id(&self) -> ElementId {
        self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn children(&self) -> &[Box<dyn Element>] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Box<dyn Element>] {
        &mut self.children
    }

    fn update(&mut self, new_view: &dyn View) -> UpdateResult {
        // Try to downcast the new_view to the same type as our stored view
        if let Some(typed_view) = new_view.as_any().downcast_ref::<V>() {
            // Update the stored view (clone from the reference)
            self.view = typed_view.clone();
            // Delegate to the RenderObject's update method
            self.render_object.update(new_view)
        } else {
            // Type mismatch - need to replace
            UpdateResult::Replaced
        }
    }

    fn rebuild(&mut self) -> UpdateResult {
        // RenderElement doesn't need to rebuild since properties are
        // updated directly through the update() method.
        // The stored view remains the same, and changes happen through
        // State updates triggering update() calls.
        UpdateResult::NoChange
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.render_object.layout(constraints)
    }

    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, position: Point) {
        self.position = position;
    }

    fn bounds(&self) -> Rect {
        Rect {
            origin: self.position,
            size: self.render_object.size(),
        }
    }

    fn hit_test(&self, point: Point) -> bool {
        // Translate point to local coordinates
        let local_point = Point {
            x: point.x - self.position.x,
            y: point.y - self.position.y,
        };
        self.render_object.hit_test(local_point)
    }

    fn handle_event(&mut self, _event: &crate::event::Event, _phase: crate::event::Phase) -> bool {
        // RenderObjects don't handle events by default
        false
    }
}
