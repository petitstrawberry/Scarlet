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

            // Phase 1: Capture (root → target)
            self.capture_phase(event, target_id);

            // Phase 2: Target
            self.target_phase(event, target_id);

            // Phase 3: Bubble (target → root)
            self.bubble_phase(event, target_id);
        }
    }

    fn dispatch_keyboard(&mut self, event: &Event) {
        // Check for Tab key to move focus
        if let Event::Key(KeyEvent::Tab) = event {
            if let Some(new_focus) = self.focus_manager.focus_next(self.root) {
                // Notify old focus to lose focus
                if let Some(old_focus_id) = self.focus_manager.focused() {
                    if old_focus_id != new_focus {
                        if let Some(node) = self.get_node_mut(old_focus_id) {
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

        // Get focused node
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

        for node_id in &path {
            if let Some(node) = self.get_node_mut_via_path(&path, *node_id) {
                node.handle_event(event, &mut ctx);
                if ctx.stop_propagation || ctx.stop_immediate {
                    return;
                }
            }
        }

        // Target phase (focused node only)
        ctx.phase = EventPhase::Target;
        if let Some(node) = self.get_node_mut_via_path(&path, focused_id) {
            node.handle_event(event, &mut ctx);
            if ctx.stop_propagation {
                return;
            }
        }

        // Bubble phase (focused node → root)
        ctx.phase = EventPhase::Bubble;
        for node_id in path.iter().rev() {
            if let Some(node) = self.get_node_mut_via_path(&path, *node_id) {
                node.handle_event(event, &mut ctx);
                if ctx.stop_propagation {
                    return;
                }
            }
        }
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

    fn capture_phase(&mut self, event: &Event, target_id: NodeId) {
        let path = self.build_path_to_target(target_id);

        let mut ctx = EventContext {
            phase: EventPhase::Capture,
            stop_propagation: false,
            stop_immediate: false,
        };

        // Path is root → target, iterate in order
        for node_id in &path {
            if let Some(node) = self.get_node_mut_via_path(&path, *node_id) {
                node.handle_event(event, &mut ctx);
                if ctx.stop_propagation || ctx.stop_immediate {
                    break;
                }
            }
        }
    }

    fn target_phase(&mut self, event: &Event, target_id: NodeId) {
        let path = self.build_path_to_target(target_id);

        let mut ctx = EventContext {
            phase: EventPhase::Target,
            stop_propagation: false,
            stop_immediate: false,
        };

        // Target is last element in path
        if let Some(node) = self.get_node_mut_via_path(&path, target_id) {
            node.handle_event(event, &mut ctx);
        }
    }

    fn bubble_phase(&mut self, event: &Event, target_id: NodeId) {
        let path = self.build_path_to_target(target_id);

        let mut ctx = EventContext {
            phase: EventPhase::Bubble,
            stop_propagation: false,
            stop_immediate: false,
        };

        // Path is root → target, reverse for bubble
        for node_id in path.iter().rev() {
            if let Some(node) = self.get_node_mut_via_path(&path, *node_id) {
                node.handle_event(event, &mut ctx);
                if ctx.stop_propagation {
                    break;
                }
            }
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
        // Safe traversal without unsafe: use recursive helper that properly
        // hands over mutable references at each level
        fn traverse<'a>(node: &'a mut dyn RenderNode, path: &[NodeId], index: usize) -> Option<&'a mut dyn RenderNode> {
            if index >= path.len() {
                return Some(node);
            }

            let target_id = path[index];

            // If current node matches, skip to next
            if node.id() == target_id {
                return traverse(node, path, index + 1);
            }

            // Get child and continue traversal
            let child = node.get_child_mut(target_id)?;
            traverse(child, path, index + 1)
        }

        traverse(self.root, path, 0)
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
