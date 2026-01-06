//! Task namespace module.
//!
//! This module provides task ID namespace functionality, allowing different
//! ABI modules to maintain separate task ID spaces while preserving parent-child
//! relationships and global task management.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::Mutex;

/// Task namespace for managing task IDs within a specific context.
///
/// Each namespace maintains its own task ID counter and can be used by
/// ABI modules to implement their own task ID systems (e.g., Linux PID
/// namespace, xv6 process IDs, etc.).
///
/// Namespaces form a hierarchy where child namespaces inherit from parent
/// namespaces, allowing tasks in child namespaces to be visible from parent
/// namespaces with potentially different IDs.
#[derive(Debug)]
pub struct TaskNamespace {
    /// Unique identifier for this namespace
    id: usize,
    /// Next task ID to allocate in this namespace
    next_task_id: Mutex<usize>,
    /// Mapping from namespace-local task IDs to global task IDs.
    ///
    /// This enables syscall boundary translation (PID namespace semantics)
    /// while keeping kernel internals globally-addressed.
    local_to_global: Mutex<BTreeMap<usize, usize>>,
    /// Reverse mapping from global task IDs to namespace-local task IDs.
    global_to_local: Mutex<BTreeMap<usize, usize>>,
    /// Parent namespace (None for root namespace)
    parent: Option<Arc<TaskNamespace>>,
    /// Name/description of this namespace (for debugging)
    name: alloc::string::String,
}

impl TaskNamespace {
    /// Create a new root namespace.
    ///
    /// # Arguments
    /// * `name` - Name/description of this namespace
    ///
    /// # Returns
    /// A new root namespace with ID 0
    pub fn new_root(name: alloc::string::String) -> Arc<Self> {
        Arc::new(TaskNamespace {
            id: 0,
            next_task_id: Mutex::new(1), // Start from 1 (0 is often reserved)
            local_to_global: Mutex::new(BTreeMap::new()),
            global_to_local: Mutex::new(BTreeMap::new()),
            parent: None,
            name,
        })
    }

    /// Create a new child namespace.
    ///
    /// # Arguments
    /// * `parent` - Parent namespace
    /// * `name` - Name/description of this namespace
    ///
    /// # Returns
    /// A new child namespace
    pub fn new_child(parent: Arc<TaskNamespace>, name: alloc::string::String) -> Arc<Self> {
        static NEXT_NS_ID: Mutex<usize> = Mutex::new(1);
        let ns_id = {
            let mut id = NEXT_NS_ID.lock();
            let current = *id;
            *id += 1;
            current
        };

        Arc::new(TaskNamespace {
            id: ns_id,
            next_task_id: Mutex::new(1),
            local_to_global: Mutex::new(BTreeMap::new()),
            global_to_local: Mutex::new(BTreeMap::new()),
            parent: Some(parent),
            name,
        })
    }

    /// Allocate a new task ID in this namespace.
    ///
    /// # Returns
    /// The next available task ID in this namespace
    pub fn allocate_task_id(&self) -> usize {
        let mut next_id = self.next_task_id.lock();
        let id = *next_id;
        *next_id += 1;
        id
    }

    /// Allocate a new namespace-local task ID and register it for a global task.
    ///
    /// This is the preferred allocator when a task is created or enters this namespace,
    /// because syscalls need a stable local↔global mapping.
    pub fn allocate_task_id_for(&self, global_task_id: usize) -> usize {
        let local_id = self.allocate_task_id();
        self.register_mapping(local_id, global_task_id);
        local_id
    }

    /// Register an existing namespace-local ID mapping for a global task ID.
    pub fn register_mapping(&self, local_id: usize, global_task_id: usize) {
        self.local_to_global.lock().insert(local_id, global_task_id);
        self.global_to_local.lock().insert(global_task_id, local_id);
    }

    /// Resolve a namespace-local task ID to a global task ID.
    pub fn resolve_global_id(&self, local_id: usize) -> Option<usize> {
        self.local_to_global.lock().get(&local_id).copied()
    }

    /// Resolve a global task ID to a namespace-local task ID.
    pub fn resolve_local_id(&self, global_task_id: usize) -> Option<usize> {
        self.global_to_local.lock().get(&global_task_id).copied()
    }

    /// Get the namespace ID.
    pub fn get_id(&self) -> usize {
        self.id
    }

    /// Get the parent namespace, if any.
    pub fn get_parent(&self) -> Option<&Arc<TaskNamespace>> {
        self.parent.as_ref()
    }

    /// Get the namespace name.
    pub fn get_name(&self) -> &str {
        &self.name
    }

    /// Check if this namespace is the root namespace.
    pub fn is_root(&self) -> bool {
        self.parent.is_none()
    }
}

/// Global root task namespace.
///
/// This is the default namespace used by all tasks unless explicitly
/// assigned to a different namespace.
static ROOT_NAMESPACE: spin::Once<Arc<TaskNamespace>> = spin::Once::new();

/// Get the global root task namespace.
pub fn get_root_namespace() -> &'static Arc<TaskNamespace> {
    ROOT_NAMESPACE.call_once(|| TaskNamespace::new_root("root".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test_case]
    fn test_root_namespace_creation() {
        let root = TaskNamespace::new_root("test_root".to_string());
        assert_eq!(root.get_id(), 0);
        assert_eq!(root.get_name(), "test_root");
        assert!(root.is_root());
        assert!(root.get_parent().is_none());
    }

    #[test_case]
    fn test_child_namespace_creation() {
        let root = TaskNamespace::new_root("root".to_string());
        let child = TaskNamespace::new_child(root.clone(), "child".to_string());

        assert_ne!(child.get_id(), 0);
        assert_eq!(child.get_name(), "child");
        assert!(!child.is_root());
        assert!(child.get_parent().is_some());
    }

    #[test_case]
    fn test_task_id_allocation() {
        let ns = TaskNamespace::new_root("test".to_string());
        let id1 = ns.allocate_task_id();
        let id2 = ns.allocate_task_id();
        let id3 = ns.allocate_task_id();

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    #[test_case]
    fn test_separate_namespace_ids() {
        let ns1 = TaskNamespace::new_root("ns1".to_string());
        let ns2 = TaskNamespace::new_root("ns2".to_string());

        let id1_1 = ns1.allocate_task_id();
        let id2_1 = ns2.allocate_task_id();
        let id1_2 = ns1.allocate_task_id();
        let id2_2 = ns2.allocate_task_id();

        // Both namespaces should allocate IDs independently
        assert_eq!(id1_1, 1);
        assert_eq!(id2_1, 1);
        assert_eq!(id1_2, 2);
        assert_eq!(id2_2, 2);
    }

    #[test_case]
    fn test_root_namespace_singleton() {
        let root1 = get_root_namespace();
        let root2 = get_root_namespace();

        // Should return the same instance
        assert_eq!(Arc::as_ptr(root1), Arc::as_ptr(root2));
    }
}
