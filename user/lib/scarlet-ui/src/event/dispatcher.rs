//! Event Dispatcher - Routes events to target elements
//!
//! EventDispatcher implements hit testing and event routing through
//! the element tree with three-phase event dispatching.

use alloc::vec::Vec;
use crate::element::{Element, ElementId, ElementTree};
use crate::geometry::{Point, Rect};
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
    hovered_id: Option<ElementId>,
    hovered_path: Vec<ElementId>,
    /// Events emitted by elements during event handling
    emitted_events: Vec<Event>,
}

impl EventDispatcher {
    /// Create a new EventDispatcher
    pub fn new() -> Self {
        Self {
            root_id: None,
            hovered_id: None,
            hovered_path: Vec::new(),
            emitted_events: Vec::new(),
        }
    }

    /// Set the root element ID
    pub fn set_root(&mut self, id: ElementId) {
        self.root_id = Some(id);
    }

    /// Emit an event (called by elements during event handling)
    pub fn emit(&mut self, event: Event) {
        self.emitted_events.push(event);
    }

    /// Take all emitted events and clear the buffer
    pub fn take_emitted_events(&mut self) -> Vec<Event> {
        core::mem::take(&mut self.emitted_events)
    }

    /// Dispatch an event to the appropriate element
    pub fn dispatch(&mut self, element_tree: &mut ElementTree, event: &Event) -> bool {
        if crate::debug::is_enabled() {
            scarlet_std::println!("[EventDispatcher] dispatch: {:?}", event);
        }
        match event {
            Event::Quit => {
                // Quit event is handled at application level
                self.handle_quit(element_tree);
                false
            }
            Event::Resize { width, height } => {
                self.handle_resize(element_tree, *width, *height);
                false
            }
            Event::Mouse(mouse_event) => {
                self.dispatch_mouse(element_tree, mouse_event)
            }
            Event::Keyboard(key_event) => {
                self.dispatch_keyboard(element_tree, key_event)
            }
            Event::Focus(focus_event) => {
                self.dispatch_focus(element_tree, focus_event)
            }
            Event::Lifecycle(lifecycle_event) => {
                self.dispatch_lifecycle(element_tree, lifecycle_event)
            }
            Event::Window(_window_event) => {
                false
            }
            Event::Input(_) => {
                // Raw input events are typically handled by higher layers
                false
            }
            Event::Custom { .. } => {
                // Custom events can be dispatched similarly
                false
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
    fn dispatch_mouse(&mut self, element_tree: &mut ElementTree, event: &crate::event::MouseEvent) -> bool {
        // 1. Hit test to find target and path
        let point = self.extract_point_from_mouse(&event);
        let path = if matches!(event, crate::event::MouseEvent::Moved { .. }) {
            self.cached_path_if_inside(element_tree, point)
                .or_else(|| self.hit_test_with_path_ids(element_tree, point))
        } else {
            self.hit_test_with_path_ids(element_tree, point)
        };

        if let Some(path) = path {
            let target_id = *path.last().unwrap();
            if crate::debug::is_enabled() {
                scarlet_std::println!(
                    "[EventDispatcher] mouse: {:?} point=({:.1},{:.1}) target_id={:?} path_len={}",
                    event,
                    point.x,
                    point.y,
                    target_id,
                    path.len()
                );
                for (index, id) in path.iter().enumerate() {
                    if let Some(element) = element_tree.find_element_mut(*id) {
                        let bounds = element.bounds();
                        scarlet_std::println!(
                            "[EventDispatcher] path[{}] id={:?} type={} bounds=({:.1},{:.1},{:.1},{:.1})",
                            index,
                            id,
                            element.type_name_debug(),
                            bounds.origin.x,
                            bounds.origin.y,
                            bounds.size.width,
                            bounds.size.height
                        );
                    } else {
                        scarlet_std::println!(
                            "[EventDispatcher] path[{}] id={:?} (not found)",
                            index,
                            id
                        );
                    }
                }
            }

            if let crate::event::MouseEvent::Moved { x, y } = event {
                if self.hovered_id != Some(target_id) {
                    if crate::debug::is_enabled() {
                        scarlet_std::println!(
                            "[EventDispatcher] hover change: {:?} -> {:?}",
                            self.hovered_id,
                            Some(target_id)
                        );
                    }
                    if let Some(old_id) = self.hovered_id {
                        if let Some(old_element) = element_tree.find_element_mut(old_id) {
                            let _ = old_element.handle_event(
                                &Event::Mouse(crate::event::MouseEvent::Exited { x: *x, y: *y }),
                                Phase::Target,
                            );
                        }
                    }

                    if let Some(new_element) = element_tree.find_element_mut(target_id) {
                        let _ = new_element.handle_event(
                            &Event::Mouse(crate::event::MouseEvent::Entered { x: *x, y: *y }),
                            Phase::Target,
                        );
                    }

                    self.hovered_id = Some(target_id);
                    self.hovered_path = path.clone();
                }
            }

            // 2. Three-phase dispatch
            let event_wrapper = Event::Mouse(event.clone());
            let mut handled = false;

            // 2.1 Capture Phase: root → target (excluding target)
            for id in path.iter().take(path.len().saturating_sub(1)) {
                if let Some(element) = element_tree.find_element_mut(*id) {
                    if element.handle_event(&event_wrapper, Phase::Capture) {
                        if let Some(window_event) = element.take_window_action() {
                            self.emitted_events.push(Event::Window(window_event));
                        }
                        handled = true;
                        break;
                    }
                }
            }

            // 2.2 Target Phase: at the target
            if !handled {
                if let Some(target) = element_tree.find_element_mut(target_id) {
                    handled = target.handle_event(&event_wrapper, Phase::Target);
                    if handled {
                        if let Some(window_event) = target.take_window_action() {
                            self.emitted_events.push(Event::Window(window_event));
                        }
                    }
                }
            }

            // 2.3 Bubble Phase: target's parent → root
            if !handled {
                for id in path.iter().rev().skip(1) {
                    if let Some(element) = element_tree.find_element_mut(*id) {
                        if element.handle_event(&event_wrapper, Phase::Bubble) {
                            if let Some(window_event) = element.take_window_action() {
                                self.emitted_events.push(Event::Window(window_event));
                            }
                            handled = true;
                            break;
                        }
                    }
                }
            }

            if crate::debug::is_enabled() {
                scarlet_std::println!("[EventDispatcher] mouse handled={}", handled);
            }
            handled
        } else {
            if let crate::event::MouseEvent::Moved { x, y } = event {
                if let Some(old_id) = self.hovered_id {
                    if let Some(old_element) = element_tree.find_element_mut(old_id) {
                        let _ = old_element.handle_event(
                            &Event::Mouse(crate::event::MouseEvent::Exited { x: *x, y: *y }),
                            Phase::Target,
                        );
                    }
                }
                self.hovered_id = None;
                self.hovered_path.clear();
            }
            false
        }
    }

    /// Dispatch a keyboard event
    fn dispatch_keyboard(&mut self, element_tree: &mut ElementTree, event: &crate::event::KeyEvent) -> bool {
        // Keyboard events typically go to the focused element
        // For now, send to the root with Target phase
        if let Some(root) = element_tree.root_mut() {
            root.handle_event(&Event::Keyboard(event.clone()), Phase::Target)
        } else {
            false
        }
    }

    /// Dispatch a focus event
    fn dispatch_focus(&mut self, element_tree: &mut ElementTree, event: &crate::event::FocusEvent) -> bool {
        // Focus events are sent to the element gaining or losing focus
        // For now, send to the root with Target phase
        if let Some(root) = element_tree.root_mut() {
            root.handle_event(&Event::Focus(event.clone()), Phase::Target)
        } else {
            false
        }
    }

    /// Dispatch a lifecycle event
    fn dispatch_lifecycle(&mut self, element_tree: &mut ElementTree, event: &crate::event::LifecycleEvent) -> bool {
        // Lifecycle events are sent to elements during mount/unmount
        // For now, send to the root with Target phase
        if let Some(root) = element_tree.root_mut() {
            root.handle_event(&Event::Lifecycle(event.clone()), Phase::Target)
        } else {
            false
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
        let local_point = Point {
            x: point.x - element.position().x,
            y: point.y - element.position().y,
        };
        // Check children first (reverse order for z-index)
        for child in element.children().iter().rev() {
            if let Some((found, mut path)) = self.hit_test_recursive(child.as_ref(), local_point) {
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

    /// Hit test to find the element at a point with the path of IDs from root
    pub fn hit_test_with_path_ids(
        &self,
        element_tree: &ElementTree,
        point: Point,
    ) -> Option<Vec<ElementId>> {
        let root = element_tree.root()?;
        let mut path = self.hit_test_recursive_ids(root, point)?;
        path.reverse();
        Some(path)
    }

    fn hit_test_recursive_ids(
        &self,
        element: &dyn Element,
        point: Point,
    ) -> Option<Vec<ElementId>> {
        let local_point = Point {
            x: point.x - element.position().x,
            y: point.y - element.position().y,
        };
        for child in element.children().iter().rev() {
            if let Some(mut path) = self.hit_test_recursive_ids(child.as_ref(), local_point) {
                path.push(element.id());
                return Some(path);
            }
        }

        if element.hit_test(point) {
            return Some(alloc::vec![element.id()]);
        }

        None
    }

    /// Extract point from a mouse event
    fn extract_point_from_mouse(&self, event: &crate::event::MouseEvent) -> Point {
        match event {
            crate::event::MouseEvent::Moved { x, y } => Point { x: *x as f32, y: *y as f32 },
            crate::event::MouseEvent::Entered { x, y } => Point { x: *x as f32, y: *y as f32 },
            crate::event::MouseEvent::Exited { x, y } => Point { x: *x as f32, y: *y as f32 },
            crate::event::MouseEvent::ButtonPressed { x, y, .. } => Point { x: *x as f32, y: *y as f32 },
            crate::event::MouseEvent::ButtonReleased { x, y, .. } => Point { x: *x as f32, y: *y as f32 },
            crate::event::MouseEvent::Wheel { .. } => Point::ZERO,
        }
    }

    fn cached_path_if_inside(
        &mut self,
        element_tree: &mut ElementTree,
        point: Point,
    ) -> Option<Vec<ElementId>> {
        let hovered_id = self.hovered_id?;
        let last = *self.hovered_path.last()?;
        if last != hovered_id {
            self.hovered_path.clear();
            return None;
        }

        let mut absolute_origin = Point::ZERO;
        for id in self.hovered_path.iter() {
            let Some(element) = element_tree.find_element_mut(*id) else {
                self.hovered_path.clear();
                return None;
            };
            let pos = element.position();
            absolute_origin.x += pos.x;
            absolute_origin.y += pos.y;
        }

        let Some(target) = element_tree.find_element_mut(hovered_id) else {
            self.hovered_path.clear();
            return None;
        };
        let size = target.bounds().size;
        let rect = Rect::from_xywh(absolute_origin.x, absolute_origin.y, size.width, size.height);
        if rect.contains(point) {
            return Some(self.hovered_path.clone());
        }

        None
    }
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}
