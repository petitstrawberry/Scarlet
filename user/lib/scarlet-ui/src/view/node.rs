//! View node registry for buffer management
//!
//! Window maintains a flat registry of all views for O(1) dirty tracking.
//! Each View manages its own buffer (see buffer.rs).

use scarlet_std::vec::Vec;
use scarlet_std::sync::{Arc, Mutex};
use std::collections::HashSet;
use crate::graphics::Rect;

/// ViewからWindowへのdirty通知
#[derive(Clone)]
pub struct DirtyNotifier {
    dirty_ids: Arc<Mutex<Vec<ViewId>>>,
}

impl DirtyNotifier {
    pub fn new() -> Self {
        Self {
            dirty_ids: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn mark_dirty(&self, id: ViewId) {
        self.dirty_ids.lock().push(id);
    }

    pub fn take_dirty(&self) -> Vec<ViewId> {
        core::mem::take(&mut self.dirty_ids.lock())
    }
}

/// Unique identifier for a view
pub type ViewId = u64;

/// Z-order for composition (higher values drawn on top)
pub type ZOrder = u32;

/// A node in the flat view registry
/// Note: Each View manages its own buffer (see super::buffer::ViewBuffer)
pub struct ViewNode {
    pub id: ViewId,
    pub parent_id: Option<ViewId>,  // Parent view ID (for propagating dirty state)
    pub frame: Rect,
    pub z_order: ZOrder,
}

impl ViewNode {
    pub fn new(id: ViewId, frame: Rect, z_order: ZOrder) -> Self {
        Self {
            id,
            parent_id: None,
            frame,
            z_order,
        }
    }

    pub fn with_parent(mut self, parent_id: ViewId) -> Self {
        self.parent_id = Some(parent_id);
        self
    }
}

/// Flat registry of all views in a window
pub struct ViewRegistry {
    nodes: Vec<ViewNode>,
    dirty_ids: HashSet<ViewId>,
    next_id: ViewId,
}

impl ViewRegistry {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            dirty_ids: HashSet::new(),
            next_id: 1,
        }
    }

    pub fn register(&mut self, frame: Rect, z_order: ZOrder) -> ViewId {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.nodes.push(ViewNode::new(id, frame, z_order));
        id
    }

    pub fn mark_dirty(&mut self, id: ViewId) {
        self.dirty_ids.insert(id);
    }

    pub fn get_dirty_ids(&self) -> &HashSet<ViewId> {
        &self.dirty_ids
    }

    pub fn clear_dirty(&mut self) {
        self.dirty_ids.clear();
    }

    pub fn get_node_mut(&mut self, id: ViewId) -> Option<&mut ViewNode> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn get_node(&self, id: ViewId) -> Option<&ViewNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ViewNode> {
        self.nodes.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut ViewNode> {
        self.nodes.iter_mut()
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.dirty_ids.clear();
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Find view ID by frame position (for second-pass ID assignment)
    pub fn find_id_by_frame(&self, frame: Rect) -> Option<ViewId> {
        self.nodes.iter().find(|n| n.frame == frame).map(|n| n.id)
    }
}
