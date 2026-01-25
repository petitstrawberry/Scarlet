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

    /// Render to buffer
    ///
    /// For leaf nodes, this renders content to the buffer.
    fn render(&mut self);

    /// Get the buffer (for compositing)
    ///
    /// Returns the buffer if this RenderObject has rendered content.
    /// Returns None for container nodes.
    fn get_buffer(&self) -> Option<&crate::buffer::Buffer> {
        None
    }

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

    fn type_name(&self) -> &str {
        "RenderElement"
    }

    fn type_name_debug(&self) -> alloc::string::String {
        alloc::format!("RenderElement<{}, {}>",
            core::any::type_name::<V>(),
            core::any::type_name::<R>()
        )
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

    fn flex_factor(&self) -> u32 {
        // Check if this is a Spacer (which should expand to fill available space)
        let type_name = core::any::type_name_of_val(&self.render_object);
        if type_name.contains("SpacerRenderObject") {
            1
        } else {
            0
        }
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        if let Some(frame) = self
            .render_object
            .as_any_mut()
            .downcast_mut::<crate::views::FrameRenderObject>()
        {
            let (min_w, max_w) = match frame.width_value() {
                Some(w) if w.is_finite() => (w, w),
                Some(_) if constraints.max_width.is_finite() => (constraints.max_width, constraints.max_width),
                Some(_) => (constraints.min_width, constraints.max_width),
                None => (constraints.min_width, constraints.max_width),
            };
            let (min_h, max_h) = match frame.height_value() {
                Some(h) if h.is_finite() => (h, h),
                Some(_) if constraints.max_height.is_finite() => (constraints.max_height, constraints.max_height),
                Some(_) => (constraints.min_height, constraints.max_height),
                None => (constraints.min_height, constraints.max_height),
            };

            let child_constraints = LayoutConstraints {
                min_width: min_w,
                max_width: max_w,
                min_height: min_h,
                max_height: max_h,
            };

            let mut child_max = Size::ZERO;
            for child in &mut self.children {
                let child_size = child.layout(child_constraints);
                child_max.width = child_max.width.max(child_size.width);
                child_max.height = child_max.height.max(child_size.height);
                child.set_position(crate::geometry::Point::ZERO);
            }

            return frame.layout_with_child(child_max, constraints);
        }

        // Layout the render_object
        let type_name = core::any::type_name_of_val(&self.render_object);
        scarlet_std::println!("[RenderElement::layout] START: type_name={}, constraints=({:?}, {:?}) -> ({:?}, {:?})",
            type_name, constraints.min_width, constraints.min_height, constraints.max_width, constraints.max_height);
        let size = self.render_object.layout(constraints);
        scarlet_std::println!("[RenderElement::layout] render_object returned size={}x{}", size.width, size.height);

        // Layout children with tight constraints
        // Exception: if render_object size is INF, pass parent constraints to allow children to expand
        for child in &mut self.children {
            let child_constraints = if size.width.is_infinite() || size.height.is_infinite() {
                scarlet_std::println!("[RenderElement::layout] render_object has INF size, using parent constraints");
                constraints
            } else {
                LayoutConstraints::tight(size.width, size.height)
            };
            scarlet_std::println!("[RenderElement::layout] child_constraints=({:?}, {:?}) -> ({:?}, {:?})",
                child_constraints.min_width, child_constraints.min_height, child_constraints.max_width, child_constraints.max_height);
            child.layout(child_constraints);
            child.set_position(crate::geometry::Point::ZERO);
        }

        size
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

    fn render(&mut self) {
        // Render this element first
        self.render_object.render();

        // Render children
        for child in &mut self.children {
            child.render();
        }

        // TODO: Composite child buffers into parent buffer if parent has a buffer
        // This is currently handled by specialized Elements like WindowRenderElement
    }

    fn get_buffer(&self) -> Option<&crate::buffer::Buffer> {
        self.render_object.get_buffer()
    }

    fn render_object(&self) -> Option<&dyn RenderObject> {
        Some(&self.render_object)
    }

    fn render_object_mut(&mut self) -> Option<&mut dyn RenderObject> {
        Some(&mut self.render_object)
    }

    fn handle_event(&mut self, _event: &crate::event::Event, _phase: crate::event::Phase) -> bool {
        // RenderObjects don't handle events by default
        false
    }
}
