//! Event Dispatcher - Routes events to target elements
//!
//! EventDispatcher implements hit testing and event routing through
//! the element tree.

use crate::element::{Element, ElementTree};
use crate::geometry::Point;
use crate::event::Event;

/// Event Dispatcher for routing events to elements
///
/// The dispatcher implements:
/// - Hit testing to find event targets
/// - Capture phase (root → target)
/// - Target phase (handle at target)
/// - Bubble phase (target → root)
pub struct EventDispatcher {
    root_id: Option<crate::element::ElementId>,
}

impl EventDispatcher {
    /// Create a new EventDispatcher
    pub fn new() -> Self {
        Self {
            root_id: None,
        }
    }

    /// Set the root element ID
    pub fn set_root(&mut self, id: crate::element::ElementId) {
        self.root_id = Some(id);
    }

    /// Dispatch an event to the appropriate element
    pub fn dispatch(&mut self, element_tree: &mut ElementTree, event: &Event) {
        match event {
            Event::Quit => {
                // Quit event is handled at application level
                self.handle_quit(element_tree);
            }
            Event::Resize { width, height } => {
                self.handle_resize(element_tree, *width, *height);
            }
            Event::Mouse(mouse_event) => {
                self.dispatch_mouse(element_tree, mouse_event);
            }
            Event::Keyboard(key_event) => {
                self.dispatch_keyboard(element_tree, key_event);
            }
            Event::Input(_) => {
                // Raw input events are typically handled by higher layers
            }
            Event::Custom { .. } => {
                // Custom events can be dispatched similarly
            }
        }
    }

    /// Handle quit event
    fn handle_quit(&mut self, _element_tree: &mut ElementTree) {
        // Signal the application to exit
        // In a full implementation, this would set a flag or call a callback
    }

    /// Handle resize event
    fn handle_resize(&mut self, _element_tree: &mut ElementTree, width: u32, height: u32) {
        // Handle window resize
        // In a full implementation, this would mark elements for relayout
        let _ = (width, height);
    }

    /// Dispatch a mouse event
    fn dispatch_mouse(&mut self, element_tree: &mut ElementTree, event: &crate::event::MouseEvent) {
        // 1. Hit test to find target
        let point = self.extract_point_from_mouse(&event);
        let target = self.hit_test(element_tree, point);

        // 2. Dispatch to target
        if let Some(_target_element) = target {
            // Forward event to the element
            // Note: Full event forwarding will be implemented later
            // For now, we just find the target
            let _ = element_tree;
        }
    }

    /// Dispatch a keyboard event
    fn dispatch_keyboard(&mut self, element_tree: &mut ElementTree, event: &crate::event::KeyEvent) {
        // Keyboard events typically go to the focused element
        // For now, send to the root
        if let Some(root) = element_tree.root_mut() {
            root.handle_event(&Event::Keyboard(event.clone()));
        }
    }

    /// Hit test to find the element at a point
    pub fn hit_test<'a>(&'a self, element_tree: &'a ElementTree, point: Point) -> Option<&'a dyn Element> {
        let root = element_tree.root()?;
        self.hit_test_recursive(root, point)
    }

    /// Recursive hit test implementation
    fn hit_test_recursive<'a>(&'a self, element: &'a dyn Element, point: Point) -> Option<&'a dyn Element> {
        // Check children first (reverse order for z-index)
        for child in element.children().iter().rev() {
            if let Some(found) = self.hit_test_recursive(child.as_ref(), point) {
                return Some(found);
            }
        }

        // Check this element
        if element.hit_test(point) {
            return Some(element);
        }

        None
    }

    /// Extract point from a mouse event
    fn extract_point_from_mouse(&self, event: &crate::event::MouseEvent) -> Point {
        match event {
            crate::event::MouseEvent::Moved { x, y } => Point { x: *x as f32, y: *y as f32 },
            crate::event::MouseEvent::ButtonPressed { x, y, .. } => Point { x: *x as f32, y: *y as f32 },
            crate::event::MouseEvent::ButtonReleased { x, y, .. } => Point { x: *x as f32, y: *y as f32 },
            crate::event::MouseEvent::Wheel { .. } => Point::ZERO,
        }
    }
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}
