//! View registry for O(1) view lookup
//!
//! ViewRegistry maintains the view tree structure and provides efficient
//! access to views by ViewId. This enables:
//! - O(1) view lookup
//! - Parent-child traversal
//! - Dirty tracking integration
//! - Event dispatch

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::view::id::ViewId;
use crate::view::traits::View;
use crate::layout::Size;
use crate::graphics::Rect;
use scarlet_std::collections::HashMap;

/// Registry of all views in the view tree
///
/// ViewRegistry maintains a flat map of ViewId → ViewEntry, enabling
/// efficient lookup without traversing the view tree.
pub struct ViewRegistry {
    /// Map of ViewId → ViewEntry
    entries: HashMap<ViewId, ViewEntry>,
    /// Next ViewId to assign
    next_id: AtomicU64,
}

/// Entry in the view registry
///
/// Contains the actual View and metadata about its position in the tree.
pub struct ViewEntry {
    /// The actual view (None if temporarily checked out)
    view: Option<Box<dyn View>>,
    /// Parent view ID
    pub parent: Option<ViewId>,
    /// Child view IDs
    pub children: Vec<ViewId>,
    /// Cached size from last layout
    pub cached_size: Size,
    /// Cached frame from last layout
    pub cached_frame: Option<Rect>,
    /// Whether this entry is a Window (root view)
    pub is_window: bool,
}

impl ViewRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            next_id: AtomicU64::new(1),
        }
    }

    /// Generate a new unique ViewId
    pub fn generate_id(&self) -> ViewId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        ViewId::from_u64(id)
    }

    /// Register a single view (without children)
    ///
    /// After registering, use add_child() to build the tree structure.
    pub fn register(&mut self, view: Box<dyn View>) -> ViewId {
        let id = view.id();
        let is_window = Self::check_is_window(&view);

        self.entries.insert(id, ViewEntry {
            view: Some(view),
            parent: None,
            children: Vec::new(),
            cached_size: Size::ZERO,
            cached_frame: None,
            is_window,
        });

        id
    }

    /// Add a child to a parent view
    pub fn add_child(&mut self, parent_id: ViewId, child_id: ViewId) {
        if let Some(entry) = self.entries.get_mut(&parent_id) {
            entry.children.push(child_id);
        }
        if let Some(entry) = self.entries.get_mut(&child_id) {
            entry.parent = Some(parent_id);
        }
    }

    /// Get a view by ID (immutable)
    pub fn get(&self, id: ViewId) -> Option<&Box<dyn View>> {
        self.entries.get(&id)?.view.as_ref()
    }

    /// Get a view by ID (mutable)
    pub fn get_mut(&mut self, id: ViewId) -> Option<&mut Box<dyn View>> {
        self.entries.get_mut(&id)?.view.as_mut()
    }

    /// Get the parent of a view
    pub fn get_parent(&self, id: ViewId) -> Option<ViewId> {
        self.entries.get(&id)?.parent
    }

    /// Get the children of a view
    pub fn get_children(&self, id: ViewId) -> &[ViewId] {
        if let Some(entry) = self.entries.get(&id) {
            &entry.children
        } else {
            &[]
        }
    }

    /// Check if a view is a Window
    pub fn is_window(&self, id: ViewId) -> bool {
        self.entries.get(&id).map(|e| e.is_window).unwrap_or(false)
    }

    /// Find the parent Window of a view
    ///
    /// Traverses up the tree until a Window is found.
    pub fn find_parent_window(&self, id: ViewId) -> ViewId {
        let mut current = id;
        while let Some(parent) = self.get_parent(current) {
            if self.is_window(parent) {
                return parent;
            }
            current = parent;
        }
        id  // Assume we are a Window if no parent found
    }

    /// Get the cached frame for a view
    pub fn get_frame(&self, id: ViewId) -> Option<Rect> {
        self.entries.get(&id)?.cached_frame
    }

    /// Set the cached frame for a view
    pub fn set_frame(&mut self, id: ViewId, frame: Rect) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.cached_frame = Some(frame);
        }
    }

    /// Get the cached size for a view
    pub fn get_size(&self, id: ViewId) -> Option<Size> {
        self.entries.get(&id).map(|e| e.cached_size)
    }

    /// Set the cached size for a view
    pub fn set_size(&mut self, id: ViewId, size: Size) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.cached_size = size;
        }
    }

    /// Remove a view from the registry
    pub fn remove(&mut self, id: ViewId) -> Option<ViewEntry> {
        self.entries.remove(&id)
    }

    /// Check if a view ID exists in the registry
    pub fn contains(&self, id: ViewId) -> bool {
        self.entries.contains_key(&id)
    }

    /// Get the number of views in the registry
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get all view IDs
    pub fn ids(&self) -> Vec<ViewId> {
        self.entries.keys().copied().collect()
    }

    /// Clear all views from the registry
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Check if a view is a Window using type erasure
    fn check_is_window(view: &Box<dyn View>) -> bool {
        // Use Any to check the concrete type
        view.as_any().is::<crate::view::Window>()
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
    use crate::view::controls::{Text, Button};
    use crate::view::Spacer;

    #[test]
    fn test_registry_register() {
        let mut registry = ViewRegistry::new();
        let text = Text::new("Hello");
        let id = registry.register(Box::new(text));

        assert!(registry.contains(id));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_registry_parent_child() {
        let mut registry = ViewRegistry::new();
        let parent = Text::new("Parent");
        let child = Text::new("Child");

        let parent_id = registry.register(Box::new(parent));
        let child_id = registry.register(Box::new(child));

        registry.add_child(parent_id, child_id);

        assert_eq!(registry.get_parent(child_id), Some(parent_id));
        assert_eq!(registry.get_children(parent_id), &[child_id]);
    }

    #[test]
    fn test_registry_generate_unique_ids() {
        let registry = ViewRegistry::new();
        let id1 = registry.generate_id();
        let id2 = registry.generate_id();
        assert_ne!(id1, id2);
    }
}
