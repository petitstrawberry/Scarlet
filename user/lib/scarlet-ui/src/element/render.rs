//! RenderElement - wraps RenderObjects for leaf elements
//!
//! RenderElement represents leaf nodes in the element tree that directly
//! render content (text, rectangles, images, etc.).

use alloc::boxed::Box;
use core::any::Any;

use crate::element::{Element, ElementId, LayoutConstraints};
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
}

/// Element that wraps a RenderObject
///
/// RenderElement is a leaf element that directly delegates to a RenderObject.
pub struct RenderElement<R: RenderObject> {
    id: ElementId,
    render_object: R,
    position: Point,
}

impl<R: RenderObject> RenderElement<R> {
    /// Create a new RenderElement with a RenderObject
    pub fn new(render_object: R) -> Self {
        Self {
            id: ElementId::generate(),
            render_object,
            position: Point::ZERO,
        }
    }

    /// Get the RenderObject
    pub fn render_object(&self) -> &R {
        &self.render_object
    }

    /// Get mutable reference to the RenderObject
    pub fn render_object_mut(&mut self) -> &mut R {
        &mut self.render_object
    }
}

impl<R: RenderObject> Element for RenderElement<R> {
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
        &[]
    }

    fn children_mut(&mut self) -> &mut [Box<dyn Element>] {
        &mut []
    }

    fn rebuild(&mut self, _new_view: &dyn View) -> bool {
        // RenderElements don't rebuild from Views (they're updated via property setters)
        false
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

    fn handle_event(&mut self, _event: &crate::event::Event) -> bool {
        // RenderObjects don't handle events by default
        false
    }
}
