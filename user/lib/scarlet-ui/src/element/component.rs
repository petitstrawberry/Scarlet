//! ComponentElement - wraps Views and manages their lifecycle
//!
//! ComponentElement is the bridge between Views (immutable descriptions) and
//! the element tree (mutable runtime objects).

use alloc::boxed::Box;
use core::any::Any;

use crate::element::{Element, ElementId, LayoutConstraints};
use crate::geometry::{Point, Size};
use crate::view::View;

/// Element that wraps a View and manages its lifecycle
///
/// ComponentElement is responsible for:
/// - Owning the View instance
/// - Tracking State subscriptions for rebuilds
/// - Managing child Elements created by the View
pub struct ComponentElement<V: View + Clone> {
    id: ElementId,
    view: V,
    child: Option<Box<dyn Element>>,
    size: Size,
    position: Point,
}

impl<V: View + Clone> ComponentElement<V> {
    /// Create a new ComponentElement with a View
    pub fn new(view: V) -> Self {
        let id = ElementId::generate();
        let child = view.create_element();
        Self {
            id,
            view,
            child: Some(child),
            size: Size::ZERO,
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
}

impl<V: View + Clone> Element for ComponentElement<V> {
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
        match &self.child {
            Some(child) => core::slice::from_ref(child),
            None => &[],
        }
    }

    fn children_mut(&mut self) -> &mut [Box<dyn Element>] {
        match &mut self.child {
            Some(child) => core::slice::from_mut(child),
            None => &mut [],
        }
    }

    fn rebuild(&mut self, new_view: &dyn View) -> bool {
        // Try to downcast the new_view to our View type V
        if let Some(new_typed_view) = new_view.as_any().downcast_ref::<V>() {
            // Update our stored view
            self.view = new_typed_view.clone();

            // Create new child element
            let new_child = self.view.create_element();

            // Replace the old child with the new one
            // Note: We could attempt to update in place if the child types match,
            // but for now we always replace
            self.child = Some(new_child);
            true
        } else {
            false
        }
    }

    fn mount(&mut self) {
        // Subscribe to all Listenables from the View
        let listenables = self.view.listenables();

        // For each listenable, subscribe to rebuild notifications
        // Note: In a real implementation, we'd need a way to track and clean up these subscriptions
        // For now, this is a placeholder for the subscription mechanism
        let _ = listenables; // Suppress unused warning

        // Mount the child
        if let Some(ref mut child) = self.child {
            child.mount();
        }
    }

    fn unmount(&mut self) {
        // Unmount the child
        if let Some(ref mut child) = self.child {
            child.unmount();
        }
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Delegate layout to the child
        if let Some(ref mut child) = self.child {
            self.size = child.layout(constraints);
        } else {
            self.size = Size::ZERO;
        }
        self.size
    }

    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, position: Point) {
        self.position = position;
        // Update child position
        if let Some(ref mut child) = self.child {
            child.set_position(position);
        }
    }

    fn bounds(&self) -> crate::geometry::Rect {
        crate::geometry::Rect {
            origin: self.position,
            size: self.size,
        }
    }

    fn hit_test(&self, point: Point) -> bool {
        // Delegate to child
        if let Some(ref child) = self.child {
            child.hit_test(point)
        } else {
            self.bounds().contains(point)
        }
    }

    fn handle_event(&mut self, event: &crate::event::Event) -> bool {
        // Delegate to child
        if let Some(ref mut child) = self.child {
            child.handle_event(event)
        } else {
            false
        }
    }
}
