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
    /// Parent entry
    pub parent_entry: Option<VfsEntryRef>,
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
            parent_entry: None,
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
            parent_entry: None,
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
            parent_entry: None,
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
        let mut_child: *const MountPoint = Arc::as_ptr(&child);
        unsafe {
            let mut_child = mut_child as *mut MountPoint;
            (*mut_child).parent = Some(Arc::downgrade(self));
        }
        let child_name = child.path.clone();
        self.children.write().insert(child_name, child);
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
    /// Root mount point (can be updated when mounting at "/")
    pub root_mount: RwLock<Arc<MountPoint>>,
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
            root_mount: RwLock::new(root_mount),
            mounts: RwLock::new(mounts),
        }
    }

    /// Mount a filesystem at the specified path
    pub fn mount(&self, path: &str, root: VfsEntryRef) -> VfsResult<MountId> {
        // Special case: mounting at root ("/") replaces the root mount
        if path == "/" {
            // Create new root mount
            let new_root_mount = MountPoint::new_regular("/".to_string(), root.clone());
            let mount_id = new_root_mount.id;
            
            // Replace root_mount (unsafe operation for self-modification)
            *self.root_mount.write() = new_root_mount.clone();
            
            // Update global mount table
            let old_root_id = self.root_mount.read().id;
            let mut mounts = self.mounts.write();
            mounts.remove(&old_root_id);
            mounts.insert(mount_id, Arc::downgrade(&new_root_mount));
            
            return Ok(mount_id);
        }
        
        let mount_point = self.find_mount_point_for_path(path)?;
        let relative_path = self.get_relative_path(&mount_point, path)?;
        
        // Create new mount
        let new_mount = MountPoint::new_regular(relative_path.clone(), root.clone());
        let mount_id = new_mount.id;

        // Add to parent's children
        debug_assert!(new_mount.parent.is_none(), "new_mount.parent should be None before add_child");
        mount_point.add_child(new_mount.clone())?;
        debug_assert!(new_mount.parent.is_some(), "new_mount.parent should be Some after add_child");
        debug_assert!(Arc::ptr_eq(&mount_point, &new_mount.parent.as_ref().unwrap().upgrade().unwrap()), "new_mount.parent must point to mount_point");

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
        // crate::println!("Resolving path: '{}'", path);
        if path.is_empty() || path == "/" {
            return Ok(self.root_mount.read().root.clone());
        }

        let components = self.parse_path(path);
        let mut current_mount = self.root_mount.read().clone();
        let mut current_entry = current_mount.root.clone();
        
        // crate::println!("Resolving components: '{:?}'", components);

        for component in components {
            if component == ".." {
                // Check if we're at a mount point root before deciding how to handle ".."
                if Arc::ptr_eq(&current_entry, &current_mount.root) {
                    // At mount root - handle mount boundary crossing
                    current_entry = self.resolve_parent_with_mount_crossing(
                        current_mount.clone(),
                        current_entry,
                    )?;
                    // Update current_mount in case we crossed mount boundary
                    current_mount = self.find_mount_for_entry(&current_entry)?;
                } else {
                    // Not at mount root - delegate to filesystem via regular path resolution
                    current_entry = self.resolve_component(current_entry, &component)?;
                }
            } else if let Some(child_mount) = current_mount.get_child(&component) {
                // Cross mount boundary
                current_mount = child_mount;
                current_entry = current_mount.root.clone();
            } else {
                // Regular path traversal within current mount
                current_entry = self.resolve_component(current_entry, &component)?;
            }
        }

        // crate::println!("Resolved path '{}' to entry: {:?}", path, current_entry);

        Ok(current_entry)
    }

    /// Resolve parent directory with mount boundary crossing support
    /// This function is called when current_entry is already confirmed to be at mount root
    fn resolve_parent_with_mount_crossing(
        &self,
        current_mount: Arc<MountPoint>,
        _current_entry: VfsEntryRef, // We know this is current_mount.root
    ) -> VfsResult<VfsEntryRef> {
        // At mount root - need to cross to parent mount
        if let Some(parent_mount_weak) = &current_mount.parent {
            if let Some(parent_mount) = parent_mount_weak.upgrade() {
                // Find the mount point entry in parent mount
                return self.find_mount_point_entry(&parent_mount, &current_mount.path);
            }
        }
        // No parent mount - stay at current mount root (this is the VFS root)
        Ok(current_mount.root.clone())
    }

    /// Find mount point entry in parent mount
    fn find_mount_point_entry(
        &self,
        parent_mount: &Arc<MountPoint>,
        _mount_path: &str,
    ) -> VfsResult<VfsEntryRef> {
        // This is a simplified implementation
        // In practice, we'd need to resolve the mount path in the parent mount
        // For now, return the parent mount root
        Ok(parent_mount.root.clone())
    }

    /// Find which mount contains the given entry
    fn find_mount_for_entry(&self, _entry: &VfsEntryRef) -> VfsResult<Arc<MountPoint>> {
        // This is a simplified implementation
        // In practice, we'd need to traverse up the entry hierarchy to find the mount root
        // For now, return root mount
        Ok(self.root_mount.read().clone())
    }

    /// Check if a path is a mount point
    pub fn is_mount_point(&self, entry: VfsEntryRef) -> VfsResult<bool> {
        // Check if the entry is a mount point by looking up its parent
        if let Some(parent) = entry.parent() {
            let parent_mount = self.find_mount_for_entry(&parent)?;
            Ok(parent_mount.get_child(&entry.name()).is_some())
        } else {
            // If no parent, it cannot be a mount point
            Ok(false)
        }
    }

    /// Get mount information for a path
    pub fn get_mount_info(&self, entry: VfsEntryRef) -> VfsResult<MountId> {
        // Check if the entry is a mount point
        if self.is_mount_point(entry.clone())? {
            // Find the mount point for this entry
            let mount = self.find_mount_for_entry(&entry)?;
            Ok(mount.id)
        } else {
            Err(vfs_error(FileSystemErrorKind::NotFound, "Entry is not a mount point"))
        }
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

    // /// Find mount ID for a given path
    // pub fn find_mount_id_by_path(&self, path: &str) -> Option<MountId> {
    //     let components = self.parse_path(path);
    //     let mut current_mount = self.root_mount.read().clone();

    //     for component in components {
    //         if let Some(child_mount) = current_mount.get_child(&component) {
    //             current_mount = child_mount;
    //         } else {
    //             // Path doesn't correspond to a mount point
    //             return None;
    //         }
    //     }

    //     Some(current_mount.id)
    // }

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
        let mut current_mount = self.root_mount.read().clone();

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
        // Handle special cases
        if component == "." {
            return Ok(entry);
        }
        
        // For "..", this should have been handled at the caller level
        // If we reach here with "..", it's a programming error
        if component == ".." {
            return Err(vfs_error(FileSystemErrorKind::InvalidPath, 
                ".. should be handled by mount boundary logic"));
        }

        // Check cache first (fast path)
        let component_string = component.to_string();
        if let Some(cached_child) = entry.get_child(&component_string) {
            return Ok(cached_child);
        }

        // Cache miss - perform filesystem lookup
        let parent_node = entry.node();
        debug_assert!(parent_node.filesystem().is_some(), "resolve_component: parent_node.filesystem() is None");
        let filesystem = parent_node.filesystem()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| vfs_error(FileSystemErrorKind::NotSupported, "No filesystem reference"))?;
        // Ask filesystem to lookup the component
        let child_node = filesystem.lookup(parent_node, &component_string)
            .map_err(|e| vfs_error(e.kind, &e.message))?;

        // Create new VfsEntry for the child
        let child_entry = VfsEntry::new(
            Some(Arc::downgrade(&entry)),
            component_string.clone(),
            child_node,
        );

        // Add to parent's cache
        entry.add_child(component_string, Arc::clone(&child_entry));

        Ok(child_entry)
    }
}