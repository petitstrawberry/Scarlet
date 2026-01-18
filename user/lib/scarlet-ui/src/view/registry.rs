//! View registry for O(1) view lookup
//!
//! ViewRegistry provides a flat registry of all views in the view tree,
//! enabling efficient lookup by ViewId. This is essential for:
//! - O(1) dirty tracking
//! - Event targeting
//! - Cross-view references

use core::sync::atomic::{AtomicU64, Ordering};
use crate::view::id::ViewId;
use scarlet_std::collections::HashMap;

/// Registry of all views in the view tree
///
/// ViewRegistry maintains a flat map of ViewId → ViewNode, enabling
/// efficient lookup without traversing the view tree.
pub struct ViewRegistry {
    /// Map of ViewId → ViewNode
    nodes: HashMap<ViewId, ViewNodeEntry>,
    /// Next ViewId to assign
    next_id: AtomicU64,
}

/// Entry in the view registry
///
/// This is a placeholder for now. Once ViewNode is defined,
/// this will contain the actual view node.
#[derive(Debug)]
pub struct ViewNodeEntry {
    pub id: ViewId,
    pub parent: Option<ViewId>,
    // More fields will be added when ViewNode is defined
}

impl ViewNodeEntry {
    pub fn new(id: ViewId, parent: Option<ViewId>) -> Self {
        Self { id, parent }
    }
}

impl ViewRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            next_id: AtomicU64::new(1),
        }
    }

    /// Generate a new unique ViewId
    pub fn generate_id(&self) -> ViewId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        ViewId::from_u64(id)
    }

    /// Insert a view node into the registry
    ///
    /// Returns the ViewId that was assigned to the node.
    pub fn insert(&mut self, entry: ViewNodeEntry) -> ViewId {
        let id = entry.id;
        self.nodes.insert(id, entry);
        id
    }

    /// Get a view node by ID
    pub fn get(&self, id: ViewId) -> Option<&ViewNodeEntry> {
        self.nodes.get(&id)
    }

    /// Get a mutable view node by ID
    pub fn get_mut(&mut self, id: ViewId) -> Option<&mut ViewNodeEntry> {
        self.nodes.get_mut(&id)
    }

    /// Remove a view node from the registry
    pub fn remove(&mut self, id: ViewId) -> Option<ViewNodeEntry> {
        self.nodes.remove(&id)
    }

    /// Check if a view ID exists in the registry
    pub fn contains(&self, id: ViewId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Get the number of views in the registry
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Get all view IDs
    pub fn ids(&self) -> scarlet_std::vec::Vec<ViewId> {
        self.nodes.keys().copied().collect()
    }

    /// Clear all views from the registry
    pub fn clear(&mut self) {
        self.nodes.clear();
    }
}

impl Default for ViewRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_insert_get() {
        let mut registry = ViewRegistry::new();
        let id = ViewId::new();
        let entry = ViewNodeEntry::new(id, None);

        registry.insert(entry);
        assert!(registry.contains(id));

        let retrieved = registry.get(id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, id);
    }

    #[test]
    fn test_registry_generate_unique_ids() {
        let registry = ViewRegistry::new();
        let id1 = registry.generate_id();
        let id2 = registry.generate_id();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_registry_remove() {
        let mut registry = ViewRegistry::new();
        let id = ViewId::new();
        let entry = ViewNodeEntry::new(id, None);

        registry.insert(entry);
        assert!(registry.contains(id));

        registry.remove(id);
        assert!(!registry.contains(id));
    }

    #[test]
    fn test_registry_len() {
        let mut registry = ViewRegistry::new();
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());

        let id1 = ViewId::new();
        let entry1 = ViewNodeEntry::new(id1, None);
        registry.insert(entry1);

        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
    }
}
