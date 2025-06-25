//! Path Walking Algorithm for VFS v2
//!
//! This module implements the core path resolution algorithm that converts
//! path strings into VfsEntry objects using the path_walk algorithm.

use alloc::{
    format, string::{String, ToString}, sync::Arc, vec::Vec
};

use crate::fs::{FileSystemError, FileSystemErrorKind, FileType};
use super::core::{VfsEntry, VfsNode, FileSystemOperations};

/// Path walking context for resolving paths to VfsEntry objects
pub struct PathWalkContext {
    /// Root VfsEntry for this context
    root: Arc<VfsEntry>,
}

impl PathWalkContext {
    /// Create a new path walking context with the given root
    pub fn new(root: Arc<VfsEntry>) -> Self {
        Self { root }
    }

    /// Walk a path and resolve it to a VfsEntry
    /// 
    /// This is the core path_walk algorithm that:
    /// 1. Splits the path into components
    /// 2. Iteratively resolves each component using cache or lookup
    /// 3. Handles mount points and symbolic links transparently
    /// 
    /// # Arguments
    /// * `path` - Absolute path to resolve (must start with '/')
    /// * `current_working_dir` - Optional current working directory for relative paths
    /// 
    /// # Returns
    /// * `Result<Arc<RwLock<VfsEntry>>, FileSystemError>` - The resolved VfsEntry
    pub fn path_walk(
        &self,
        path: &str,
        current_working_dir: Option<Arc<VfsEntry>>,
    ) -> Result<Arc<VfsEntry>, FileSystemError> {
        // Phase 1: Initialize starting point
        let starting_entry = if path.starts_with('/') {
            // Absolute path - start from root
            Arc::clone(&self.root)
        } else {
            // Relative path - start from CWD
            current_working_dir.ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::InvalidPath,
                "Relative path requires current working directory"
            ))?
        };

        // Phase 2: Split path into components
        let components = self.split_path(path)?;
        
        // Phase 3: Walk through each component
        let mut current_entry = starting_entry;
        
        for component in components {
            current_entry = self.resolve_component(current_entry, &component)?;
        }

        Ok(current_entry)
    }

    /// Split a path into individual components, handling . and .. appropriately
    fn split_path(&self, path: &str) -> Result<Vec<String>, FileSystemError> {
        let mut components = Vec::new();
        
        // Remove leading and trailing slashes, split by '/'
        let path_trimmed = path.trim_matches('/');
        if path_trimmed.is_empty() {
            return Ok(components); // Root path
        }

        for component in path_trimmed.split('/') {
            match component {
                "." => {
                    // Current directory - skip
                    continue;
                }
                ".." => {
                    // Parent directory - pop last component
                    if components.is_empty() {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::InvalidPath,
                            "Path traversal above root directory"
                        ));
                    }
                    components.pop();
                }
                "" => {
                    // Empty component from double slash - skip
                    continue;
                }
                name => {
                    // Regular component
                    components.push(String::from(name));
                }
            }
        }

        Ok(components)
    }

    /// Resolve a single component within a parent VfsEntry
    fn resolve_component(
        &self,
        parent_entry: Arc<VfsEntry>,
        component: &String,
    ) -> Result<Arc<VfsEntry>, FileSystemError> {
        // Phase 1: Cache lookup (fast path)
        if let Some(cached_child) = parent_entry.get_child(component) {
            return Ok(cached_child);
        }

        // Phase 2: Cache miss - perform lookup
        self.perform_lookup(parent_entry, component)
    }

    /// Perform filesystem lookup when cache misses
    fn perform_lookup(
        &self,
        parent_entry: Arc<VfsEntry>,
        component: &String,
    ) -> Result<Arc<VfsEntry>, FileSystemError> {
        // Get parent VfsNode and filesystem
        let parent_node = parent_entry.node();
        let filesystem = parent_node.filesystem();

        // Call filesystem's lookup method
        let child_node = filesystem.lookup(parent_node, component)?;

        // Create new VfsEntry for the child
        let child_entry = VfsEntry::new(
            Some(Arc::downgrade(&parent_entry)),
            component.clone(),
            child_node,
        );

        // Add to parent's cache
        parent_entry.add_child(component.clone(), Arc::clone(&child_entry));

        // Handle special cases
        self.handle_special_nodes(child_entry)
    }

    /// Handle special node types (mount points, symbolic links)
    fn handle_special_nodes(
        &self,
        entry: Arc<VfsEntry>,
    ) -> Result<Arc<VfsEntry>, FileSystemError> {
        // Check if this is a mount point
        if entry.is_mount_point() {
            // TODO: Handle mount point traversal
            // For now, return the entry as-is
            return Ok(entry);
        }

        // Check if this is a symbolic link
        let node = entry.node();

        if node.is_symlink()? {
            // TODO: Handle symbolic link resolution
            // For now, return the entry as-is
            // In full implementation, we would:
            // 1. Read the link target using node.read_link()
            // 2. Recursively call path_walk on the target
            // 3. Return the resolved target
            return Ok(entry);
        }

        Ok(entry)
    }
}

/// Utility functions for path manipulation
impl PathWalkContext {
    /// Normalize a path by resolving . and .. components
    pub fn normalize_path(path: &str) -> Result<String, FileSystemError> {
        let mut normalized_components = Vec::new();

        // Split by '/' and process each component
        for component in path.split('/') {
            match component {
                "." | "" => {
                    // Skip current directory and empty components
                    continue;
                }
                ".." => {
                    // Parent directory - pop if possible
                    if normalized_components.is_empty() {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::InvalidPath,
                            "Path traversal above root"
                        ));
                    }
                    normalized_components.pop();
                }
                name => {
                    normalized_components.push(name);
                }
            }
        }

        // Reconstruct the path
        if normalized_components.is_empty() {
            Ok("/".to_string())
        } else {
            Ok(format!("/{}", normalized_components.join("/")))
        }
    }
}
