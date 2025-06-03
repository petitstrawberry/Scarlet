//! Mount Tree Implementation
//!
//! This module provides a Trie-based mount point management system
//! for efficient path resolution and hierarchical mount point organization.

use alloc::{collections::BTreeMap, string::String, sync::Arc, vec::Vec};
use super::*;

/// Mount point management using Trie structure
#[derive(Clone)]
pub struct MountTree {
    root: Arc<MountNode>,
    /// Cache for fast lookup
    path_cache: BTreeMap<String, Arc<MountPoint>>,
}

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
#[derive(Clone)]
pub struct MountPoint {
    pub path: String,
    pub fs: super::FileSystemRef,
    pub fs_id: usize,  // VfsManager managed filesystem ID
    pub mount_type: MountType,
    pub mount_options: MountOptions,
    pub parent: Option<String>,
    pub children: Vec<String>,
    pub mount_time: u64,
}

impl MountPoint {
    /// Get filesystem and internal path from MountPoint
    /// Supports Regular/Tmpfs/Overlay mounts only
    pub fn resolve_fs(&self, relative_path: &str) -> Result<(super::FileSystemRef, String)> {
        self.resolve_fs_with_depth(relative_path, 0)
    }
    
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

#[derive(Clone)]
pub enum MountType {
    Regular,
    Bind {
        source_mount_node: Arc<MountNode>,
        source_relative_path: String,
        bind_type: BindType,
    },
    Overlay {
        lower_layers: Vec<String>,
        upper_layer: String,
        work_dir: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BindType {
    ReadOnly,
    ReadWrite,
    Shared,
}

#[derive(Clone)]
pub struct MountOptions {
    pub read_only: bool,
    pub no_exec: bool,
    pub no_suid: bool,
    pub no_dev: bool,
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
    pub fn new() -> Self {
        Self {
            root: Arc::new(MountNode::new("".to_string())),
            path_cache: BTreeMap::new(),
        }
    }
    
    /// Add mount point (used by VfsManager)
    pub fn mount(&mut self, path: &str, mount_point: MountPoint) -> Result<()> {
        self.insert(path, mount_point)
    }
    
    /// Add mount point
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
    
    /// Remove mount point
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
    
    /// List all mount points
    pub fn list_all(&self) -> Vec<String> {
        let mut paths = Vec::new();
        self.collect_mount_paths(&self.root, String::new(), &mut paths);
        paths
    }
    
    /// Get number of mount points
    pub fn len(&self) -> usize {
        self.path_cache.len()
    }
    
    /// Secure path normalization
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
