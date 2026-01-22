use std::collections::HashSet;
use std::vec::Vec;

use crate::dirty::DirtyFlags;
use crate::traits::{Element, ElementId, RenderObject};

/// PipelineOwner manages the rendering pipeline
/// Based on Flutter's PipelineOwner which manages dirty nodes and scheduling
pub struct PipelineOwner {
    /// Elements that need layout
    nodes_needing_layout: HashSet<ElementId>,

    /// Elements that need paint
    nodes_needing_paint: HashSet<ElementId>,

    /// Whether a flush is pending
    flush_pending: bool,
}

impl PipelineOwner {
    pub fn new() -> Self {
        Self {
            nodes_needing_layout: HashSet::new(),
            nodes_needing_paint: HashSet::new(),
            flush_pending: false,
        }
    }

    /// Mark an element as needing layout
    pub fn request_layout(&mut self, element_id: ElementId) {
        self.nodes_needing_layout.insert(element_id);
    }

    /// Mark an element as needing paint
    pub fn request_paint(&mut self, element_id: ElementId) {
        self.nodes_needing_paint.insert(element_id);
    }

    /// Flush layout for all nodes that need it
    pub fn flush_layout(&mut self, root: &mut dyn Element) {
        if self.nodes_needing_layout.is_empty() {
            return;
        }

        // Collect IDs to avoid borrow checker issues
        let ids: Vec<_> = self.nodes_needing_layout.iter().copied().collect();

        for element_id in ids {
            if let Some(element) = find_element_in_tree_mut(root, element_id) {
                let render_object = element.get_render_object_mut();
                render_object.mark_dirty(DirtyFlags::LAYOUT);
            }
        }

        self.nodes_needing_layout.clear();
    }

    /// Flush paint for all nodes that need it
    pub fn flush_paint(&mut self, root: &mut dyn Element) {
        if self.nodes_needing_paint.is_empty() {
            return;
        }

        // Collect IDs to avoid borrow checker issues
        let ids: Vec<_> = self.nodes_needing_paint.iter().copied().collect();

        for element_id in ids {
            if let Some(element) = find_element_in_tree_mut(root, element_id) {
                let render_object = element.get_render_object_mut();
                render_object.mark_dirty(DirtyFlags::PAINT);
            }
        }

        self.nodes_needing_paint.clear();
    }

    /// Check if any work is pending
    pub fn has_work(&self) -> bool {
        !self.nodes_needing_layout.is_empty() || !self.nodes_needing_paint.is_empty()
    }
}

/// Find an element by ID in the tree (mutable)
/// Standalone function to avoid borrow checker issues
fn find_element_in_tree_mut(element: &mut dyn Element, id: ElementId) -> Option<&mut dyn Element> {
    if element.id() == id {
        return Some(element);
    }

    for child in element.children_mut() {
        if let Some(found) = find_element_in_tree_mut(child.as_mut(), id) {
            return Some(found);
        }
    }

    None
}

impl Default for PipelineOwner {
    fn default() -> Self {
        Self::new()
    }
}
