//! Mount Tree Implementation
//!
//! This module provides a Trie-based mount point management system
//! for efficient path resolution and hierarchical mount point organization.

use alloc::{collections::BTreeMap, string::String, sync::Arc, vec::Vec};
use super::*;

/// Mount point management using Trie structure
/// 
/// MountTree provides efficient hierarchical mount point management using a Trie
/// data structure. This enables O(log k) path resolution where k is the path depth,
/// making filesystem operations scale well with complex mount hierarchies.
/// 
/// # Features
/// 
/// - **Efficient Path Resolution**: Trie-based lookup for fast mount point discovery
/// - **Bind Mount Support**: Advanced bind mounting with cross-VFS capability
/// - **Security**: Enhanced path normalization preventing directory traversal attacks
/// - **Caching**: Optional path caching for frequently accessed mount points
/// - **Thread Safety**: All operations are thread-safe using RwLock protection
/// 
/// # Architecture
/// 
/// The mount tree consists of:
/// - Root node representing the filesystem root "/"
/// - Internal nodes for directory components
/// - Leaf nodes containing actual mount points
/// - Path cache for performance optimization
/// 
/// # Usage
/// 
/// ```rust
/// let mut mount_tree = MountTree::new();
/// 
/// // Mount a filesystem
/// mount_tree.mount("/mnt/data", mount_point)?;
/// 
/// // Resolve a path to its mount point
/// let (mount_node, relative_path) = mount_tree.resolve("/mnt/data/file.txt")?;
/// ```
#[derive(Clone)]
pub struct MountTree {
    root: Arc<MountNode>,
    /// Cache for fast lookup
    path_cache: BTreeMap<String, Arc<MountPoint>>,
}

/// Mount tree node representing a single component in the filesystem hierarchy
/// 
/// Each MountNode represents a directory component in the filesystem path.
/// Nodes can optionally contain mount points and have child nodes for
/// subdirectories. The tree structure enables efficient path traversal
/// and mount point resolution.
/// 
/// # Thread Safety
/// 
/// All fields are protected by RwLock to ensure thread-safe access in
/// a multi-threaded kernel environment.
pub struct MountNode {
    /// Path component
    #[allow(dead_code)]
    pub component: RwLock<String>,
    /// Mount information if this node is a mount point
    mount_point: RwLock<Option<Arc<MountPoint>>>,
    /// Child nodes
    children: RwLock<BTreeMap<String, Arc<MountNode>>>,
}

impl MountNode {
    /// Create a new mount node with the specified path component
    /// 
    /// # Arguments
    /// 
    /// * `component` - The path component name for this node
    /// 
    /// # Returns
    /// 
    /// A new MountNode instance with the given component name
    fn new(component: String) -> Self {
        Self {
            component: RwLock::new(component),
            mount_point: RwLock::new(None),
            children: RwLock::new(BTreeMap::new()),
        }
    }

    /// Get the mount point associated with this node
    /// 
    /// # Returns
    /// * `Ok(Arc<MountPoint>)` - The mount point if it exists.
    /// * `Err(FileSystemError)` - If no mount point is found.
    /// 
    pub fn get_mount_point(&self) -> Result<Arc<MountPoint>> {
        let mount_guard = self.mount_point.read();
        if let Some(mount_point) = &*mount_guard {
            Ok(mount_point.clone())
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "No mount point found".to_string(),
            })
        }
    }

    /// Resolve path within this mount node and its children (transparent resolution)
    /// 
    /// This method resolves the given path components starting from this node.
    /// For bind mounts, it recursively resolves through the source mount tree
    /// to find the deepest actual mount that handles the requested path.
    /// 
    /// This is an internal method that operates on Arc<MountNode>
    /// 
    /// # Arguments
    /// * `self_arc` - Arc reference to this node
    /// * `components` - The path components to resolve.
    /// * `depth` - Current recursion depth to prevent infinite loops.
    /// 
    /// # Returns
    /// * `Ok(Some((Arc<MountNode>, String)))` - A tuple containing the resolved mount node and the relative path.
    /// * `Ok(None)` - If this node is the final target (self-resolution).
    /// * `Err(FileSystemError)` - If the path is invalid or no mount point is found.
    /// 
    fn resolve_internal(self_arc: Arc<MountNode>, components: &[String], depth: usize) -> Result<Option<(Arc<MountNode>, String)>> {
        // Prevent infinite recursion in bind mount chains
        if depth > 32 {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "Too many bind mount redirections".to_string(),
            });
        }

        // crate::println!("Resolving internal mount point for path: {}, depth: {}", components.join("/"), depth);

        // If no components left, check if this node is a mount point
        if components.is_empty() {
            // crate::println!("No components left, checking if this node is a mount point");
            let mount_guard = self_arc.mount_point.read();
            if mount_guard.is_some() {
                // This node is a mount point and is the target
                return Ok(None);
            } else {
                // No mount point at this location
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::NotFound,
                    message: "No mount point found".to_string(),
                });
            }
        }

        // Start with self
        let mut current_node = self_arc.clone();
        let mut best_match_node: Option<Arc<MountNode>> = None;
        let mut match_depth = 0;

        // Check if current node is a mount point
        {
            let mount_guard = current_node.mount_point.read();
            if mount_guard.is_some() {
                best_match_node = Some(current_node.clone());
                match_depth = 0;

                // crate::println!("Current node is a mount point: {}", current_node.component.read().as_str());
            }
        }
        
        // Traverse components to find the deepest mount point
        for (depth_idx, component) in components.iter().enumerate() {
            // Move to next node
            let next_node = {
                let children_guard = current_node.children.read();
                if let Some(child) = children_guard.get(component) {
                    child.clone()
                } else {
                    break;
                }
            };
            current_node = next_node;
            
            // Check if the moved-to node is a mount point
            {
                let mount_guard = current_node.mount_point.read();
                if mount_guard.is_some() {
                    best_match_node = Some(current_node.clone());
                    match_depth = depth_idx + 1; // +1 for depth after movement
                }
            }
        }
        
        let resolved_node = best_match_node.ok_or(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "No mount point found in this subtree".to_string(),
        })?;
        
        // Build relative path
        let relative_components = &components[match_depth..];
        let relative_path = if relative_components.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", relative_components.join("/"))
        };

        // Check if this is a bind mount and needs further resolution
        let mount_point = resolved_node.get_mount_point()?;
        match &mount_point.mount_type {
            MountType::Bind { source_mount_node, source_relative_path, .. } => {
                // Construct the full source path
                let full_source_path = match (relative_path.as_str(), source_relative_path.as_str()) {
                    ("/", _) => source_relative_path.clone(),
                    (_, "/") => relative_path.clone(),
                    (rel, src) => {
                        let src_trimmed = src.trim_end_matches('/');
                        let rel_trimmed = rel.trim_start_matches('/');
                        format!("{}/{}", src_trimmed, rel_trimmed)
                    }
                };

                // Split the full source path into components
                let source_components: Vec<String> = if full_source_path == "/" {
                    Vec::new()
                } else {
                    full_source_path.trim_start_matches('/')
                        .split('/')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect()
                };
                
                // Recursively resolve through the source mount node
                let result = MountNode::resolve_internal(source_mount_node.clone(), &source_components, depth + 1)?;
                let (final_node, final_relative_path) = match result {
                    Some((node, path)) => (node, path),
                    None => {
                        // If None, use the source mount node itself
                        (source_mount_node.clone(), "/".to_string())
                    }
                };
                
                // Verify that the final node is not another bind mount to the same location
                // to prevent infinite loops
                let final_mount_point = final_node.get_mount_point()?;
                match &final_mount_point.mount_type {
                    MountType::Bind { .. } => {
                        // If we still have a bind mount, check if it's different from where we started
                        if Arc::ptr_eq(&final_node, &resolved_node) {
                            return Err(FileSystemError {
                                kind: FileSystemErrorKind::NotSupported,
                                message: "Circular bind mount detected".to_string(),
                            });
                        }
                    }
                    _ => {}
                }
                
                Ok(Some((final_node, final_relative_path)))
            }
            _ => {
                // Regular mount or overlay - return as-is
                Ok(Some((resolved_node, relative_path)))
            }
        }
    }

    /// Resolve path within this mount node and its children (non-transparent resolution)
    /// 
    /// This method resolves the given path components starting from this node without
    /// resolving through bind mounts. This is useful for mount management operations
    /// where you need to work with the bind mount node itself.
    /// 
    /// This is an internal method that operates on Arc<MountNode>
    /// 
    /// # Arguments
    /// * `self_arc` - Arc reference to this node
    /// * `components` - The path components to resolve.
    /// 
    /// # Returns
    /// * `Ok(Some((Arc<MountNode>, String)))` - A tuple containing the mount node and the relative path.
    /// * `Ok(None)` - If this node is the final target (self-resolution).
    /// * `Err(FileSystemError)` - If the path is invalid or no mount point is found.
    /// 
    fn resolve_non_transparent_internal(self_arc: Arc<MountNode>, components: &[String]) -> Result<Option<(Arc<MountNode>, String)>> {
        // If no components left, check if this node is a mount point
        if components.is_empty() {
            let mount_guard = self_arc.mount_point.read();
            if mount_guard.is_some() {
                // This node is a mount point and is the target
                return Ok(None);
            } else {
                // No mount point at this location
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::NotFound,
                    message: "No mount point found".to_string(),
                });
            }
        }

        // Start with self
        let mut current_node = self_arc.clone();
        let mut best_match_node: Option<Arc<MountNode>> = None;
        let mut match_depth = 0;
        
        // Check if current node is a mount point
        {
            let mount_guard = current_node.mount_point.read();
            if mount_guard.is_some() {
                best_match_node = Some(current_node.clone());
                match_depth = 0;
            }
        }
        
        // Traverse components to find the deepest mount point
        for (depth, component) in components.iter().enumerate() {
            // Move to next node
            let next_node = {
                let children_guard = current_node.children.read();
                if let Some(child) = children_guard.get(component) {
                    child.clone()
                } else {
                    break;
                }
            };
            current_node = next_node;
            
            // Check if the moved-to node is a mount point
            {
                let mount_guard = current_node.mount_point.read();
                if mount_guard.is_some() {
                    best_match_node = Some(current_node.clone());
                    match_depth = depth + 1; // +1 for depth after movement
                }
            }
        }
        
        let resolved_node = best_match_node.ok_or(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: "No mount point found in this subtree".to_string(),
        })?;
        
        // Build relative path
        let relative_components = &components[match_depth..];
        let relative_path = if relative_components.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", relative_components.join("/"))
        };
        
        // Return the mount node without resolving bind mounts
        Ok(Some((resolved_node, relative_path)))
    }
}

/// Extended mount point information
/// 
/// MountPoint contains comprehensive information about a mounted filesystem,
/// including metadata, mount options, and relationship information for
/// hierarchical mount management.
/// 
/// # Features
/// 
/// - **Filesystem Integration**: Direct reference to the mounted filesystem
/// - **Mount Hierarchy**: Parent/child relationships for mount tree management
/// - **Security Options**: Configurable mount options for access control
/// - **Bind Mount Support**: Resolves bind mount chains to actual filesystems
/// - **Metadata Tracking**: Mount time and filesystem identification
/// 
/// # Thread Safety
/// 
/// MountPoint is designed to be shared safely between threads using Arc
/// wrapper when stored in the mount tree.
#[derive(Clone)]
pub struct MountPoint {
    /// Absolute mount path in the filesystem hierarchy
    pub path: String,
    /// Reference to the actual filesystem implementation
    pub fs: super::FileSystemRef,
    /// VfsManager-assigned filesystem identifier
    pub fs_id: usize,
    /// Type of mount (regular, bind, overlay)
    pub mount_type: MountType,
    /// Security and behavior options for this mount
    pub mount_options: MountOptions,
    /// Parent mount path (None for root mount)
    pub parent: Option<String>,
    /// List of child mount paths
    pub children: Vec<String>,
    /// Timestamp when this mount was created
    pub mount_time: u64,
}

impl MountPoint {
    /// Resolve filesystem and internal path from MountPoint
    /// 
    /// This method resolves bind mount chains to find the actual filesystem
    /// that handles the requested path. For regular mounts, it returns the
    /// filesystem directly. For bind mounts, it recursively follows the
    /// bind chain to the source filesystem.
    /// 
    /// # Arguments
    /// 
    /// * `relative_path` - Path relative to this mount point
    /// 
    /// # Returns
    /// 
    /// * `Ok((FileSystemRef, String))` - The actual filesystem and the resolved path within it
    /// * `Err(FileSystemError)` - If bind mount resolution fails or exceeds recursion limit
    /// 
    /// # Note
    /// 
    /// This method only supports Regular, Tmpfs, and Overlay mounts.
    /// Bind mounts are resolved transparently to their source filesystems.
    pub fn resolve_fs(&self, relative_path: &str) -> Result<(super::FileSystemRef, String)> {
        self.resolve_fs_with_depth(relative_path, 0)
    }
    
    /// Internal method for bind mount resolution with recursion depth tracking
    /// 
    /// This method prevents infinite recursion in circular bind mount chains
    /// by limiting the maximum recursion depth to 32 levels.
    /// 
    /// # Arguments
    /// 
    /// * `relative_path` - Path relative to this mount point
    /// * `depth` - Current recursion depth for loop detection
    /// 
    /// # Returns
    /// 
    /// * `Ok((FileSystemRef, String))` - The actual filesystem and resolved path
    /// * `Err(FileSystemError)` - If recursion limit exceeded or resolution fails
    fn resolve_fs_with_depth(&self, relative_path: &str, depth: usize) -> Result<(super::FileSystemRef, String)> {
        // Prevent circular references
        if depth > 32 {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "Too many bind mount redirections".to_string(),
            });
        }
        
        match &self.mount_type {
            MountType::Regular | MountType::Overlay { .. } => {
                // Regular mount: return filesystem as-is
                Ok((self.fs.clone(), relative_path.to_string()))
            }
            
            MountType::Bind { source_mount_node, source_relative_path, .. } => {
                // Combine paths
                let full_source_path = match (relative_path, source_relative_path.as_str()) {
                    ("/", _) => source_relative_path.clone(),
                    (_, "/") => relative_path.to_string().clone(),
                    (rel, src) => {
                        let src_trimmed = src.trim_end_matches('/');
                        let rel_trimmed = rel.trim_start_matches('/');
                        format!("{}/{}", src_trimmed, rel_trimmed)
                    }
                };
                
                // Recursively resolve with source MountNode
                let source_mount_point = source_mount_node.get_mount_point()?;
                source_mount_point.resolve_fs_with_depth(&full_source_path, depth + 1)
            }
        }
    }
}

/// Mount type classification for different mount strategies
/// 
/// This enum defines the various mount types supported by the VFS system,
/// each with different behaviors and resource handling approaches.
#[derive(Clone)]
pub enum MountType {
    /// Regular filesystem mount
    /// 
    /// Standard mount where the filesystem directly handles all operations
    /// at the mount point. This is the most common mount type.
    Regular,
    
    /// Bind mount - maps one directory tree to another location
    /// 
    /// Bind mounts allow the same filesystem content to be accessible
    /// from multiple mount points. They support:
    /// - Cross-VFS sharing for container resource sharing
    /// - Read-only restrictions for security
    /// - Shared propagation for namespace management
    Bind {
        /// Source mount node that provides the actual filesystem
        source_mount_node: Arc<MountNode>,
        /// Relative path within the source filesystem
        source_relative_path: String,
        /// Type of bind mount (read-only, read-write, shared)
        bind_type: BindType,
    },
    
    /// Overlay filesystem mount
    /// 
    /// Combines multiple filesystem layers into a unified view,
    /// typically used for container images and copy-on-write scenarios.
    Overlay {
        /// Lower filesystem layers (read-only)
        lower_layers: Vec<String>,
        /// Upper layer for writes
        upper_layer: String,
        /// Working directory for overlay operations
        work_dir: String,
    },
}

/// Bind mount type specifying access and propagation behavior
/// 
/// Different bind types provide various levels of access control
/// and mount propagation for container and namespace isolation.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BindType {
    /// Read-only bind mount - prevents write operations
    ReadOnly,
    /// Read-write bind mount - allows full access
    ReadWrite,
    /// Shared bind mount - propagates mount events to other namespaces
    Shared,
}

/// Mount options controlling filesystem behavior and security
/// 
/// These options provide fine-grained control over filesystem access
/// and can be used to enhance security in containerized environments.
#[derive(Clone)]
pub struct MountOptions {
    /// Prevent write operations on this mount
    pub read_only: bool,
    /// Disable execution of binaries on this mount
    pub no_exec: bool,
    /// Disable set-uid/set-gid bits on this mount
    pub no_suid: bool,
    /// Disable device file access on this mount
    pub no_dev: bool,
    /// Force synchronous I/O operations
    pub sync: bool,
}

impl Default for MountOptions {
    fn default() -> Self {
        Self {
            read_only: false,
            no_exec: false,
            no_suid: false,
            no_dev: false,
            sync: false,
        }
    }
}

impl MountTree {
    /// Create a new empty mount tree
    /// 
    /// Initializes a new mount tree with an empty root node and no cached paths.
    /// This creates the foundation for a new filesystem namespace.
    /// 
    /// # Returns
    /// 
    /// A new MountTree instance ready for mount operations
    pub fn new() -> Self {
        Self {
            root: Arc::new(MountNode::new("".to_string())),
            path_cache: BTreeMap::new(),
        }
    }
    
    /// Add mount point (VfsManager interface)
    /// 
    /// This method provides the primary interface for VfsManager to add new
    /// mount points to the tree. It normalizes the path and delegates to
    /// the internal insert method.
    /// 
    /// # Arguments
    /// 
    /// * `path` - Absolute path where the filesystem should be mounted
    /// * `mount_point` - MountPoint structure containing mount information
    /// 
    /// # Returns
    /// 
    /// * `Ok(())` - Mount operation succeeded
    /// * `Err(FileSystemError)` - If path is invalid or mount point already exists
    pub fn mount(&mut self, path: &str, mount_point: MountPoint) -> Result<()> {
        self.insert(path, mount_point)
    }
    
    /// Internal method to add mount point to the tree
    /// 
    /// This method handles the actual insertion logic, creating intermediate
    /// nodes as needed and validating that mount points don't conflict.
    /// 
    /// # Arguments
    /// 
    /// * `path` - Normalized absolute path for the mount
    /// * `mount_point` - MountPoint structure to insert
    /// 
    /// # Returns
    /// 
    /// * `Ok(())` - Mount point successfully added
    /// * `Err(FileSystemError)` - If mount point already exists at the path
    pub fn insert(&mut self, path: &str, mount_point: MountPoint) -> Result<()> {
        let normalized = Self::normalize_path(path)?;
        let components = self.split_path(&normalized);
        
        // Traverse path using Trie structure
        let mut current_arc = self.root.clone();
        for component in &components {
            let next_arc = {
                let mut children = current_arc.children.write();
                children.entry(component.clone())
                    .or_insert_with(|| Arc::new(MountNode::new(component.clone())))
                    .clone()
            };
            current_arc = next_arc;
        }
        
        // Return error if mount point already exists
        if current_arc.mount_point.read().is_some() {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::AlreadyExists,
                message: format!("Mount point {} already exists", path),
            });
        }
        
        // Set mount point
        let mount_point_arc = Arc::new(mount_point);
        *current_arc.mount_point.write() = Some(mount_point_arc.clone());
        self.path_cache.insert(normalized, mount_point_arc);
        
        Ok(())
    }
    
    /// Find mount point by path (transparent resolution - resolves through bind mounts)
    /// 
    /// This method resolves the given path to its corresponding mount point.
    /// For bind mounts, it recursively resolves through the source mount tree
    /// to find the deepest actual mount that handles the requested path.
    /// 
    /// # Arguments
    /// * `path` - The absolute path to resolve.
    /// 
    /// # Returns
    /// * `Ok((Arc<MountNode>, String))` - A tuple containing the resolved mount node and the relative path.
    /// * `Err(FileSystemError)` - If the path is invalid or no mount point is found.
    /// 
    pub fn resolve(&self, path: &str) -> Result<(Arc<MountNode>, String)> {
        let normalized = Self::normalize_path(path)?;
        let components = self.split_path(&normalized);
        
        let result = MountNode::resolve_internal(self.root.clone(), &components, 0)?;
        match result {
            Some((node, path)) => Ok((node, path)),
            None => {
                // If None, use the root node itself with empty path
                Ok((self.root.clone(), "/".to_string()))
            }
        }
    }

    /// Find mount point by path (non-transparent resolution - stops at bind mount nodes)
    /// 
    /// This method resolves the given path to its corresponding mount point without
    /// resolving through bind mounts. This is useful for mount management operations
    /// where you need to work with the bind mount node itself.
    /// 
    /// # Arguments
    /// * `path` - The absolute path to resolve.
    /// 
    /// # Returns
    /// * `Ok((Arc<MountNode>, String))` - A tuple containing the mount node and the relative path.
    /// * `Err(FileSystemError)` - If the path is invalid or no mount point is found.
    /// 
    pub fn resolve_non_transparent(&self, path: &str) -> Result<(Arc<MountNode>, String)> {
        let normalized = Self::normalize_path(path)?;
        let components = self.split_path(&normalized);
        
        let result = MountNode::resolve_non_transparent_internal(self.root.clone(), &components)?;
        match result {
            Some((node, path)) => Ok((node, path)),
            None => {
                // If None, use the root node itself with empty path
                Ok((self.root.clone(), String::new()))
            }
        }
    }
    
    /// Remove mount point from the tree
    /// 
    /// Removes a mount point at the specified path and returns the removed
    /// MountPoint for cleanup. This operation also removes the path from
    /// the internal cache.
    /// 
    /// # Arguments
    /// 
    /// * `path` - Absolute path of the mount point to remove
    /// 
    /// # Returns
    /// 
    /// * `Ok(Arc<MountPoint>)` - The removed mount point
    /// * `Err(FileSystemError)` - If no mount point exists at the path
    pub fn remove(&mut self, path: &str) -> Result<Arc<MountPoint>> {
        let normalized = Self::normalize_path(path)?;
        let components = self.split_path(&normalized);
        
        // Traverse path to find node
        let mut current_arc = self.root.clone();
        for component in &components {
            let next_arc = {
                let children = current_arc.children.read();
                children.get(component)
                    .ok_or(FileSystemError {
                        kind: FileSystemErrorKind::NotFound,
                        message: format!("Mount point {} not found", path),
                    })?
                    .clone()
            };
            current_arc = next_arc;
        }
        
        // Remove mount point
        let mount_point = current_arc.mount_point.write()
            .take()
            .ok_or(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("No mount point at {}", path),
            })?;
        
        self.path_cache.remove(&normalized);
        
        Ok(mount_point)
    }
    
    /// List all mount points in the tree
    /// 
    /// Returns a vector of all mount point paths currently registered
    /// in the mount tree. This is useful for debugging and system
    /// introspection.
    /// 
    /// # Returns
    /// 
    /// Vector of mount point paths in no particular order
    pub fn list_all(&self) -> Vec<String> {
        let mut paths = Vec::new();
        self.collect_mount_paths(&self.root, String::new(), &mut paths);
        paths
    }
    
    /// Get number of mount points
    /// 
    /// Returns the total number of mount points currently registered
    /// in this mount tree.
    /// 
    /// # Returns
    /// 
    /// Number of active mount points
    pub fn len(&self) -> usize {
        self.path_cache.len()
    }
    
    /// Secure path normalization with directory traversal protection
    /// 
    /// This method normalizes filesystem paths by resolving "." and ".."
    /// components while preventing directory traversal attacks that could
    /// escape the filesystem root. It ensures all paths are absolute
    /// and properly formatted.
    /// 
    /// # Arguments
    /// 
    /// * `path` - Input path to normalize
    /// 
    /// # Returns
    /// 
    /// * `Ok(String)` - Normalized absolute path
    /// * `Err(FileSystemError)` - If path is not absolute or invalid
    /// 
    /// # Security
    /// 
    /// This method prevents:
    /// - Relative path traversal (../../../etc/passwd)
    /// - Root directory escape attempts
    /// - Malformed path components
    /// 
    /// # Examples
    /// 
    /// ```rust
    /// assert_eq!(MountTree::normalize_path("/a/b/../c")?, "/a/c");
    /// assert_eq!(MountTree::normalize_path("/a/./b")?, "/a/b");
    /// assert_eq!(MountTree::normalize_path("/../..")?, "/");
    /// ```
    pub fn normalize_path(path: &str) -> Result<String> {
        if !path.starts_with('/') {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::InvalidPath,
                message: "Path must be absolute".to_string(),
            });
        }
        
        let mut normalized_components = Vec::new();
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        
        for component in components {
            match component {
                "." => continue,
                ".." => {
                    // Move to parent directory (cannot go above root)
                    if !normalized_components.is_empty() {
                        normalized_components.pop();
                    }
                }
                comp => normalized_components.push(comp),
            }
        }
        
        if normalized_components.is_empty() {
            Ok("/".to_string())
        } else {
            Ok(format!("/{}", normalized_components.join("/")))
        }
    }
    
    /// Split normalized path into components
    /// 
    /// Converts a normalized path string into a vector of path components
    /// for tree traversal. Root path "/" becomes an empty vector.
    /// 
    /// # Arguments
    /// 
    /// * `path` - Normalized absolute path
    /// 
    /// # Returns
    /// 
    /// Vector of path components, empty for root path
    fn split_path(&self, path: &str) -> Vec<String> {
        if path == "/" {
            return Vec::new();
        }
        path.trim_start_matches('/')
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }
    
    /// Recursively collect mount paths from tree nodes
    /// 
    /// Internal method for traversing the mount tree and collecting
    /// all mount point paths for the list_all() operation.
    /// 
    /// # Arguments
    /// 
    /// * `node` - Current node being examined
    /// * `current_path` - Path accumulated up to this node
    /// * `paths` - Vector to collect found mount paths
    fn collect_mount_paths(&self, node: &MountNode, current_path: String, paths: &mut Vec<String>) {
        // Check if this node has a mount point
        let mount_guard = node.mount_point.read();
        if mount_guard.is_some() {
            let path = if current_path.is_empty() { "/".to_string() } else { current_path.clone() };
            paths.push(path);
        }
        // Guard is dropped here
        
        // Iterate through children
        let children_guard = node.children.read();
        for (component, child) in children_guard.iter() {
            let child_path = if current_path.is_empty() {
                format!("/{}", component)
            } else {
                format!("{}/{}", current_path, component)
            };
            self.collect_mount_paths(child, child_path, paths);
        }
        // Guard is dropped here
    }
}
