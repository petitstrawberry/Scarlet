use crate::event::{Event, EventContext, EventPhase, HitResult, KeyEvent};
use crate::geometry::Point;
use crate::node_id::NodeId;
use crate::traits::RenderNode;
use std::vec::Vec;

pub struct EventDispatcher<'a> {
    root: &'a mut dyn RenderNode,
    focus_manager: &'a mut crate::event::FocusManager,
}

impl<'a> EventDispatcher<'a> {
    pub fn new(root: &'a mut dyn RenderNode, focus_manager: &'a mut crate::event::FocusManager) -> Self {
        Self { root, focus_manager }
    }

    pub fn dispatch(&mut self, event: &Event) {
        if event.is_keyboard() {
            self.dispatch_keyboard(event);
        } else {
            // Mouse events - find target via hit test
            let target_id = match self.find_target(event) {
                Some(id) => id,
                None => return, // No target, ignore event
            };

            // Build path once (root → target)
            let path = self.build_path_to_target(target_id);

            // Use a single context across phases so stop_propagation works like DOM.
            let mut ctx = EventContext {
                phase: EventPhase::Capture,
                stop_propagation: false,
                stop_immediate: false,
            };

            // Phase 1: Capture (root → target)
            self.capture_phase(event, &path, &mut ctx);
            if ctx.stop_propagation || ctx.stop_immediate {
                return;
            }

            // Phase 2: Target (target only)
            self.target_phase(event, &path, target_id, &mut ctx);
            if ctx.stop_propagation || ctx.stop_immediate {
                return;
            }

            // Phase 3: Bubble (target → root)
            self.bubble_phase(event, &path, &mut ctx);
        }
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
        self.walk_path(&path, event, &mut ctx, /*forward=*/true);
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
        self.walk_path_reverse(&path, event, &mut ctx);
    }

    fn find_target(&self, event: &Event) -> Option<NodeId> {
        let position = event.position()?;
        self.hit_test_recursive(self.root, position)
    }

    fn hit_test_recursive(&self, node: &dyn RenderNode, point: Point) -> Option<NodeId> {
        // Check children first (reverse for z-order)
        for child in node.children().iter().rev() {
            let local_point = point - child.frame().origin;
            if let Some(id) = self.hit_test_recursive(child.as_ref(), local_point) {
                return Some(id);
            }
        }

        // Check self
        match node.hit_test(point) {
            HitResult::Handled(id) => Some(id),
            HitResult::Passthrough => None,
            HitResult::Stop => Some(node.id()),
        }
    }

    fn capture_phase(&mut self, event: &Event, path: &[NodeId], ctx: &mut EventContext) {
        ctx.phase = EventPhase::Capture;

        // Walk path once from root to target, calling handle_event on each node
        // This is O(depth) instead of O(depth^2)
        self.walk_path(path, event, ctx, /*forward=*/true);
    }

    fn target_phase(&mut self, event: &Event, path: &[NodeId], target_id: NodeId, ctx: &mut EventContext) {
        ctx.phase = EventPhase::Target;

        // Target is last element in path
        if let Some(node) = self.get_node_mut_via_path(path, target_id) {
            node.handle_event(event, ctx);
        }
    }

    fn bubble_phase(&mut self, event: &Event, path: &[NodeId], ctx: &mut EventContext) {
        ctx.phase = EventPhase::Bubble;

        // Walk path once from target to root (reverse order)
        // This is O(depth) instead of O(depth^2)
        self.walk_path_reverse(path, event, ctx);
    }

    /// Walk the path once from root to target, calling handle_event on each node.
    /// This is O(depth) instead of calling get_node_mut_via_path for each node (O(depth^2)).
    fn walk_path(&mut self, path: &[NodeId], event: &Event, ctx: &mut EventContext, _forward: bool) {
        if path.is_empty() {
            return;
        }

        // Start from root (path[0])
        // Reborrow `self.root` so we don't move it out of `self`.
        let mut current_node: &mut dyn RenderNode = &mut *self.root;

        // Call handle_event on root first
        current_node.handle_event(event, ctx);
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
                    // Call handle_event on this child
                    child.handle_event(event, ctx);
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

    /// Walk the path once from target to root (reverse), calling handle_event on each node.
    /// Uses recursive post-order traversal to avoid multiple mutable borrows.
    /// This is O(depth) - descends to target, then calls handle_event on way back.
    fn walk_path_reverse(&mut self, path: &[NodeId], event: &Event, ctx: &mut EventContext) {
        if path.is_empty() {
            return;
        }

        fn walk_path_reverse_recursive(
            parent: &mut dyn RenderNode,
            path: &[NodeId],
            index: usize,
            event: &Event,
            ctx: &mut EventContext,
        ) {
            if index >= path.len() {
                // Reached past target (leaf), start bubbling back
                return;
            }

            let child_id = path[index];
            let child = match parent.get_child_mut(child_id) {
                Some(c) => c,
                None => return,
            };

            // First, recurse deeper (descent to target)
            walk_path_reverse_recursive(child, path, index + 1, event, ctx);

            // After returning from recursion, check if propagation was stopped
            // If stopped in deeper nodes, don't call handle_event on this node
            if ctx.stop_propagation || ctx.stop_immediate {
                return;
            }

            // Handle event on this child (post-order)
            // This processes nodes in reverse: target first, then parents on way back
            child.handle_event(event, ctx);
        }

        // Start recursive descent from root
        // path[0] is root, path[1] is first child, etc.
        // For bubble phase, we need to process in reverse: target → ... → child1 → root
        // Note: Root IS processed in bubble phase (DOM standard behavior)
        let root: &mut dyn RenderNode = &mut *self.root;
        walk_path_reverse_recursive(root, path, 1, event, ctx);

        // After processing all descendants, handle root (last in bubble phase)
        if !(ctx.stop_propagation || ctx.stop_immediate) {
            root.handle_event(event, ctx);
        }
    }

    fn build_path_to_target(&self, target_id: NodeId) -> Vec<NodeId> {
        let mut path = Vec::new();
        let mut current_id = Some(target_id);

        while let Some(id) = current_id {
            path.push(id);
            current_id = self.get_node(id).and_then(|n| n.parent());
        }

        path.reverse(); // Root → target
        path
    }

    fn get_node(&self, id: NodeId) -> Option<&dyn RenderNode> {
        if self.root.id() == id {
            Some(self.root)
        } else {
            self.root.get_child(id)
        }
    }

    fn get_node_mut(&mut self, id: NodeId) -> Option<&mut dyn RenderNode> {
        if self.root.id() == id {
            Some(self.root)
        } else {
            self.root.get_child_mut(id)
        }
    }

    fn get_node_mut_via_path(&mut self, path: &[NodeId], target: NodeId) -> Option<&mut dyn RenderNode> {
        let prefix_end = path.iter().position(|&id| id == target)? + 1;
        let prefix = &path[..prefix_end];
        self.get_node_mut_via_prefix(prefix)
    }

    fn get_node_mut_via_prefix(&mut self, path: &[NodeId]) -> Option<&mut dyn RenderNode> {
        // path is [root, child1, child2, ..., target]
        // Start traversal from root, but skip matching root itself
        fn traverse<'a>(node: &'a mut dyn RenderNode, path: &[NodeId], index: usize) -> Option<&'a mut dyn RenderNode> {
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
