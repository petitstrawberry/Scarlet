//! VFS v2 Mount Tree Implementation
//! 
//! This module provides a new mount tree architecture for VFS v2 that supports:
//! - Hierarchical mount points with parent-child relationships
//! - Bind mounts and overlay mounts
//! - Proper path resolution across mount boundaries
//! - Efficient mount point lookup and traversal

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::sync::{Arc, Weak};
use alloc::format;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::RwLock;

use super::core::VfsEntry;
use crate::fs::{FileSystemError, FileSystemErrorKind};

pub type VfsResult<T> = Result<T, FileSystemError>;
pub type VfsEntryRef = Arc<VfsEntry>;

// Helper function to create FileSystemError
fn vfs_error(kind: FileSystemErrorKind, message: &str) -> FileSystemError {
    FileSystemError::new(kind, message)
}

/// Unique identifier for mount points
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MountId(u64);

impl MountId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Type of mount operation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountType {
    /// Regular mount
    Regular,
    /// Bind mount (mount existing directory at another location)
    Bind,
    /// Overlay mount (overlay multiple directories)
    Overlay,
}

/// Mount options (for compatibility with manager_v2.rs)
#[derive(Debug, Clone, Default)]
pub struct MountOptionsV2 {
    pub readonly: bool,
    pub flags: u32,
}

/// Bind mount type (for compatibility with manager_v2.rs)  
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindType {
    Regular,
    Recursive,
}

/// Mount point information
#[derive(Debug)]
pub struct MountPoint {
    /// Unique mount ID
    pub id: MountId,
    /// Type of mount
    pub mount_type: MountType,
    /// Mount path (relative to parent mount)
    pub path: String,
    /// Root entry of the mounted filesystem
    pub root: VfsEntryRef,
    /// Parent mount (weak reference to avoid cycles)
    pub parent: Option<Weak<MountPoint>>,
    /// Child mounts
    pub children: RwLock<BTreeMap<String, Arc<MountPoint>>>,
    /// For bind mounts: the original entry being bound
    pub bind_source: Option<VfsEntryRef>,
    /// For overlay mounts: the list of overlay layers
    pub overlay_layers: Vec<VfsEntryRef>,
}

impl MountPoint {
    /// Create a new regular mount point
    pub fn new_regular(path: String, root: VfsEntryRef) -> Arc<Self> {
        Arc::new(Self {
            id: MountId::new(),
            mount_type: MountType::Regular,
            path,
            root,
            parent: None,
            children: RwLock::new(BTreeMap::new()),
            bind_source: None,
            overlay_layers: Vec::new(),
        })
    }

    /// Create a new bind mount point
    pub fn new_bind(path: String, source: VfsEntryRef) -> Arc<Self> {
        Arc::new(Self {
            id: MountId::new(),
            mount_type: MountType::Bind,
            path,
            root: source.clone(),
            parent: None,
            children: RwLock::new(BTreeMap::new()),
            bind_source: Some(source),
            overlay_layers: Vec::new(),
        })
    }

    /// Create a new overlay mount point
    pub fn new_overlay(path: String, layers: Vec<VfsEntryRef>) -> VfsResult<Arc<Self>> {
        if layers.is_empty() {
            return Err(vfs_error(FileSystemErrorKind::InvalidPath, "Overlay mount requires at least one layer"));
        }

        // Use the top layer as the root
        let root = layers[0].clone();

        Ok(Arc::new(Self {
            id: MountId::new(),
            mount_type: MountType::Overlay,
            path,
            root,
            parent: None,
            children: RwLock::new(BTreeMap::new()),
            bind_source: None,
            overlay_layers: layers,
        }))
    }

    /// Get the parent mount point
    pub fn get_parent(&self) -> Option<Arc<MountPoint>> {
        self.parent.as_ref().and_then(|weak| weak.upgrade())
    }

    /// Check if this is the root mount
    pub fn is_root_mount(&self) -> bool {
        self.parent.is_none()
    }

    /// Get child mount by name
    pub fn get_child(&self, name: &str) -> Option<Arc<MountPoint>> {
        self.children.read().get(name).cloned()
    }

    /// Add a child mount
    pub fn add_child(self: &Arc<Self>, child: Arc<MountPoint>) -> VfsResult<()> {
        // Set parent reference in child
        let child_arc = child.clone();
        if let Some(child_mut) = Arc::get_mut(&mut child.clone()) {
            child_mut.parent = Some(Arc::downgrade(self));
        }

        let child_name = child_arc.path.clone();
        self.children.write().insert(child_name, child_arc);
        Ok(())
    }

    /// Remove a child mount
    pub fn remove_child(&self, name: &str) -> Option<Arc<MountPoint>> {
        self.children.write().remove(name)
    }

    /// List all child mount names
    pub fn list_children(&self) -> Vec<String> {
        self.children.read().keys().cloned().collect()
    }
}

/// Mount tree manager for VFS v2
#[derive(Debug)]
pub struct MountTree {
    /// Root mount point
    root_mount: Arc<MountPoint>,
    /// All mounts indexed by ID for quick lookup
    mounts: RwLock<BTreeMap<MountId, Weak<MountPoint>>>,
}

impl MountTree {
    /// Create a new mount tree with the given root
    pub fn new(root_entry: VfsEntryRef) -> Self {
        let root_mount = MountPoint::new_regular("/".to_string(), root_entry);
        let root_id = root_mount.id;
        
        let mut mounts = BTreeMap::new();
        mounts.insert(root_id, Arc::downgrade(&root_mount));

        Self {
            root_mount,
            mounts: RwLock::new(mounts),
        }
    }

    /// Get the root mount point
    pub fn root_mount(&self) -> &Arc<MountPoint> {
        &self.root_mount
    }

    /// Get the root entry of the root mount
    pub fn root_entry(&self) -> VfsEntryRef {
        self.root_mount.root.clone()
    }

    /// Mount a filesystem at the specified path
    pub fn mount(&self, path: &str, root: VfsEntryRef) -> VfsResult<MountId> {
        let mount_point = self.find_mount_point_for_path(path)?;
        let relative_path = self.get_relative_path(&mount_point, path)?;
        
        // Create new mount
        let new_mount = MountPoint::new_regular(relative_path.clone(), root);
        let mount_id = new_mount.id;

        // Add to parent's children
        mount_point.add_child(new_mount.clone())?;

        // Register in global mount table
        self.mounts.write().insert(mount_id, Arc::downgrade(&new_mount));

        Ok(mount_id)
    }

    /// Create a bind mount
    pub fn bind_mount(&self, source_path: &str, target_path: &str) -> VfsResult<MountId> {
        // Find the source entry
        let source_entry = self.resolve_path(source_path)?;
        
        // Find target mount point
        let target_mount = self.find_mount_point_for_path(target_path)?;
        let relative_path = self.get_relative_path(&target_mount, target_path)?;

        // Create bind mount
        let bind_mount = MountPoint::new_bind(relative_path.clone(), source_entry);
        let mount_id = bind_mount.id;

        // Add to parent's children
        target_mount.add_child(bind_mount.clone())?;

        // Register in global mount table
        self.mounts.write().insert(mount_id, Arc::downgrade(&bind_mount));

        Ok(mount_id)
    }

    /// Create an overlay mount
    pub fn overlay_mount(&self, layers: Vec<&str>, target_path: &str) -> VfsResult<MountId> {
        // Resolve all layer paths
        let mut layer_entries = Vec::new();
        for layer_path in layers {
            let entry = self.resolve_path(layer_path)?;
            layer_entries.push(entry);
        }

        // Find target mount point
        let target_mount = self.find_mount_point_for_path(target_path)?;
        let relative_path = self.get_relative_path(&target_mount, target_path)?;

        // Create overlay mount
        let overlay_mount = MountPoint::new_overlay(relative_path.clone(), layer_entries)?;
        let mount_id = overlay_mount.id;

        // Add to parent's children
        target_mount.add_child(overlay_mount.clone())?;

        // Register in global mount table
        self.mounts.write().insert(mount_id, Arc::downgrade(&overlay_mount));

        Ok(mount_id)
    }

    /// Unmount a filesystem
    pub fn unmount(&self, mount_id: MountId) -> VfsResult<()> {
        // Find the mount point
        let mount = {
            let mounts = self.mounts.read();
            let weak_mount = mounts.get(&mount_id)
                .ok_or_else(|| vfs_error(FileSystemErrorKind::NotFound, "Mount not found"))?;
            weak_mount.upgrade()
                .ok_or_else(|| vfs_error(FileSystemErrorKind::NotFound, "Mount no longer exists"))?
        };

        // Cannot unmount root
        if mount.is_root_mount() {
            return Err(vfs_error(FileSystemErrorKind::InvalidPath, "Cannot unmount root filesystem"));
        }

        // Check if mount has children (busy)
        if !mount.children.read().is_empty() {
            return Err(vfs_error(FileSystemErrorKind::NotSupported, "Mount point has child mounts"));
        }

        // Remove from parent
        if let Some(parent) = mount.get_parent() {
            parent.remove_child(&mount.path);
        }

        // Remove from global mount table
        self.mounts.write().remove(&mount_id);

        Ok(())
    }

    /// Resolve a path to a VFS entry, handling mount boundaries
    pub fn resolve_path(&self, path: &str) -> VfsResult<VfsEntryRef> {
        if path.is_empty() || path == "/" {
            return Ok(self.root_mount.root.clone());
        }

        let components = self.parse_path(path);
        let mut current_mount = self.root_mount.clone();
        let mut current_entry = current_mount.root.clone();

        for component in components {
            // Check for mount points at current location
            if let Some(child_mount) = current_mount.get_child(&component) {
                // Cross mount boundary
                current_mount = child_mount;
                current_entry = current_mount.root.clone();
            } else {
                // Regular path traversal within current mount
                current_entry = self.resolve_component(current_entry, &component)?;
            }
        }

        Ok(current_entry)
    }

    /// Check if a path is a mount point
    pub fn is_mount_point(&self, path: &str) -> VfsResult<bool> {
        let mount_point = self.find_mount_point_for_path(path)?;
        let relative_path = self.get_relative_path(&mount_point, path)?;
        
        Ok(mount_point.get_child(&relative_path).is_some())
    }

    /// Get mount information for a path
    pub fn get_mount_info(&self, path: &str) -> VfsResult<MountId> {
        let mount_point = self.find_mount_point_for_path(path)?;
        Ok(mount_point.id)
    }

    /// List all mounts
    pub fn list_mounts(&self) -> Vec<(MountId, String, MountType)> {
        let mut result = Vec::new();
        let mounts = self.mounts.read();
        
        for (id, weak_mount) in mounts.iter() {
            if let Some(mount) = weak_mount.upgrade() {
                let full_path = self.get_mount_path(&mount);
                result.push((*id, full_path, mount.mount_type.clone()));
            }
        }
        
        result
    }

    /// Find mount ID for a given path
    pub fn find_mount_id_by_path(&self, path: &str) -> Option<MountId> {
        let components = self.parse_path(path);
        let mut current_mount = self.root_mount.clone();

        for component in components {
            if let Some(child_mount) = current_mount.get_child(&component) {
                current_mount = child_mount;
            } else {
                // Path doesn't correspond to a mount point
                return None;
            }
        }

        Some(current_mount.id)
    }

    // Helper methods

    /// Parse a path into components
    pub fn parse_path(&self, path: &str) -> Vec<String> {
        path.split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .map(|s| s.to_string())
            .collect()
    }

    /// Find the mount point that should contain the given path
    fn find_mount_point_for_path(&self, path: &str) -> VfsResult<Arc<MountPoint>> {
        let components = self.parse_path(path);
        let mut current_mount = self.root_mount.clone();

        for component in components {
            if let Some(child_mount) = current_mount.get_child(&component) {
                current_mount = child_mount;
            } else {
                break;
            }
        }

        Ok(current_mount)
    }

    /// Get the relative path within a mount point
    fn get_relative_path(&self, mount: &Arc<MountPoint>, full_path: &str) -> VfsResult<String> {
        let mount_path = self.get_mount_path(mount);
        
        if full_path == mount_path {
            return Ok(".".to_string());
        }

        if !full_path.starts_with(&mount_path) {
            return Err(vfs_error(FileSystemErrorKind::InvalidPath, "Path not within mount"));
        }

        let relative = &full_path[mount_path.len()..];
        Ok(relative.trim_start_matches('/').to_string())
    }

    /// Get the full path of a mount point
    fn get_mount_path(&self, mount: &Arc<MountPoint>) -> String {
        if mount.is_root_mount() {
            return "/".to_string();
        }

        let mut components = Vec::new();
        let mut current = Some(mount.clone());

        while let Some(mount) = current {
            if !mount.is_root_mount() {
                components.push(mount.path.clone());
                current = mount.get_parent();
            } else {
                break;
            }
        }

        components.reverse();
        if components.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", components.join("/"))
        }
    }

    /// Resolve a single path component within a VFS entry
    fn resolve_component(&self, entry: VfsEntryRef, component: &str) -> VfsResult<VfsEntryRef> {
        if component == "." {
            return Ok(entry);
        }

        if component == ".." {
            // Handle parent directory traversal
            // This is simplified - in a real implementation, you'd need to handle
            // mount boundaries properly for ".." traversal
            if let Some(parent) = entry.parent() {
                return Ok(parent);
            } else {
                return Err(vfs_error(FileSystemErrorKind::NotFound, "No parent directory"));
            }
        }

        // Look up the component in the current directory
        // For now, return a simple error - this needs proper VfsEntry lookup implementation
        Err(vfs_error(FileSystemErrorKind::NotSupported, "Component lookup not implemented"))
    }
}