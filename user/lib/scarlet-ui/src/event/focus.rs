use crate::node_id::NodeId;
use crate::traits::RenderNode;
use std::vec::Vec;

/// Manages hover state for the UI tree.
///
/// Tracks which node is currently hovered and sends MouseLeave/MouseEnter events
/// when the hover target changes.
pub struct HoverManager {
    hovered: Option<NodeId>,
}

impl HoverManager {
    pub fn new() -> Self {
        Self {
            hovered: None,
        }
    }

    /// Get the currently hovered node
    pub fn hovered(&self) -> Option<NodeId> {
        self.hovered
    }

    /// Update hover state when mouse moves
    ///
    /// Returns (old_hovered, new_hovered) if changed, None otherwise
    pub fn update_hover(&mut self, new_target: Option<NodeId>) -> Option<(Option<NodeId>, Option<NodeId>)> {
        if self.hovered != new_target {
            let old = self.hovered;
            self.hovered = new_target;
            Some((old, new_target))
        } else {
            None
        }
    }

    /// Set hovered node directly (for testing or manual control)
    pub fn set_hovered(&mut self, node_id: Option<NodeId>) {
        self.hovered = node_id;
    }
}

impl Default for HoverManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Manages focus state for the UI tree.
///
/// ## Focus State Synchronization
///
/// The focus system has two parts that must be kept in sync:
/// 1. **FocusManager.focused**: The canonical source of which node has focus
/// 2. **RenderNode.interaction_state.focused**: Each node's local focus state
///
/// When focus changes, BOTH must be updated:
/// - Call `FocusManager::set_focus()` to update the manager
/// - Call `node.request_focus()` on the new focused node
/// - Call `node.lose_focus()` on the old focused node
///
/// ## Focus Change Protocol
///
/// To change focus safely:
/// ```rust
/// let old_focus = focus_manager.focused();
/// let new_focus = some_node_id;
///
/// // Step 1: Update manager
/// focus_manager.set_focus(new_focus);
///
/// // Step 2: Update node states
/// if let Some(old_id) = old_focus {
///     if old_id != new_focus {
///         get_node_mut(old_id)?.lose_focus();
///     }
/// }
/// get_node_mut(new_focus)?.request_focus();
/// ```
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

    /// Get the currently focused node (canonical source)
    pub fn focused(&self) -> Option<NodeId> {
        self.focused
    }

    /// Set focus to a specific node.
    ///
    /// **Important**: After calling this, you must also:
    /// - Call `request_focus()` on the new focused node
    /// - Call `lose_focus()` on the old focused node
    ///
    /// See module-level documentation for the full protocol.
    pub fn set_focus(&mut self, node_id: NodeId) {
        if self.focused != Some(node_id) {
            self.focused = Some(node_id);
        }
    }

    /// Clear focus (no node has focus).
    ///
    /// **Important**: After calling this, you must call
    /// `lose_focus()` on the previously focused node.
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
