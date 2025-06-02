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

struct MountNode {
    /// Path component
    component: RwLock<String>,
    /// Mount information if this node is a mount point
    mount_point: RwLock<Option<Arc<MountPoint>>>,
    /// Child nodes
    children: RwLock<BTreeMap<String, Arc<MountNode>>>,
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

#[derive(Clone)]
pub enum MountType {
    Regular,
    Bind {
        source_vfs: Option<Arc<VfsManager>>,
        source_path: String,
        bind_type: BindType,
    },
    Overlay {
        lower_layers: Vec<String>,
        upper_layer: String,
        work_dir: String,
    },
    Tmpfs {
        memory_limit: usize,
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
    
    /// Path resolution (efficient O(log k) implementation)
    pub fn resolve(&self, path: &str) -> Result<(Arc<MountPoint>, String)> {
        let normalized = Self::normalize_path(path)?;
        // Try direct cache lookup
        if let Some(mount) = self.path_cache.get(&normalized) {
            return Ok((mount.clone(), "/".to_string()));
        }
        
        let components = self.split_path(&normalized);
        
        let mut current_arc = self.root.clone();
        let mut best_match: Option<Arc<MountPoint>> = None;
        let mut match_depth = 0;
        
        // Search for longest match using Trie structure
        for (depth, component) in components.iter().enumerate() {
            // Check if current node has a mount point
            {
                let mount_guard = current_arc.mount_point.read();
                if let Some(mount) = &*mount_guard {
                    best_match = Some(mount.clone());
                    match_depth = depth;
                }
            }
            
            // Move to next node
            let next_arc = {
                let children_guard = current_arc.children.read();
                if let Some(child) = children_guard.get(component) {
                    child.clone()
                } else {
                    break;
                }
            };
            current_arc = next_arc;
        }
        
        let mount = best_match.ok_or(FileSystemError {
            kind: FileSystemErrorKind::NotFound,
            message: format!("No mount point found for path: {}", path),
        })?;

        // Build relative path
        let relative_components = &components[match_depth..];
        let relative_path = if relative_components.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", relative_components.join("/"))
        };

        Ok((mount, relative_path))
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

impl MountNode {
    fn new(component: String) -> Self {
        Self {
            component: RwLock::new(component),
            mount_point: RwLock::new(None),
            children: RwLock::new(BTreeMap::new()),
        }
    }
}

impl Default for MountTree {
    fn default() -> Self {
        Self::new()
    }
}
