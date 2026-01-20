use crate::event::{Event, EventContext, EventPhase, HitResult};
use crate::geometry::Point;
use crate::node_id::NodeId;
use crate::traits::RenderNode;
use std::vec::Vec;

pub struct EventDispatcher<'a> {
    root: &'a mut dyn RenderNode,
}

impl<'a> EventDispatcher<'a> {
    pub fn new(root: &'a mut dyn RenderNode) -> Self {
        Self { root }
    }

    pub fn dispatch(&mut self, event: &Event) {
        // Find target via hit test
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

    fn get_node_mut_via_path(&mut self, path: &[NodeId], target: NodeId) -> Option<&mut dyn RenderNode> {
        let prefix_end = path.iter().position(|&id| id == target)? + 1;
        let prefix = &path[..prefix_end];
        self.get_node_mut_via_prefix(prefix)
    }

    fn get_node_mut_via_prefix(&mut self, path: &[NodeId]) -> Option<&mut dyn RenderNode> {
        // Use raw pointer to avoid self-borrow issues
        // This is safe because we're extending the lifetime of the mutable borrow
        // but ensuring we only return a reference that doesn't alias with anything
        let mut current = self.root as *mut dyn RenderNode;

        for id in path {
            let node_ref = unsafe { &mut *current };
            if node_ref.id() == *id {
                continue;
            }
            match node_ref.get_child_mut(*id) {
                Some(child) => current = child as *mut dyn RenderNode,
                None => return None,
            }
        }

        Some(unsafe { &mut *current })
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
