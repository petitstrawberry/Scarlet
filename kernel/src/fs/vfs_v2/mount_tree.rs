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

use super::core::{VfsEntry, FileSystemOperations};
use crate::fs::{FileSystemError, FileSystemErrorKind};

pub type VfsResult<T> = Result<T, FileSystemError>;
pub type VfsEntryRef = Arc<VfsEntry>;
pub type VfsEntryWeakRef = Weak<VfsEntry>;


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
    /// Parent entry (strong reference to the VFS entry at the mount point to ensure it stays alive)
    pub parent_entry: Option<VfsEntryRef>,
    /// Child mounts: map of VfsEntry ID to MountPoint
    pub children: RwLock<BTreeMap<u64, Arc<MountPoint>>>,
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

    /// Get child mount by VfsEntry
    pub fn get_child(&self, entry: &VfsEntryRef) -> Option<Arc<MountPoint>> {
        let key = entry.node().id();
        self.children.read().get(&key).cloned()
    }

    /// Add a child mount by VfsEntry
    pub fn add_child(self: &Arc<Self>, entry: &VfsEntryRef, child: Arc<MountPoint>) -> VfsResult<()> {
        // Set parent reference in child
        let mut_child: *const MountPoint = Arc::as_ptr(&child);
        unsafe {
            let mut_child = mut_child as *mut MountPoint;
            (*mut_child).parent = Some(Arc::downgrade(self));
            (*mut_child).parent_entry = Some(entry.clone());
        }
        let key = entry.node().id();
        self.children.write().insert(key, child);
        Ok(())
    }

    /// Remove a child mount by VfsEntry
    pub fn remove_child(&self, entry: &VfsEntryRef) -> Option<Arc<MountPoint>> {
        let key = entry.node().id();
        self.children.write().remove(&key)
    }

    /// List all child mount IDs
    pub fn list_children(&self) -> Vec<u64> {
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
            root_mount: RwLock::new(root_mount.clone()),
            mounts: RwLock::new(mounts),
        }
    }

    /// Create a bind mount.
    ///
    /// # Arguments
    /// * `source_entry` - The VFS entry to be mounted.
    /// * `target_entry` - The VFS entry where the source will be mounted.
    /// * `target_mount_point` - The mount point containing the target entry.
    pub fn bind_mount(
        &self,
        source_entry: VfsEntryRef,
        target_entry: VfsEntryRef,
        target_mount_point: Arc<MountPoint>,
    ) -> VfsResult<MountId> {
        // Create a new bind mount point. The name of the mount point is the name of the target entry.
        let bind_mount = MountPoint::new_bind(target_entry.name().clone(), source_entry);
        let mount_id = bind_mount.id;

        // Add the new mount as a child of the target's containing mount point, attached to the target entry.
        target_mount_point.add_child(&target_entry, bind_mount.clone())?;

        // Register the new mount in the global mount table.
        self.mounts.write().insert(mount_id, Arc::downgrade(&bind_mount));

        Ok(mount_id)
    }

    /// Mount a filesystem at a specific entry in the mount tree.
    pub fn mount(
        &self,
        target_entry: VfsEntryRef,
        target_mount_point: Arc<MountPoint>,
        filesystem: Arc<dyn FileSystemOperations>,
    ) -> VfsResult<MountId> {
        // The root of the new filesystem.
        let new_fs_root_node = filesystem.root_node();

        // Create a VfsEntry for the root of the new filesystem.
        let new_fs_root_entry = VfsEntry::new(None, "/".to_string(), new_fs_root_node);

        // Create a new mount point for the filesystem.
        let new_mount = MountPoint::new_regular(target_entry.name().clone(), new_fs_root_entry);
        let mount_id = new_mount.id;

        // Add the new mount as a child to the target's mount point.
        target_mount_point.add_child(&target_entry, new_mount.clone())?;

        // Register the new mount in the global mount table.
        self.mounts.write().insert(mount_id, Arc::downgrade(&new_mount));

        Ok(mount_id)
    }

    /// Replaces the root mount point.
    pub fn replace_root(&self, new_root: Arc<MountPoint>) {
        let old_root_id = self.root_mount.read().id;
        *self.root_mount.write() = new_root.clone();
        let mut mounts = self.mounts.write();
        mounts.remove(&old_root_id);
        mounts.insert(new_root.id, Arc::downgrade(&new_root));
    }

    /// Check if a path is a mount point
    pub fn is_mount_point(&self, entry_to_check: &VfsEntryRef) -> bool {
        crate::println!("Checking if entry is a mount point: {}", entry_to_check.name());
        let node_to_check = entry_to_check.node();
        let node_id = node_to_check.id();
        
        let fs_ptr_to_check = match node_to_check.filesystem().and_then(|w| w.upgrade()) {
            Some(fs) => Arc::as_ptr(&fs) as *const (),
            None => return false,
        };

        for mount in self.mounts.read().values().filter_map(|w| w.upgrade()) {
            if let Some(parent_entry) = &mount.parent_entry {
                if parent_entry.node().id() == node_id {
                    let parent_fs_ptr = parent_entry.node().filesystem().and_then(|w| w.upgrade())
                        .map(|fs| Arc::as_ptr(&fs) as *const ());
                    if parent_fs_ptr == Some(fs_ptr_to_check) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if an entry is a source for a bind mount
    pub fn is_bind_source(&self, entry_to_check: &VfsEntryRef) -> bool {
        let node_to_check = entry_to_check.node();
        let node_id = node_to_check.id();
        
        let fs_ptr_to_check = match node_to_check.filesystem().and_then(|w| w.upgrade()) {
            Some(fs) => Arc::as_ptr(&fs) as *const (),
            None => return false,
        };

        for mount in self.mounts.read().values().filter_map(|w| w.upgrade()) {
            if let Some(bind_source) = &mount.bind_source {
                if bind_source.node().id() == node_id {
                    let source_fs_ptr = bind_source.node().filesystem().and_then(|w| w.upgrade())
                        .map(|fs| Arc::as_ptr(&fs) as *const ());
                    if source_fs_ptr == Some(fs_ptr_to_check) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Check if an entry is used in a mount (either as a mount point or a bind source)
    pub fn is_entry_used_in_mount(&self, entry_to_check: &VfsEntryRef) -> bool {
        self.is_mount_point(entry_to_check) || self.is_bind_source(entry_to_check)
    }

    /// Unmount a filesystem
    pub fn unmount(&self, entry: &VfsEntryRef) -> VfsResult<()> {
        // Find the mount point
        let mount = {
            let mounts = self.mounts.read();
            mounts.values()
                .filter_map(|w| w.upgrade())
                .find(|m| m.root.node().id() == entry.node().id())
                .ok_or_else(|| vfs_error(FileSystemErrorKind::NotFound, "Mount point not found"))?
        };

        // Cannot unmount root
        if mount.is_root_mount() {
            return Err(vfs_error(FileSystemErrorKind::InvalidPath, "Cannot unmount root filesystem"));
        }

        crate::println!("Children of mount point {}: {:?}", mount.path, mount.list_children());

        // Check if mount has children (busy)
        if !mount.children.read().is_empty() {
            return Err(vfs_error(FileSystemErrorKind::NotSupported, "Mount point has child mounts"));
        }

        // Remove from parent
        if let Some(parent) = mount.get_parent() {
            parent.remove_child(&mount.root);
        }

        // Remove from global mount table
        self.mounts.write().remove(&mount.id);

        Ok(())
    }

    /// Resolve a path to a VFS entry, handling mount boundaries
    pub fn resolve_path(&self, path: &str) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        self.resolve_path_internal(path, false)
    }

    /// Resolve a path to the mount point entry (not the mounted content)
    /// This is used for unmount operations where we need the actual mount point
    pub fn resolve_mount_point(&self, path: &str) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        self.resolve_path_internal(path, true)
    }

    fn resolve_path_internal(&self, path: &str, resolve_mount: bool) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        if path.is_empty() || path == "/" {
            return Ok((self.root_mount.read().root.clone(), self.root_mount.read().clone()));
        }

        let components = self.parse_path(path);
        let mut current_mount = self.root_mount.read().clone();
        let mut current_entry = current_mount.root.clone();
        
        let mut resolved_path = String::new();
        for (i, component) in components.iter().enumerate() {
            if component == ".." {
                // crate::println!("Processing '..' - current_mount: {:?}, current_entry: {:?}", current_mount.path, current_entry.name());
                
                // Check if current entry points to the root node of current mount
                let is_at_mount_root = current_entry.node().id() == current_mount.root.node().id();
                // crate::println!("Is at mount root? {}", is_at_mount_root);
                
                // Handle parent directory traversal
                if is_at_mount_root {
                    // We're at the root of current mount - go to parent mount regardless of mount type
                    let parent_info = current_mount.get_parent().zip(current_mount.parent_entry.clone());
                    match parent_info {
                        Some((parent_mount, parent_entry)) => {
                            // crate::println!("Moving to parent mount: {:?}", parent_mount.path);
                            current_mount = parent_mount;
                            // Resolve ".." from the mount point in the parent mount
                            current_entry = self.resolve_component(parent_entry, &"..")?;
                        },
                        None => {
                            // No parent mount - stay at current mount root (this is the VFS root)
                            // crate::println!("No parent mount - staying at root");
                        }
                    }
                } else {
                    // Not at mount root - use normal filesystem navigation
                    // crate::println!("Not at mount root - resolving within filesystem");
                    current_entry = self.resolve_component(current_entry, &component)?;
                }
            } else {
                // Regular path traversal within current mount
                current_entry = self.resolve_component(current_entry, &component)?;

                // Check if we've reached a mount point but this is the final component
                // If so, return the mount point entry itself, not the mounted content
                if resolve_mount && i == components.len() - 1 {
                    // This is a mount point - return the mount point entry and the parent mount
                    if let Some(_child_mount) = current_mount.get_child(&current_entry) {
                        return Ok((current_entry, current_mount));
                    }
                } else {
                    // Not the final component - cross mount boundaries normally
                    if let Some(child_mount) = current_mount.get_child(&current_entry) {
                        current_mount = child_mount;
                        current_entry = current_mount.root.clone();
                    }
                }
            }

            resolved_path.push('/');
            resolved_path.push_str(&component);
            crate::println!("Resolved path: {}", resolved_path);
        }

        Ok((current_entry, current_mount))
    }

    /// Get mount information for a path
    pub fn get_mount_info(&self, entry: VfsEntryRef) -> VfsResult<MountId> {
        // Check if the entry is a mount point
        if self.is_mount_point(&entry) {
            // Find the mount point for this entry
            for mount in self.mounts.read().values().filter_map(|w| w.upgrade()) {
                if let Some(parent_entry) = &mount.parent_entry {
                    if Arc::ptr_eq(&entry, parent_entry) {
                        return Ok(mount.id);
                    }
                }
            }
            Err(vfs_error(FileSystemErrorKind::NotFound, "Mount not found"))
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

    // Helper methods

    /// Parse a path into components
    pub fn parse_path(&self, path: &str) -> Vec<String> {
        path.split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .map(|s| s.to_string())
            .collect()
    }

    // /// Find the mount point that should contain the given path
    // fn find_mount_point_for_path(&self, path: &str) -> VfsResult<Arc<MountPoint>> {
    //     let components = self.parse_path(path);
    //     let mut current_mount = self.root_mount.read().clone();

    //     for component in components {
    //         if let Some(child_mount) = current_mount.get_child(&component) {
    //             current_mount = child_mount;
    //         } else {
    //             break;
    //         }
    //     }

    //     Ok(current_mount)
    // }

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

        // Check cache first (fast path)
        let component_string = component.to_string();
        if let Some(cached_child) = entry.get_child(&component_string) {
            // crate::println!("Cache hit for component '{}'", component_string);
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
        entry.add_child(component_string, child_entry.clone());

        Ok(child_entry)
    }
}