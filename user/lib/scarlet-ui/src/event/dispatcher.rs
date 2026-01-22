use crate::event::{Event, EventContext, EventPhase, HitResult, KeyEvent, MouseEventKind};
use crate::geometry::Point;
use crate::node_id::NodeId;
use crate::traits::RenderObject;
use std::vec::Vec;
use std::println;

pub struct EventDispatcher<'a> {
    root: &'a mut dyn RenderObject,
    focus_manager: &'a mut crate::event::FocusManager,
    hover_manager: &'a mut crate::event::HoverManager,
    pressed_manager: &'a mut crate::event::PressedManager,
}

impl<'a> EventDispatcher<'a> {
    pub fn new(
        root: &'a mut dyn RenderObject,
        focus_manager: &'a mut crate::event::FocusManager,
        hover_manager: &'a mut crate::event::HoverManager,
        pressed_manager: &'a mut crate::event::PressedManager,
    ) -> Self {
        Self { root, focus_manager, hover_manager, pressed_manager }
    }

    pub fn dispatch(&mut self, event: &Event) {
        // println!("[dispatcher] dispatch()");
        if event.is_keyboard() {
            // println!("[dispatcher] Keyboard event, dispatching");
            self.dispatch_keyboard(event);
        } else {
            // Track press/release for proper behavior
            if let Event::Mouse(mouse_event) = event {
                if mouse_event.kind == MouseEventKind::Press {
                    if let Some(target) = self.find_target(event) {
                        self.pressed_manager.set_pressed(target);
                    }
                } else if mouse_event.kind == MouseEventKind::Release {
                    // Notify pressed node about release (even if released outside)
                    if let Some(pressed_id) = self.pressed_manager.pressed() {
                        let current_target = self.find_target(event);

                        // If released outside pressed node, send release to pressed node first
                        if current_target != Some(pressed_id) {
                            let path = self.build_path_to_target(pressed_id);
                            let release_event = Event::Mouse(crate::event::MouseEvent {
                                position: mouse_event.position,
                                buttons: mouse_event.buttons,
                                kind: MouseEventKind::Release,
                            });
                            let mut ctx = EventContext::default();
                            self.target_phase(&release_event, &path, pressed_id, &mut ctx);
                        }
                    }
                    self.pressed_manager.clear_pressed();
                }

                // Mouse move for hover tracking
                if mouse_event.kind == MouseEventKind::Move {
                    self.dispatch_mouse_move_with_hover(mouse_event);
                    return;
                }
            }

            // Mouse events - find target via hit test
            let target_id = match self.find_target(event) {
                Some(id) => id,
                None => return, // No target, ignore event
            };

            // Build path once (root → target)
            let path = self.build_path_to_target(target_id);
            // println!("[dispatcher] Path: {:?}", path);

            // Use a single context across phases so stop_propagation works like DOM.
            let mut ctx = EventContext {
                phase: EventPhase::Capture,
                stop_propagation: false,
                stop_immediate: false,
            };

            // Phase 1: Capture (root → target)
            // println!("[dispatcher] === Capture phase ===");
            self.capture_phase(event, &path, &mut ctx);
            if ctx.stop_propagation || ctx.stop_immediate {
                // println!("[dispatcher] Stopped after capture");
                return;
            }

            // Phase 2: Target (target only)
            // println!("[dispatcher] === Target phase ===");
            self.target_phase(event, &path, target_id, &mut ctx);
            if ctx.stop_propagation || ctx.stop_immediate {
                // println!("[dispatcher] Stopped after target");
                return;
            }

            // Phase 3: Bubble (target → root)
            // println!("[dispatcher] === Bubble phase ===");
            self.bubble_phase(event, &path, &mut ctx);
        }
    }

    fn dispatch_mouse_move_with_hover(&mut self, mouse_event: &crate::event::MouseEvent) {
        use crate::event::MouseEvent;

        // Check if hover target changed
        let new_target = self.find_target_for_mouse(mouse_event);
        let old_target = self.hover_manager.hovered();

        if old_target == new_target {
            // No change, dispatch normally
            if let Some(target_id) = new_target {
                let path = self.build_path_to_target(target_id);
                let event = Event::Mouse(mouse_event.clone());
                let mut ctx = EventContext::default();

                self.capture_phase(&event, &path, &mut ctx);
                self.target_phase(&event, &path, target_id, &mut ctx);
                self.bubble_phase(&event, &path, &mut ctx);
            }
            return;
        }

        // Hover target changed - send Leave to old, Enter to new
        std::println!("[dispatcher] Hover changed: {:?} -> {:?}", old_target, new_target);

        // Send Leave to old target
        if let Some(old_id) = old_target {
            let path = self.build_path_to_target(old_id);
            let leave_event = Event::Mouse(MouseEvent {
                position: mouse_event.position,
                buttons: mouse_event.buttons,
                kind: MouseEventKind::Leave,
            });
            let mut ctx = EventContext::default();

            self.capture_phase(&leave_event, &path, &mut ctx);
            self.target_phase(&leave_event, &path, old_id, &mut ctx);
            self.bubble_phase(&leave_event, &path, &mut ctx);
        }

        // Send Enter to new target
        if let Some(new_id) = new_target {
            let path = self.build_path_to_target(new_id);
            let enter_event = Event::Mouse(MouseEvent {
                position: mouse_event.position,
                buttons: mouse_event.buttons,
                kind: MouseEventKind::Enter,
            });
            let mut ctx = EventContext::default();

            self.capture_phase(&enter_event, &path, &mut ctx);
            self.target_phase(&enter_event, &path, new_id, &mut ctx);
            self.bubble_phase(&enter_event, &path, &mut ctx);
        }

        // Update hover manager
        self.hover_manager.set_hovered(new_target);
    }

    fn find_target_for_mouse(&self, mouse_event: &crate::event::MouseEvent) -> Option<NodeId> {
        self.hit_test_recursive(self.root, mouse_event.position)
    }

    fn dispatch_keyboard(&mut self, event: &Event) {
        // Tab key: move focus
        if let Event::Key(KeyEvent::Tab) = event {
            // Save old focus BEFORE focus_next() updates it
            let old_focus_id = self.focus_manager.focused();

            if let Some(new_focus) = self.focus_manager.focus_next(self.root) {
                // Notify old focus to lose focus (if different from new)
                if let Some(old_id) = old_focus_id {
                    if old_id != new_focus {
                        if let Some(node) = self.get_node_mut(old_id) {
                            node.lose_focus();
                        }
                    }
                }

                // Notify new focus
                if let Some(node) = self.get_node_mut(new_focus) {
                    node.request_focus();
                }
            }
            return;
        }

        // Other keyboard events: only send to focused node
        // If no node has focus, keyboard events are ignored (this is intentional)
        let focused_id = match self.focus_manager.focused() {
            Some(id) => id,
            None => return, // No focus, ignore keyboard event
        };

        // Build path to focused node
        let path = self.build_path_to_target(focused_id);

        // Capture phase (root → focused node)
        let mut ctx = EventContext {
            phase: EventPhase::Capture,
            stop_propagation: false,
            stop_immediate: false,
        };

        // Walk path once from root to focused node
        self.walk_path_with_transform(event, &path, &mut ctx, /*forward=*/true);
        if ctx.stop_propagation || ctx.stop_immediate {
            return;
        }

        // Target phase (focused node only)
        ctx.phase = EventPhase::Target;
        if let Some(node) = self.get_node_mut_via_path(&path, focused_id) {
            node.handle_event(event, &mut ctx);
            if ctx.stop_propagation || ctx.stop_immediate {
                return;
            }
        }

        // Bubble phase (focused node → root)
        ctx.phase = EventPhase::Bubble;
        self.walk_path_reverse_with_transform(event, &path, &mut ctx);
    }

    fn find_target(&self, event: &Event) -> Option<NodeId> {
        let position = event.position()?;
        self.hit_test_recursive(self.root, position)
    }

    fn hit_test_recursive(&self, node: &dyn RenderObject, point: Point) -> Option<NodeId> {
        // println!("[dispatcher] hit_test_recursive: node={}, point={:?}, node.frame={:?}",
        //     node.type_name(), point, node.frame());

        // Check children first (reverse for z-order)
        for child in node.children().iter().rev() {
            let local_point = point - child.frame().origin;
            // println!("[dispatcher]   → checking child={}, child.frame={:?}, local_point={:?} (parent_point - child.origin)",
            //     child.type_name(), child.frame(), local_point);
            if let Some(id) = self.hit_test_recursive(child.as_ref(), local_point) {
                return Some(id);
            }
        }

        // Check self
        // println!("[dispatcher]   → checking self.hit_test({:?})", point);
        match node.hit_test(point) {
            HitResult::Handled(id) => {
                // println!("[dispatcher]   → Handled by id={:?}", id);
                Some(id)
            }
            HitResult::Passthrough => {
                // println!("[dispatcher]   → Passthrough");
                None
            }
            HitResult::Stop => {
                // println!("[dispatcher]   → Stop");
                Some(node.id())
            }
        }
    }

    fn capture_phase(&mut self, event: &Event, path: &[NodeId], ctx: &mut EventContext) {
        ctx.phase = EventPhase::Capture;

        // Walk path once from root to target, calling handle_event on each node
        // This is O(depth) instead of O(depth^2)
        self.walk_path_with_transform(event, path, ctx, /*forward=*/true);
    }

    fn target_phase(&mut self, event: &Event, path: &[NodeId], target_id: NodeId, ctx: &mut EventContext) {
        ctx.phase = EventPhase::Target;

        // Calculate absolute position of target node
        if let Some(absolute_frame) = self.calculate_absolute_frame(path, target_id) {
            // Transform event position to local coordinates
            if let Some(local_event) = self.transform_event_to_local(event, absolute_frame) {
                if let Some(node) = self.get_node_mut_via_path(path, target_id) {
                    node.handle_event(&local_event, ctx);
                }
                return;
            }
        }

        // Fallback: use original event
        if let Some(node) = self.get_node_mut_via_path(path, target_id) {
            node.handle_event(event, ctx);
        }
    }

    fn calculate_absolute_frame(&self, path: &[NodeId], target_id: NodeId) -> Option<crate::geometry::Rect> {
        use crate::geometry::{Point, Rect};

        let mut current_node: &dyn RenderObject = self.root;
        let mut abs_x = 0.0;
        let mut abs_y = 0.0;

        // Start from root (path[0])
        abs_x += current_node.frame().origin.x;
        abs_y += current_node.frame().origin.y;

        // Navigate to target
        for i in 1..path.len() {
            let child_id = path[i];
            match current_node.get_child(child_id) {
                Some(child) => {
                    abs_x += child.frame().origin.x;
                    abs_y += child.frame().origin.y;
                    if child.id() == target_id {
                        return Some(Rect::new(Point::new(abs_x, abs_y), child.frame().size));
                    }
                    current_node = child;
                }
                None => return None,
            }
        }

        None
    }

    fn transform_event_to_local(&self, event: &Event, absolute_frame: crate::geometry::Rect) -> Option<Event> {
        match event {
            Event::Mouse(mouse_event) => {
                let local_position = crate::geometry::Point::new(
                    mouse_event.position.x - absolute_frame.origin.x,
                    mouse_event.position.y - absolute_frame.origin.y,
                );
                Some(Event::Mouse(crate::event::MouseEvent {
                    position: local_position,
                    buttons: mouse_event.buttons,
                    kind: mouse_event.kind,
                }))
            }
            _ => Some(event.clone()),
        }
    }

    fn bubble_phase(&mut self, event: &Event, path: &[NodeId], ctx: &mut EventContext) {
        ctx.phase = EventPhase::Bubble;

        // Walk path once from target to root (reverse order)
        // This is O(depth) instead of O(depth^2)
        self.walk_path_reverse_with_transform(event, path, ctx);
    }

    /// Walk the path once from root to target, calling handle_event on each node.
    /// This is O(depth) instead of calling get_node_mut_via_path for each node (O(depth^2)).
    /// Transforms event coordinates to each node's local coordinate system.
    fn walk_path_with_transform(&mut self, event: &Event, path: &[NodeId], ctx: &mut EventContext, _forward: bool) {
        if path.is_empty() {
            return;
        }

        // Track accumulated position from root
        let mut abs_x = 0.0;
        let mut abs_y = 0.0;

        // Start from root (path[0])
        // Reborrow `self.root` so we don't move it out of `self`.
        let mut current_node: &mut dyn RenderObject = &mut *self.root;

        // Add root's frame origin
        abs_x += current_node.frame().origin.x;
        abs_y += current_node.frame().origin.y;

        // Transform event to root's local coordinates and call handle_event (inline to avoid borrow issues)
        let local_event = match event {
            Event::Mouse(mouse_event) => {
                let local_position = crate::geometry::Point::new(
                    mouse_event.position.x - abs_x,
                    mouse_event.position.y - abs_y,
                );
                Event::Mouse(crate::event::MouseEvent {
                    position: local_position,
                    buttons: mouse_event.buttons,
                    kind: mouse_event.kind,
                })
            }
            _ => event.clone(),
        };
        current_node.handle_event(&local_event, ctx);
        if ctx.stop_propagation || ctx.stop_immediate {
            return;
        }

        // Iterate through path[1..] to get to target
        // path[0] is root, path[1] is first child, etc.
        for i in 1..path.len() {
            let child_id = path[i];

            // Get child node
            match current_node.get_child_mut(child_id) {
                Some(child) => {
                    // Add child's frame origin to accumulated position
                    abs_x += child.frame().origin.x;
                    abs_y += child.frame().origin.y;

                    // Transform event to this child's local coordinates (inline to avoid borrow issues)
                    let local_event = match event {
                        Event::Mouse(mouse_event) => {
                            let local_position = crate::geometry::Point::new(
                                mouse_event.position.x - abs_x,
                                mouse_event.position.y - abs_y,
                            );
                            Event::Mouse(crate::event::MouseEvent {
                                position: local_position,
                                buttons: mouse_event.buttons,
                                kind: mouse_event.kind,
                            })
                        }
                        _ => event.clone(),
                    };

                    // Call handle_event on this child
                    child.handle_event(&local_event, ctx);
                    if ctx.stop_propagation || ctx.stop_immediate {
                        return;
                    }
                    // Move to child for next iteration
                    current_node = child;
                }
                None => return,
            }
        }
    }

    /// Transform event by subtracting accumulated position from mouse position
    fn transform_event_to_local_coords(&self, event: &Event, abs_x: f32, abs_y: f32) -> Event {
        match event {
            Event::Mouse(mouse_event) => {
                let local_position = crate::geometry::Point::new(
                    mouse_event.position.x - abs_x,
                    mouse_event.position.y - abs_y,
                );
                Event::Mouse(crate::event::MouseEvent {
                    position: local_position,
                    buttons: mouse_event.buttons,
                    kind: mouse_event.kind,
                })
            }
            _ => event.clone(),
        }
    }

    /// Walk the path once from target to root (reverse), calling handle_event on each node.
    /// Uses iterative approach with position tracking for coordinate transformation.
    /// This is O(depth) - calculates absolute positions, then processes in reverse.
    fn walk_path_reverse_with_transform(&mut self, event: &Event, path: &[NodeId], ctx: &mut EventContext) {
        if path.is_empty() {
            return;
        }

        // First pass: calculate absolute positions for all nodes in path
        let mut abs_positions: Vec<(f32, f32)> = Vec::with_capacity(path.len());
        let mut abs_x = 0.0;
        let mut abs_y = 0.0;

        // Start from root (path[0])
        let mut current_node: &dyn RenderObject = self.root;
        abs_x += current_node.frame().origin.x;
        abs_y += current_node.frame().origin.y;
        abs_positions.push((abs_x, abs_y));

        // Navigate to target, accumulating positions
        for i in 1..path.len() {
            let child_id = path[i];
            match current_node.get_child(child_id) {
                Some(child) => {
                    abs_x += child.frame().origin.x;
                    abs_y += child.frame().origin.y;
                    abs_positions.push((abs_x, abs_y));
                    current_node = child;
                }
                None => return,
            }
        }

        // Second pass: process nodes in reverse order (target → ... → root)
        // We need to get mutable references, so we traverse again
        let mut current_node: &mut dyn RenderObject = &mut *self.root;

        // Process all nodes except root in reverse order
        for i in (1..path.len()).rev() {
            let child_id = path[i];

            match current_node.get_child_mut(child_id) {
                Some(child) => {
                    // Get absolute position for this node
                    let (node_abs_x, node_abs_y) = abs_positions[i];

                    // Transform event to this node's local coordinates (inline to avoid borrow issues)
                    let local_event = match event {
                        Event::Mouse(mouse_event) => {
                            let local_position = crate::geometry::Point::new(
                                mouse_event.position.x - node_abs_x,
                                mouse_event.position.y - node_abs_y,
                            );
                            Event::Mouse(crate::event::MouseEvent {
                                position: local_position,
                                buttons: mouse_event.buttons,
                                kind: mouse_event.kind,
                            })
                        }
                        _ => event.clone(),
                    };

                    // Call handle_event
                    child.handle_event(&local_event, ctx);
                    if ctx.stop_propagation || ctx.stop_immediate {
                        return;
                    }

                    // Move to child for next iteration
                    current_node = child;
                }
                None => return,
            }
        }

        // Process root last (if not stopped)
        if !(ctx.stop_propagation || ctx.stop_immediate) {
            let (root_abs_x, root_abs_y) = abs_positions[0];
            let local_event = match event {
                Event::Mouse(mouse_event) => {
                    let local_position = crate::geometry::Point::new(
                        mouse_event.position.x - root_abs_x,
                        mouse_event.position.y - root_abs_y,
                    );
                    Event::Mouse(crate::event::MouseEvent {
                        position: local_position,
                        buttons: mouse_event.buttons,
                        kind: mouse_event.kind,
                    })
                }
                _ => event.clone(),
            };
            current_node.handle_event(&local_event, ctx);
        }
    }

    fn build_path_to_target(&self, target_id: NodeId) -> Vec<NodeId> {
        let mut path = Vec::new();
        let mut current_id = Some(target_id);

        // println!("[dispatcher] build_path_to_target: starting from target_id={:?}", target_id);

        while let Some(id) = current_id {
            path.push(id);
            match self.get_node(id) {
                Some(node) => {
                    // println!("[dispatcher] build_path_to_target: id={:?}, type_name={}, parent={:?}", id, node.type_name(), node.parent());
                    current_id = node.parent();
                }
                None => {
                    // println!("[dispatcher] build_path_to_target: id={:?} NOT FOUND in tree", id);
                    current_id = None;
                }
            }
        }

        path.reverse(); // Root → target
        // println!("[dispatcher] build_path_to_target: final path={:?}", path);
        path
    }

    fn get_node(&self, id: NodeId) -> Option<&dyn RenderObject> {
        // Direct traversal using parent pointers
        fn find<'a>(node: &'a dyn RenderObject, target_id: NodeId) -> Option<&'a dyn RenderObject> {
            if node.id() == target_id {
                return Some(node);
            }
            for child in node.children() {
                if let Some(found) = find(child.as_ref(), target_id) {
                    return Some(found);
                }
            }
            None
        }
        find(self.root, id)
    }

    fn get_node_mut(&mut self, id: NodeId) -> Option<&mut dyn RenderObject> {
        // Build path first, then navigate using path-based approach
        let path = self.build_path_to_target(id);
        if path.is_empty() {
            None
        } else {
            self.get_node_mut_via_path(&path, id)
        }
    }

    fn get_node_mut_via_path(&mut self, path: &[NodeId], target: NodeId) -> Option<&mut dyn RenderObject> {
        let prefix_end = path.iter().position(|&id| id == target)? + 1;
        let prefix = &path[..prefix_end];
        self.get_node_mut_via_prefix(prefix)
    }

    fn get_node_mut_via_prefix(&mut self, path: &[NodeId]) -> Option<&mut dyn RenderObject> {
        // path is [root, child1, child2, ..., target]
        // Start traversal from root, but skip matching root itself
        fn traverse<'a>(node: &'a mut dyn RenderObject, path: &[NodeId], index: usize) -> Option<&'a mut dyn RenderObject> {
            if index >= path.len() {
                return Some(node);
            }

            // path[index] is the next node to traverse to
            let child_id = path[index];
            let child = node.get_child_mut(child_id)?;
            traverse(child, path, index + 1)
        }

        if path.is_empty() {
            return Some(self.root);
        }

        // path[0] is root, so start from path[1]
        traverse(self.root, path, 1)
    }
}

trait EventPosition {
    fn position(&self) -> Option<Point>;
}

impl EventPosition for Event {
    fn position(&self) -> Option<Point> {
        match self {
            Event::Mouse(e) => Some(e.position),
            _ => None,
        }
    }
}
