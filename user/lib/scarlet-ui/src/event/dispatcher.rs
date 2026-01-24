//! Event Dispatcher - Routes events to target elements
//!
//! EventDispatcher implements hit testing and event routing through
//! the element tree with three-phase event dispatching.

use alloc::vec::Vec;
use crate::element::{Element, ElementId, ElementTree};
use crate::geometry::Point;
use crate::event::Event;

/// Event dispatch phase
///
/// Events go through three phases:
/// 1. Capture: From root to target's parent
/// 2. Target: At the target element itself
/// 3. Bubble: From target's parent back to root
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Capture phase (root → target)
    Capture,
    /// Target phase (at the target)
    Target,
    /// Bubble phase (target → root)
    Bubble,
}

/// Result of a hit test operation
///
/// Contains the target element and the path from root to target
/// for use in three-phase event dispatching.
pub struct HitResult<'a> {
    /// The target element that was hit
    pub target: &'a dyn Element,
    /// The path from root to target (inclusive)
    /// For capture phase, iterate from index 0 to len-1
    /// For bubble phase, iterate from index len-1 to 0
    pub path: Vec<&'a dyn Element>,
}

impl<'a> HitResult<'a> {
    /// Create a new HitResult
    pub fn new(target: &'a dyn Element, path: Vec<&'a dyn Element>) -> Self {
        Self { target, path }
    }

    /// Get the path for the capture phase (root to target)
    pub fn capture_path(&self) -> impl Iterator<Item = &&'a dyn Element> {
        self.path.iter()
    }

    /// Get the path for the bubble phase (target to root)
    pub fn bubble_path(&self) -> impl Iterator<Item = &&'a dyn Element> {
        self.path.iter().rev()
    }
}

/// Event Dispatcher for routing events to elements
///
/// The dispatcher implements:
/// - Hit testing to find event targets
/// - Capture phase (root → target)
/// - Target phase (handle at target)
/// - Bubble phase (target → root)
pub struct EventDispatcher {
    root_id: Option<ElementId>,
}

impl EventDispatcher {
    /// Create a new EventDispatcher
    pub fn new() -> Self {
        Self {
            root_id: None,
        }
    }

    /// Set the root element ID
    pub fn set_root(&mut self, id: ElementId) {
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
            Event::Focus(focus_event) => {
                self.dispatch_focus(element_tree, focus_event);
            }
            Event::Lifecycle(lifecycle_event) => {
                self.dispatch_lifecycle(element_tree, lifecycle_event);
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

    /// Dispatch a mouse event with three-phase event handling
    fn dispatch_mouse(&mut self, element_tree: &mut ElementTree, event: &crate::event::MouseEvent) {
        // 1. Hit test to find target and path
        let point = self.extract_point_from_mouse(&event);
        let hit_result = self.hit_test_with_path(element_tree, point);

        if let Some(hit) = hit_result {
            // 2. Three-phase dispatch
            let event_wrapper = Event::Mouse(event.clone());

            // 2.1 Capture Phase: root → target (excluding target)
            for element in hit.capture_path().take(hit.path.len().saturating_sub(1)) {
                // Create mutable reference to element
                // Note: In a full implementation, we would need a way to get mutable references
                // to elements by ID. For now, this is a conceptual implementation.
                let _ = element;
                let _ = &event_wrapper;
                // element.handle_event(&event_wrapper, Phase::Capture);
            }

            // 2.2 Target Phase: at the target
            // Note: In a full implementation, we would get mutable reference to target
            let _ = hit.target;
            // hit.target.handle_event(&event_wrapper, Phase::Target);

            // 2.3 Bubble Phase: target's parent → root
            for element in hit.bubble_path().skip(1) {
                // Skip the target (already handled in target phase)
                let _ = element;
                let _ = &event_wrapper;
                // element.handle_event(&event_wrapper, Phase::Bubble);
            }
        }
    }

    /// Dispatch a keyboard event
    fn dispatch_keyboard(&mut self, element_tree: &mut ElementTree, event: &crate::event::KeyEvent) {
        // Keyboard events typically go to the focused element
        // For now, send to the root with Target phase
        if let Some(root) = element_tree.root_mut() {
            root.handle_event(&Event::Keyboard(event.clone()), Phase::Target);
        }
    }

    /// Dispatch a focus event
    fn dispatch_focus(&mut self, element_tree: &mut ElementTree, event: &crate::event::FocusEvent) {
        // Focus events are sent to the element gaining or losing focus
        // For now, send to the root with Target phase
        if let Some(root) = element_tree.root_mut() {
            root.handle_event(&Event::Focus(event.clone()), Phase::Target);
        }
    }

    /// Dispatch a lifecycle event
    fn dispatch_lifecycle(&mut self, element_tree: &mut ElementTree, event: &crate::event::LifecycleEvent) {
        // Lifecycle events are sent to elements during mount/unmount
        // For now, send to the root with Target phase
        if let Some(root) = element_tree.root_mut() {
            root.handle_event(&Event::Lifecycle(event.clone()), Phase::Target);
        }
    }

    /// Hit test to find the element at a point
    pub fn hit_test<'a>(&'a self, element_tree: &'a ElementTree, point: Point) -> Option<&'a dyn Element> {
        let root = element_tree.root()?;
        self.hit_test_recursive(root, point).map(|(target, _)| target)
    }

    /// Hit test to find the element at a point with the path from root
    ///
    /// This returns a HitResult containing both the target and the full path,
    /// which is necessary for three-phase event dispatching.
    pub fn hit_test_with_path<'a>(&'a self, element_tree: &'a ElementTree, point: Point) -> Option<HitResult<'a>> {
        let root = element_tree.root()?;
        self.hit_test_recursive(root, point).map(|(target, path)| HitResult::new(target, path))
    }

    /// Recursive hit test implementation that returns target and path
    fn hit_test_recursive<'a>(&'a self, element: &'a dyn Element, point: Point) -> Option<(&'a dyn Element, Vec<&'a dyn Element>)> {
        // Check children first (reverse order for z-index)
        for child in element.children().iter().rev() {
            if let Some((found, mut path)) = self.hit_test_recursive(child.as_ref(), point) {
                // Add this element to the path
                path.push(element);
                return Some((found, path));
            }
        }

        // Check this element
        if element.hit_test(point) {
            let mut path = Vec::new();
            path.push(element);
            return Some((element, path));
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
