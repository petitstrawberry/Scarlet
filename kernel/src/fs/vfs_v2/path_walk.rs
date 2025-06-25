//! Path Walking Algorithm for VFS v2
//!
//! This module implements the core path resolution algorithm for **single filesystem**.
//! 
//! PathWalkContext is responsible for:
//! - Resolving paths within a single filesystem boundary
//! - Managing VfsEntry cache for performance
//! - Handling filesystem lookup operations
//! - Processing symbolic links
//!
//! It does NOT handle:
//! - Mount point traversal (handled by MountTreeV2)
//! - Cross-filesystem path resolution
//! - Mount boundary detection

use alloc::{
    format, string::{String, ToString}, sync::Arc, vec::Vec
};

use crate::fs::{FileSystemError, FileSystemErrorKind, FileType};
use super::core::{VfsEntry, VfsNode, FileSystemOperations};

/// PathWalkContext provides efficient path resolution within a single filesystem
/// 
/// Key responsibilities:
/// 1. Resolve relative paths within a filesystem subtree
/// 2. Cache VfsEntry lookups for performance  
/// 3. Handle filesystem-specific operations (lookup, symbolic links)
/// 4. Manage parent-child relationships in VfsEntry tree
///
/// Usage pattern:
/// ```
/// let walker = PathWalkContext::new(filesystem_root);
/// let resolved = walker.resolve_within_filesystem("path/to/file", current_dir)?;
/// ```
#[derive(Debug)]
pub struct PathWalkContext {
    /// Root VfsEntry for this filesystem context
    filesystem_root: Arc<VfsEntry>,
}

impl PathWalkContext {
    /// Create a new path walking context with the given filesystem root
    pub fn new(filesystem_root: Arc<VfsEntry>) -> Self {
        Self { filesystem_root }
    }

    /// Resolve a path within the current filesystem context
    /// 
    /// This method resolves paths that are guaranteed to be within a single filesystem.
    /// It should be called from MountTreeV2 after mount boundary checks are done.
    /// 
    /// # Arguments
    /// * `path` - Path to resolve (can be relative or absolute within this filesystem)
    /// * `current_working_dir` - Optional starting directory for relative paths
    /// 
    /// # Returns
    /// * `Result<Arc<VfsEntry>, FileSystemError>` - The resolved VfsEntry
    pub fn resolve_within_filesystem(
        &self,
        path: &str,
        current_working_dir: Option<Arc<VfsEntry>>,
    ) -> Result<Arc<VfsEntry>, FileSystemError> {
        // Phase 1: Determine starting point
        let starting_entry = if path.starts_with('/') {
            // Absolute path within this filesystem
            Arc::clone(&self.filesystem_root)
        } else {
            // Relative path - start from provided CWD
            current_working_dir.ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::InvalidPath,
                "Relative path requires current working directory"
            ))?
        };

        // Phase 2: Split path into components
        let components = self.split_path(path)?;
        
        // Phase 3: Walk through each component within this filesystem
        let mut current_entry = starting_entry;
        
        for component in components {
            current_entry = self.resolve_component(current_entry, &component)?;
        }

        Ok(current_entry)
    }

    /// Split a path into individual components
    /// Note: . and .. are NOT processed here - they are handled by filesystem implementations
    fn split_path(&self, path: &str) -> Result<Vec<String>, FileSystemError> {
        let mut components = Vec::new();
        
        // Remove leading and trailing slashes, split by '/'
        let path_trimmed = path.trim_matches('/');
        if path_trimmed.is_empty() {
            return Ok(components); // Root path
        }

        for component in path_trimmed.split('/') {
            match component {
                "" => {
                    // Empty component from double slash - skip
                    continue;
                }
                name => {
                    // All components including . and .. are passed to filesystem
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
        // Special handling for ".." - check if we're at filesystem root
        if component == ".." {
            return self.resolve_parent_directory(parent_entry);
        }

        // Phase 1: Cache lookup (fast path)
        if let Some(cached_child) = parent_entry.get_child(component) {
            return Ok(cached_child);
        }

        // Phase 2: Cache miss - perform lookup
        self.perform_lookup(parent_entry, component)
    }

    /// Handle ".." resolution with special logic for filesystem boundaries
    fn resolve_parent_directory(
        &self,
        current_entry: Arc<VfsEntry>,
    ) -> Result<Arc<VfsEntry>, FileSystemError> {
        // Check if we're at the root of this filesystem
        if Arc::ptr_eq(&current_entry, &self.filesystem_root) {
            // At filesystem root - ".." stays at root
            return Ok(Arc::clone(&self.filesystem_root));
        }

        // First, try to get parent from VfsEntry hierarchy
        if let Some(parent) = current_entry.parent() {
            return Ok(parent);
        }

        // If no parent in VfsEntry hierarchy, we might be at a mount point root
        // This case should be handled by VfsManager/MountTreeV2 to traverse mount boundaries
        // For now, return the current entry (filesystem root behavior)
        Ok(current_entry)
    }

    /// Perform filesystem lookup when cache misses
    fn perform_lookup(
        &self,
        parent_entry: Arc<VfsEntry>,
        component: &String,
    ) -> Result<Arc<VfsEntry>, FileSystemError> {
        // Special handling for ".." - filesystem may not support it
        if component == ".." {
            return self.resolve_parent_directory(parent_entry);
        }

        // Get parent VfsNode and filesystem
        let parent_node = parent_entry.node();
        let filesystem = parent_node.filesystem();

        // Call filesystem's lookup method
        let child_node = match filesystem.lookup(parent_node, component) {
            Ok(node) => node,
            Err(e) if e.kind == FileSystemErrorKind::NotSupported && component == ".." => {
                // Filesystem doesn't support ".." - handle at VFS level
                return self.resolve_parent_directory(parent_entry);
            }
            Err(e) => return Err(e),
        };

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

    /// Legacy method for backward compatibility
    /// 
    /// This method maintains compatibility with existing code that expects
    /// the old `path_walk` interface. New code should use `resolve_within_filesystem`.
    #[deprecated(note = "Use resolve_within_filesystem instead")]
    pub fn path_walk(
        &self,
        path: &str,
        current_working_dir: Option<Arc<VfsEntry>>,
    ) -> Result<Arc<VfsEntry>, FileSystemError> {
        self.resolve_within_filesystem(path, current_working_dir)
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
