use crate::node_id::NodeId;
use crate::traits::RenderNode;

/// Manages focus state for the UI tree
pub struct FocusManager {
    focused: Option<NodeId>,
    focus_chain: Vec<NodeId>,
}

impl FocusManager {
    pub fn new() -> Self {
        Self {
            focused: None,
            focus_chain: Vec::new(),
        }
    }

    /// Get the currently focused node
    pub fn focused(&self) -> Option<NodeId> {
        self.focused
    }

    /// Set focus to a specific node
    pub fn set_focus(&mut self, node_id: NodeId) {
        if self.focused != Some(node_id) {
            self.focused = Some(node_id);
        }
    }

    /// Clear focus (no node has focus)
    pub fn clear_focus(&mut self) {
        self.focused = None;
    }

    /// Move focus to next focusable node
    pub fn focus_next(&mut self, root: &dyn RenderNode) -> Option<NodeId> {
        self.build_focus_chain(root);

        if self.focus_chain.is_empty() {
            return None;
        }

        let current_idx = self.focused
            .and_then(|id| self.focus_chain.iter().position(|&nid| nid == id))
            .unwrap_or(self.focus_chain.len() - 1);

        let next_idx = (current_idx + 1) % self.focus_chain.len();
        let next_id = self.focus_chain[next_idx];
        self.set_focus(next_id);
        Some(next_id)
    }

    /// Move focus to previous focusable node
    pub fn focus_prev(&mut self, root: &dyn RenderNode) -> Option<NodeId> {
        self.build_focus_chain(root);

        if self.focus_chain.is_empty() {
            return None;
        }

        let current_idx = self.focused
            .and_then(|id| self.focus_chain.iter().position(|&nid| nid == id))
            .unwrap_or(0);

        let prev_idx = if current_idx == 0 {
            self.focus_chain.len() - 1
        } else {
            current_idx - 1
        };
        let prev_id = self.focus_chain[prev_idx];
        self.set_focus(prev_id);
        Some(prev_id)
    }

    /// Build the focus chain (order of focusable nodes)
    fn build_focus_chain(&mut self, root: &dyn RenderNode) {
        self.focus_chain.clear();
        self.collect_focusable(root);
    }

    /// Recursively collect focusable nodes
    fn collect_focusable(&mut self, node: &dyn RenderNode) {
        if node.is_focusable() {
            self.focus_chain.push(node.id());
        }

        for child in node.children() {
            self.collect_focusable(child.as_ref());
        }
    }
}

impl Default for FocusManager {
    fn default() -> Self {
        Self::new()
    }
}
