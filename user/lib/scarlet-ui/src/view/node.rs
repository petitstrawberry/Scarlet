//! View node registry for buffer management
//!
//! Each View can have its own buffer for efficient partial updates.
//! Window maintains a flat registry of all views for O(1) dirty tracking.

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

/// Per-view backbuffer (BGRA format, 4 bytes per pixel)
pub struct ViewBuffer {
    data: Vec<u8>,
    width: u32,
    height: u32,
}

impl ViewBuffer {
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width * height * 4) as usize;
        let mut data = Vec::new();
        data.resize(size, 0);
        Self { data, width, height }
    }

    pub fn resize_if_needed(&mut self, width: u32, height: u32) -> bool {
        if width != self.width || height != self.height {
            let size = (width * height * 4) as usize;
            self.data.resize(size, 0);
            self.width = width;
            self.height = height;
            true
        } else {
            false
        }
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

/// A node in the flat view registry
pub struct ViewNode {
    pub id: ViewId,
    pub frame: Rect,
    pub z_order: ZOrder,
    pub buffer: Option<ViewBuffer>,
    pub buffer_valid: bool,
}

impl ViewNode {
    pub fn new(id: ViewId, frame: Rect, z_order: ZOrder) -> Self {
        Self {
            id,
            frame,
            z_order,
            buffer: None,
            buffer_valid: false,
        }
    }

    pub fn ensure_buffer(&mut self) -> &mut ViewBuffer {
        if self.buffer.is_none() || !self.buffer_valid {
            self.buffer = Some(ViewBuffer::new(self.frame.width, self.frame.height));
            self.buffer_valid = true;
        }
        self.buffer.as_mut().unwrap()
    }

    pub fn invalidate_buffer(&mut self) {
        self.buffer_valid = false;
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
}
