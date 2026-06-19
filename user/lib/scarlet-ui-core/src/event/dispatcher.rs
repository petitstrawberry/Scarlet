//! Event Dispatcher - Routes events to target elements
//!
//! EventDispatcher implements hit testing and event routing through
//! the element tree with three-phase event dispatching.

use crate::element::{Element, ElementId, ElementTree};
use crate::event::Event;
use crate::geometry::{Point, Rect};
use alloc::vec::Vec;

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
    captured_id: Option<ElementId>,
    captured_path: Vec<ElementId>,
    captured_point: Option<Point>,
    left_button_down: bool,
    focused_id: Option<ElementId>,
    focused_path: Vec<ElementId>,
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
            captured_id: None,
            captured_path: Vec::new(),
            captured_point: None,
            left_button_down: false,
            focused_id: None,
            focused_path: Vec::new(),
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
            crate::logln!("[EventDispatcher] dispatch: {:?}", event);
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
            Event::ScreenSizeChanged { .. } => false,
            Event::Mouse(mouse_event) => self.dispatch_mouse(element_tree, mouse_event),
            Event::Keyboard(key_event) => self.dispatch_keyboard(element_tree, key_event),
            Event::Focus(focus_event) => self.dispatch_focus(element_tree, focus_event),
            Event::Lifecycle(lifecycle_event) => {
                self.dispatch_lifecycle(element_tree, lifecycle_event)
            }
            Event::Window(_window_event) => false,
            Event::Input(_) => {
                // Raw input events are typically handled by higher layers
                false
            }
            Event::MenuItemActivated { .. } => false,
            Event::TextInputPreedit { .. }
            | Event::TextInputCommit { .. }
            | Event::TextInputDeleteSurroundingText { .. }
            | Event::TextInputDone { .. } => self.dispatch_text_input(element_tree, event),
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
    fn dispatch_mouse(
        &mut self,
        element_tree: &mut ElementTree,
        event: &crate::event::MouseEvent,
    ) -> bool {
        // 1. Hit test to find target and path
        let point = self.extract_point_from_mouse(&event);
        let mut path = if matches!(event, crate::event::MouseEvent::Moved { .. })
            && !(self.left_button_down && self.captured_id.is_some())
        {
            self.cached_path_if_inside(element_tree, point)
                .or_else(|| self.hit_test_with_path_ids(element_tree, point))
        } else {
            self.hit_test_with_path_ids(element_tree, point)
        };

        if self.left_button_down {
            if let Some(captured_id) = self.captured_id {
                if let Some(captured_path) = element_tree.find_path_ids(captured_id) {
                    self.captured_path = captured_path.clone();
                    path = Some(captured_path);
                    self.update_captured_point_from_path(element_tree);
                } else if let Some(captured_point) = self.captured_point {
                    if let Some(captured_path) =
                        self.hit_test_with_path_ids(element_tree, captured_point)
                    {
                        self.captured_id = captured_path.last().copied();
                        self.captured_path = captured_path.clone();
                        path = Some(captured_path);
                        self.update_captured_point_from_path(element_tree);
                    } else {
                        self.captured_path.clear();
                    }
                } else {
                    self.captured_path.clear();
                }
            } else if let Some(captured_point) = self.captured_point {
                if let Some(captured_path) =
                    self.hit_test_with_path_ids(element_tree, captured_point)
                {
                    self.captured_id = captured_path.last().copied();
                    self.captured_path = captured_path.clone();
                    path = Some(captured_path);
                    self.update_captured_point_from_path(element_tree);
                }
            }
        }

        if let Some(path) = path {
            let path_origins = Self::path_origins(element_tree, &path);
            let target_id = *path.last().unwrap();
            if crate::debug::is_enabled() {
                crate::logln!(
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
                        crate::logln!(
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
                        crate::logln!("[EventDispatcher] path[{}] id={:?} (not found)", index, id);
                    }
                }
            }

            if let crate::event::MouseEvent::Moved { x, y } = event {
                if self.captured_id.is_some() {
                    // Skip hover updates while dragging outside the element.
                } else if self.hovered_id != Some(target_id) {
                    if crate::debug::is_enabled() {
                        crate::logln!(
                            "[EventDispatcher] hover change: {:?} -> {:?}",
                            self.hovered_id,
                            Some(target_id)
                        );
                    }
                    if let Some(old_id) = self.hovered_id {
                        let old_origin =
                            Self::path_origin_from_ids(element_tree, &self.hovered_path)
                                .unwrap_or(Point::ZERO);
                        if let Some(old_element) = element_tree.find_element_mut(old_id) {
                            let _ = old_element.handle_event(
                                &Event::Mouse(Self::localize_mouse_event(
                                    &crate::event::MouseEvent::Exited { x: *x, y: *y },
                                    old_origin,
                                )),
                                Phase::Target,
                            );
                        }
                    }

                    if let Some(new_element) = element_tree.find_element_mut(target_id) {
                        let new_origin = *path_origins.last().unwrap_or(&Point::ZERO);
                        let _ = new_element.handle_event(
                            &Event::Mouse(Self::localize_mouse_event(
                                &crate::event::MouseEvent::Entered { x: *x, y: *y },
                                new_origin,
                            )),
                            Phase::Target,
                        );
                    }

                    self.hovered_id = Some(target_id);
                    self.hovered_path = path.clone();
                }
            }

            if let crate::event::MouseEvent::ButtonPressed { .. } = event {
                if let Some(focus_id) = element_tree.nearest_focusable_in_path(&path) {
                    if let Some(focus_path) = element_tree.find_path_ids(focus_id) {
                        self.set_focused_element(element_tree, focus_id, &focus_path);
                    }
                } else {
                    self.clear_focused_element(element_tree);
                }
                if let crate::event::MouseEvent::ButtonPressed {
                    button: crate::event::MouseButton::Left,
                    ..
                } = event
                {
                    self.left_button_down = true;
                    self.captured_id = Some(target_id);
                    self.captured_path = path.clone();
                    self.captured_point = Some(point);
                }
            }

            // 2. Three-phase dispatch
            let mut handled = false;

            // 2.1 Capture Phase: root → target (excluding target)
            for (index, id) in path.iter().take(path.len().saturating_sub(1)).enumerate() {
                if let Some(element) = element_tree.find_element_mut(*id) {
                    let localized =
                        Event::Mouse(Self::localize_mouse_event(event, path_origins[index]));
                    if element.handle_event(&localized, Phase::Capture) {
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
                    let localized = Event::Mouse(Self::localize_mouse_event(
                        event,
                        *path_origins.last().unwrap_or(&Point::ZERO),
                    ));
                    handled = target.handle_event(&localized, Phase::Target);
                    if handled {
                        if let Some(window_event) = target.take_window_action() {
                            self.emitted_events.push(Event::Window(window_event));
                        }
                    }
                }
            }

            // 2.3 Bubble Phase: target's parent → root
            if !handled {
                for (index, id) in path.iter().rev().skip(1).enumerate() {
                    if let Some(element) = element_tree.find_element_mut(*id) {
                        let origin_index = path.len().saturating_sub(2).saturating_sub(index);
                        let localized = Event::Mouse(Self::localize_mouse_event(
                            event,
                            path_origins[origin_index],
                        ));
                        if element.handle_event(&localized, Phase::Bubble) {
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
                crate::logln!("[EventDispatcher] mouse handled={}", handled);
            }
            if let crate::event::MouseEvent::ButtonReleased {
                button: crate::event::MouseButton::Left,
                ..
            } = event
            {
                self.left_button_down = false;
                self.captured_id = None;
                self.captured_path.clear();
                self.captured_point = None;
            }
            handled
        } else {
            if let crate::event::MouseEvent::Moved { x, y } = event {
                if let Some(old_id) = self.hovered_id {
                    let old_origin = Self::path_origin_from_ids(element_tree, &self.hovered_path)
                        .unwrap_or(Point::ZERO);
                    if let Some(old_element) = element_tree.find_element_mut(old_id) {
                        let _ = old_element.handle_event(
                            &Event::Mouse(Self::localize_mouse_event(
                                &crate::event::MouseEvent::Exited { x: *x, y: *y },
                                old_origin,
                            )),
                            Phase::Target,
                        );
                    }
                }
                self.hovered_id = None;
                self.hovered_path.clear();
            }
            if let crate::event::MouseEvent::ButtonReleased {
                button: crate::event::MouseButton::Left,
                ..
            } = event
            {
                self.left_button_down = false;
                self.captured_id = None;
                self.captured_path.clear();
                self.captured_point = None;
            }
            false
        }
    }

    /// Dispatch a keyboard event
    fn dispatch_keyboard(
        &mut self,
        element_tree: &mut ElementTree,
        event: &crate::event::KeyEvent,
    ) -> bool {
        let path = self
            .focused_id
            .and_then(|focused_id| {
                element_tree
                    .element_wants_keyboard_focus(focused_id)
                    .then(|| element_tree.find_path_ids(focused_id))
                    .flatten()
            })
            .or_else(|| element_tree.find_keyboard_focus_path_ids())
            .or_else(|| {
                if self
                    .focused_path
                    .last()
                    .is_some_and(|id| element_tree.element_wants_keyboard_focus(*id))
                {
                    Some(self.focused_path.clone())
                } else {
                    None
                }
            })
            .or_else(|| element_tree.root().map(|root| alloc::vec![root.id()]));

        let Some(path) = path else {
            return false;
        };
        let Some(target_id) = path.last().copied() else {
            return false;
        };
        self.focused_id = Some(target_id);
        self.focused_path = path.clone();

        if crate::debug::is_enabled() {
            crate::logln!(
                "[EventDispatcher] keyboard: {:?} target_id={:?} path_len={}",
                event,
                target_id,
                path.len()
            );
        }

        let keyboard_event = Event::Keyboard(*event);
        let mut handled = false;

        for id in path.iter().take(path.len().saturating_sub(1)) {
            if let Some(element) = element_tree.find_element_mut(*id) {
                if element.handle_event(&keyboard_event, Phase::Capture) {
                    handled = true;
                    break;
                }
            }
        }

        if !handled {
            if let Some(target) = element_tree.find_element_mut(target_id) {
                handled = target.handle_event(&keyboard_event, Phase::Target);
            }
        }

        if !handled {
            for id in path.iter().rev().skip(1) {
                if let Some(element) = element_tree.find_element_mut(*id) {
                    if element.handle_event(&keyboard_event, Phase::Bubble) {
                        handled = true;
                        break;
                    }
                }
            }
        }

        if crate::debug::is_enabled() {
            crate::logln!("[EventDispatcher] keyboard handled={}", handled);
        }
        handled
    }

    fn dispatch_text_input(&mut self, element_tree: &mut ElementTree, event: &Event) -> bool {
        let path = self
            .focused_id
            .and_then(|focused_id| {
                element_tree
                    .element_wants_keyboard_focus(focused_id)
                    .then(|| element_tree.find_path_ids(focused_id))
                    .flatten()
            })
            .or_else(|| element_tree.find_keyboard_focus_path_ids())
            .or_else(|| {
                if self
                    .focused_path
                    .last()
                    .is_some_and(|id| element_tree.element_wants_keyboard_focus(*id))
                {
                    Some(self.focused_path.clone())
                } else {
                    None
                }
            });

        let Some(path) = path else {
            return false;
        };
        let Some(target_id) = path.last().copied() else {
            return false;
        };
        self.focused_id = Some(target_id);
        self.focused_path = path;

        element_tree
            .find_element_mut(target_id)
            .is_some_and(|target| target.handle_event(event, Phase::Target))
    }

    /// Dispatch a focus event
    fn dispatch_focus(
        &mut self,
        element_tree: &mut ElementTree,
        event: &crate::event::FocusEvent,
    ) -> bool {
        // Focus events are sent to the element gaining or losing focus
        // For now, send to the root with Target phase
        if let Some(root) = element_tree.root_mut() {
            root.handle_event(&Event::Focus(event.clone()), Phase::Target)
        } else {
            false
        }
    }

    /// Dispatch a lifecycle event
    fn dispatch_lifecycle(
        &mut self,
        element_tree: &mut ElementTree,
        event: &crate::event::LifecycleEvent,
    ) -> bool {
        // Lifecycle events are sent to elements during mount/unmount
        // For now, send to the root with Target phase
        if let Some(root) = element_tree.root_mut() {
            root.handle_event(&Event::Lifecycle(event.clone()), Phase::Target)
        } else {
            false
        }
    }

    /// Hit test to find the element at a point
    pub fn hit_test<'a>(
        &'a self,
        element_tree: &'a ElementTree,
        point: Point,
    ) -> Option<&'a dyn Element> {
        let root = element_tree.root()?;
        self.hit_test_select_overlay_recursive(root, point)
            .or_else(|| self.hit_test_recursive(root, point))
            .map(|(target, _)| target)
    }

    /// Hit test to find the element at a point with the path from root
    ///
    /// This returns a HitResult containing both the target and the full path,
    /// which is necessary for three-phase event dispatching.
    pub fn hit_test_with_path<'a>(
        &'a self,
        element_tree: &'a ElementTree,
        point: Point,
    ) -> Option<HitResult<'a>> {
        let root = element_tree.root()?;
        self.hit_test_select_overlay_recursive(root, point)
            .or_else(|| self.hit_test_recursive(root, point))
            .map(|(target, path)| HitResult::new(target, path))
    }

    /// Recursive hit test implementation that returns target and path
    fn hit_test_recursive<'a>(
        &'a self,
        element: &'a dyn Element,
        point: Point,
    ) -> Option<(&'a dyn Element, Vec<&'a dyn Element>)> {
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
        let mut path = self
            .hit_test_select_overlay_recursive_ids(root, point)
            .or_else(|| self.hit_test_recursive_ids(root, point))?;
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

    fn hit_test_select_overlay_recursive<'a>(
        &'a self,
        element: &'a dyn Element,
        point: Point,
    ) -> Option<(&'a dyn Element, Vec<&'a dyn Element>)> {
        let local_point = Point {
            x: point.x - element.position().x,
            y: point.y - element.position().y,
        };

        for child in element.children().iter().rev() {
            if let Some((found, mut path)) =
                self.hit_test_select_overlay_recursive(child.as_ref(), local_point)
            {
                path.push(element);
                return Some((found, path));
            }
        }

        if Self::is_expanded_select(element) && element.hit_test(point) {
            return Some((element, alloc::vec![element]));
        }

        None
    }

    fn hit_test_select_overlay_recursive_ids(
        &self,
        element: &dyn Element,
        point: Point,
    ) -> Option<Vec<ElementId>> {
        let local_point = Point {
            x: point.x - element.position().x,
            y: point.y - element.position().y,
        };

        for child in element.children().iter().rev() {
            if let Some(mut path) =
                self.hit_test_select_overlay_recursive_ids(child.as_ref(), local_point)
            {
                path.push(element.id());
                return Some(path);
            }
        }

        if Self::is_expanded_select(element) && element.hit_test(point) {
            return Some(alloc::vec![element.id()]);
        }

        None
    }

    fn is_expanded_select(element: &dyn Element) -> bool {
        element
            .render_object()
            .and_then(|render_object| {
                render_object
                    .as_any()
                    .downcast_ref::<crate::views::SelectRenderObject>()
            })
            .is_some_and(|select| select.is_expanded())
    }

    /// Extract point from a mouse event
    fn extract_point_from_mouse(&self, event: &crate::event::MouseEvent) -> Point {
        match event {
            crate::event::MouseEvent::Moved { x, y } => Point {
                x: *x as f32,
                y: *y as f32,
            },
            crate::event::MouseEvent::Entered { x, y } => Point {
                x: *x as f32,
                y: *y as f32,
            },
            crate::event::MouseEvent::Exited { x, y } => Point {
                x: *x as f32,
                y: *y as f32,
            },
            crate::event::MouseEvent::ButtonPressed { x, y, .. } => Point {
                x: *x as f32,
                y: *y as f32,
            },
            crate::event::MouseEvent::ButtonReleased { x, y, .. } => Point {
                x: *x as f32,
                y: *y as f32,
            },
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

        let mut parent_origin = Point::ZERO;
        if self.hovered_path.len() > 1 {
            for id in self.hovered_path.iter().take(self.hovered_path.len() - 1) {
                let Some(element) = element_tree.find_element_mut(*id) else {
                    self.hovered_path.clear();
                    return None;
                };
                let pos = element.position();
                parent_origin.x += pos.x;
                parent_origin.y += pos.y;
            }
        }

        let Some(target) = element_tree.find_element_mut(hovered_id) else {
            self.hovered_path.clear();
            return None;
        };
        let target_pos = target.position();
        let absolute_origin = Point {
            x: parent_origin.x + target_pos.x,
            y: parent_origin.y + target_pos.y,
        };
        let size = target.bounds().size;
        let rect = Rect::from_xywh(
            absolute_origin.x,
            absolute_origin.y,
            size.width,
            size.height,
        );
        if rect.contains(point) {
            return Some(self.hovered_path.clone());
        }

        None
    }

    fn update_captured_point_from_path(&mut self, element_tree: &mut ElementTree) {
        let Some(target_id) = self.captured_path.last().copied() else {
            return;
        };

        let mut parent_origin = Point::ZERO;
        if self.captured_path.len() > 1 {
            for id in self.captured_path.iter().take(self.captured_path.len() - 1) {
                let Some(element) = element_tree.find_element_mut(*id) else {
                    return;
                };
                let pos = element.position();
                parent_origin.x += pos.x;
                parent_origin.y += pos.y;
            }
        }

        let Some(target) = element_tree.find_element_mut(target_id) else {
            return;
        };
        let target_pos = target.position();
        let bounds = target.bounds().size;
        let absolute_origin = Point {
            x: parent_origin.x + target_pos.x,
            y: parent_origin.y + target_pos.y,
        };
        self.captured_point = Some(Point {
            x: absolute_origin.x + bounds.width / 2.0,
            y: absolute_origin.y + bounds.height / 2.0,
        });
    }

    fn set_focused_element(
        &mut self,
        element_tree: &mut ElementTree,
        target_id: ElementId,
        path: &[ElementId],
    ) {
        if self.focused_id == Some(target_id) {
            self.focused_path = path.to_vec();
            return;
        }

        if let Some(old_path) = self.current_focus_path(element_tree)
            && let Some(old_target_id) = old_path.last().copied()
            && old_target_id != target_id
            && let Some(old_target) = element_tree.find_element_mut(old_target_id)
        {
            let _ = old_target
                .handle_event(&Event::Focus(crate::event::FocusEvent::Lost), Phase::Target);
        }

        self.focused_id = Some(target_id);
        self.focused_path = path.to_vec();
        if let Some(target) = element_tree.find_element_mut(target_id) {
            let _ = target.handle_event(
                &Event::Focus(crate::event::FocusEvent::Gained),
                Phase::Target,
            );
        }
    }

    fn clear_focused_element(&mut self, element_tree: &mut ElementTree) {
        if let Some(old_path) = self.current_focus_path(element_tree)
            && let Some(old_target_id) = old_path.last().copied()
            && let Some(old_target) = element_tree.find_element_mut(old_target_id)
        {
            let _ = old_target
                .handle_event(&Event::Focus(crate::event::FocusEvent::Lost), Phase::Target);
        }
        self.focused_id = None;
        self.focused_path.clear();
    }

    fn current_focus_path(&mut self, element_tree: &mut ElementTree) -> Option<Vec<ElementId>> {
        self.focused_id
            .and_then(|focused_id| {
                element_tree
                    .find_path_ids(focused_id)
                    .filter(|_| element_tree.element_wants_keyboard_focus(focused_id))
            })
            .or_else(|| element_tree.find_keyboard_focus_path_ids())
    }

    fn path_origins(element_tree: &mut ElementTree, path: &[ElementId]) -> Vec<Point> {
        let mut origins = Vec::with_capacity(path.len());
        let mut acc = Point::ZERO;
        for id in path {
            if let Some(element) = element_tree.find_element_mut(*id) {
                let pos = element.position();
                acc.x += pos.x;
                acc.y += pos.y;
            }
            origins.push(acc);
        }
        origins
    }

    fn path_origin_from_ids(element_tree: &mut ElementTree, path: &[ElementId]) -> Option<Point> {
        if path.is_empty() {
            return None;
        }
        let mut acc = Point::ZERO;
        for id in path {
            let element = element_tree.find_element_mut(*id)?;
            let pos = element.position();
            acc.x += pos.x;
            acc.y += pos.y;
        }
        Some(acc)
    }

    fn localize_mouse_event(
        event: &crate::event::MouseEvent,
        origin: Point,
    ) -> crate::event::MouseEvent {
        match *event {
            crate::event::MouseEvent::Moved { x, y } => crate::event::MouseEvent::Moved {
                x: x - origin.x as i32,
                y: y - origin.y as i32,
            },
            crate::event::MouseEvent::Entered { x, y } => crate::event::MouseEvent::Entered {
                x: x - origin.x as i32,
                y: y - origin.y as i32,
            },
            crate::event::MouseEvent::Exited { x, y } => crate::event::MouseEvent::Exited {
                x: x - origin.x as i32,
                y: y - origin.y as i32,
            },
            crate::event::MouseEvent::ButtonPressed { button, x, y } => {
                crate::event::MouseEvent::ButtonPressed {
                    button,
                    x: x - origin.x as i32,
                    y: y - origin.y as i32,
                }
            }
            crate::event::MouseEvent::ButtonReleased { button, x, y } => {
                crate::event::MouseEvent::ButtonReleased {
                    button,
                    x: x - origin.x as i32,
                    y: y - origin.y as i32,
                }
            }
            crate::event::MouseEvent::Wheel { delta_x, delta_y } => {
                crate::event::MouseEvent::Wheel { delta_x, delta_y }
            }
        }
    }
}

impl Default for EventDispatcher {
    fn default() -> Self {
        Self::new()
    }
}
