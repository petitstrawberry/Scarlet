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
use super::manager::{VfsManager, PathResolutionOptions};
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

/// Unique identifier for VfsManager instances
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VfsManagerId(u64);

impl VfsManagerId {
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// Type of mount operation
#[derive(Debug, Clone)]
pub enum MountType {
    /// Regular mount
    Regular,
    /// Bind mount (mount existing directory at another location)
    Bind,
    /// Overlay mount (overlay multiple directories)
    Overlay {
        /// The list of overlay layers (top to bottom priority)
        layers: Vec<VfsEntryRef>,
    },
}

/// Mount options (for compatibility with manager_v2.rs)
#[derive(Debug, Clone, Default)]
pub struct MountOptionsV2 {
    pub readonly: bool,
    pub flags: u32,
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
    /// Child mounts: shared map of VfsEntry ID to MountPoint
    pub children: Arc<RwLock<BTreeMap<u64, Arc<MountPoint>>>>,
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
            children: Arc::new(RwLock::new(BTreeMap::new())),
        })
    }

    /// Create a new bind mount point
    pub fn new_bind(path: String, source: VfsEntryRef) -> Arc<Self> {
        Arc::new(Self {
            id: MountId::new(),
            mount_type: MountType::Bind,
            path,
            root: source,
            parent: None,
            parent_entry: None,
            children: Arc::new(RwLock::new(BTreeMap::new())),
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
            mount_type: MountType::Overlay {
                layers: layers.clone(),
            },
            path,
            root,
            parent: None,
            parent_entry: None,
            children: Arc::new(RwLock::new(BTreeMap::new())),
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

    /// Check if this mount point is a bind mount
    pub fn is_bind_mount(&self) -> bool {
        matches!(self.mount_type, MountType::Bind { .. })
    }

    /// Get the bind source entry (for regular bind mounts only)
    pub fn get_bind_source(&self) -> Option<VfsEntryRef> {
        match &self.mount_type {
            MountType::Bind { .. } => Some(self.root.clone()),
            _ => None,
        }
    }

    /// Get cross-VFS bind information
    pub fn get_cross_vfs_info(&self) -> Option<(Weak<VfsManager>, &str, u64)> {
        match &self.mount_type {
            MountType::Bind { .. } => {
                None
            }
            _ => None,
        }
    }
}

/// Mount tree manager for VFS v2
#[derive(Debug)]
pub struct MountTree {
    /// Root mount point (can be updated when mounting at "/")
    pub root_mount: RwLock<Arc<MountPoint>>,
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

        Ok(mount_id)
    }

    /// Replaces the root mount point.
    pub fn replace_root(&self, new_root: Arc<MountPoint>) {
        *self.root_mount.write() = new_root.clone();
    }

    /// Check if a path is a mount point
    /// 
    /// # Arguments
    /// * `entry_to_check` - The VFS entry to check if it is a mount point.
    /// * `mount_point_to_check` - The mount point to check against.
    /// 
    /// # Notes
    /// `entry_to_check` and `mount_point_to_check` should be in the same mount point.
    pub fn is_mount_point(&self, entry_to_check: &VfsEntryRef, mount_point_to_check: &Arc<MountPoint>) -> bool {
        // let node_to_check = entry_to_check.node();
        // let node_id = node_to_check.id();
        
        // let fs_ptr_to_check = match node_to_check.filesystem().and_then(|w| w.upgrade()) {
        //     Some(fs) => Arc::as_ptr(&fs) as *const (),
        //     None => return false,
        // };

        // for mount in self.mounts.read().values().filter_map(|w| w.upgrade()) {
        //     if let Some(parent_entry) = &mount.parent_entry {
        //         if parent_entry.node().id() == node_id {
        //             let parent_fs_ptr = parent_entry.node().filesystem().and_then(|w| w.upgrade())
        //                 .map(|fs| Arc::as_ptr(&fs) as *const ());
        //             if parent_fs_ptr == Some(fs_ptr_to_check) {
        //                 return true;
        //             }
        //         }
        //     }
        // 

        let children = mount_point_to_check.children.read();
        children.contains_key(&entry_to_check.node().id())      
    }

    /// Check if an entry is a source for a bind mount
    pub fn is_bind_source(&self, entry_to_check: &VfsEntryRef) -> bool {
        let node_to_check = entry_to_check.node();
        let node_id = node_to_check.id();
        
        let fs_ptr_to_check = match node_to_check.filesystem().and_then(|w| w.upgrade()) {
            Some(fs) => Arc::as_ptr(&fs) as *const (),
            None => return false,
        };

        false
    }

    /// Check if an entry is used in a mount (either as a mount point or a bind source)
    pub fn is_entry_used_in_mount(&self, entry_to_check: &VfsEntryRef, mount_point_to_check: &Arc<MountPoint>) -> bool {
        // self.is_mount_point(entry_to_check, mount_point_to_check) || self.is_bind_source(entry_to_check)
        self.is_mount_point(entry_to_check, mount_point_to_check)
    }

    /// Unmount a filesystem
    pub fn unmount(&self, entry: &VfsEntryRef, parent_mount_point: &Arc<MountPoint>) -> VfsResult<Arc<MountPoint>> {
        let removed_mount = parent_mount_point.remove_child(&entry);
        match removed_mount {
            Some(mount) => Ok(mount),
            None => Err(vfs_error(FileSystemErrorKind::NotFound, "Mount point not found for unmount")),
        }
    }

    /// Resolve a path to a VFS entry, handling mount boundaries
    pub fn resolve_path(&self, path: &str) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        self.resolve_path_internal(path, false, &PathResolutionOptions { no_follow: false })
    }

    /// Resolve a path to the mount point entry (not the mounted content)
    /// This is used for unmount operations where we need the actual mount point
    pub fn resolve_mount_point(&self, path: &str) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        self.resolve_path_internal(path, true, &PathResolutionOptions { no_follow: false })
    }

    /// Resolve a path with specified options
    pub fn resolve_path_with_options(&self, path: &str, options: &PathResolutionOptions) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        self.resolve_path_internal(path, false, options)
    }

    /// Resolve a path to the mount point entry with options
    pub fn resolve_mount_point_with_options(&self, path: &str, options: &PathResolutionOptions) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        self.resolve_path_internal(path, true, options)
    }

    /// Resolve a relative path from a given base VfsEntry
    /// 
    /// This method resolves a relative path starting from the specified base entry
    /// instead of the root. This is essential for implementing *at system calls
    /// like openat, fstatat, etc.
    /// 
    /// # Arguments
    /// 
    /// * `base_entry` - The base VfsEntry to resolve the path from
    /// * `base_mount` - The mount point containing the base entry
    /// * `relative_path` - The relative path to resolve
    /// * `options` - Path resolution options
    /// 
    /// # Returns
    /// 
    /// * `VfsResult<(VfsEntryRef, Arc<MountPoint>)>` - The resolved entry and its mount point
    pub fn resolve_relative_path(&self, base_entry: &VfsEntryRef, base_mount: &Arc<MountPoint>, relative_path: &str, options: &PathResolutionOptions) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        // If path is absolute, ignore base_entry and use normal resolution
        if relative_path.starts_with('/') {
            return self.resolve_path_with_options(relative_path, options);
        }
        
        // If path is empty or ".", return the base_entry itself
        if relative_path.is_empty() || relative_path == "." {
            return Ok((base_entry.clone(), base_mount.clone()));
        }
        
        // For relative paths, we need to resolve from the base_entry
        let components = self.parse_path(relative_path);
        let mut current_entry = base_entry.clone();
        let current_mount = base_mount.clone();
        
        for component in components.iter() {
            if component == ".." {
                // Handle parent directory traversal - simplified for now
                return Err(crate::fs::FileSystemError::new(
                    crate::fs::FileSystemErrorKind::NotSupported,
                    "Parent directory traversal not implemented yet"
                ));
            } else {
                // Handle regular component lookup using filesystem.lookup
                let node = current_entry.node();
                let filesystem = node.filesystem()
                    .and_then(|w| w.upgrade())
                    .ok_or_else(|| crate::fs::FileSystemError::new(
                        crate::fs::FileSystemErrorKind::NotSupported,
                        "No filesystem reference"
                    ))?;
                
                // Use filesystem.lookup to find child
                let child_node = filesystem.lookup(&node, &component.to_string())?;
                
                // Create or get cached VfsEntry for the child
                if let Some(cached_entry) = current_entry.get_child(component) {
                    current_entry = cached_entry;
                } else {
                    // Create new VfsEntry
                    let child_entry = VfsEntry::new(
                        Some(Arc::downgrade(&current_entry)),
                        component.clone(),
                        child_node
                    );
                    current_entry.add_child(component.clone(), child_entry.clone());
                    current_entry = child_entry;
                }
            }
        }
        
        Ok((current_entry, current_mount))
    }

    fn resolve_path_internal(&self, path: &str, resolve_mount: bool, options: &PathResolutionOptions) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        if path.is_empty() || path == "/" {
            return Ok((self.root_mount.read().root.clone(), self.root_mount.read().clone()));
        }

        let components = self.parse_path(path);
        let mut current_mount = self.root_mount.read().clone();
        let mut current_entry = current_mount.root.clone();
        
        let mut resolved_path = String::new();
        for (i, component) in components.iter().enumerate() {
            let is_final_component = i == components.len() - 1;
            
            if component == ".." {
                // Handle parent directory traversal (same as original implementation)
                let is_at_mount_root = current_entry.node().id() == current_mount.root.node().id();
                
                if is_at_mount_root {
                    let parent_info = current_mount.get_parent().zip(current_mount.parent_entry.clone());
                    match parent_info {
                        Some((parent_mount, parent_entry)) => {
                            current_mount = parent_mount;
                            current_entry = self.resolve_component(parent_entry, &"..")?;
                        },
                        None => {
                            // No parent mount - stay at current mount root
                        }
                    }
                } else {
                    current_entry = self.resolve_component(current_entry, &component)?;
                }
            } else {
                // Regular path traversal with symlink handling based on options
                let should_follow_symlinks = if is_final_component {
                    // For the final component, check no_follow option
                    !options.no_follow
                } else {
                    // For intermediate components, always follow symlinks
                    true
                };

                if should_follow_symlinks {
                    // Use normal component resolution (which follows symlinks)
                    current_entry = self.resolve_component(current_entry, &component)?;
                } else {
                    // Don't follow symlinks - use direct filesystem lookup
                    current_entry = self.resolve_component_no_symlink(current_entry, &component)?;
                }

                // Handle mount points (same as original implementation)
                if resolve_mount && is_final_component {
                    if let Some(_child_mount) = current_mount.get_child(&current_entry) {
                        return Ok((current_entry, current_mount));
                    }
                } else {
                    if let Some(child_mount) = current_mount.get_child(&current_entry) {
                        current_mount = child_mount;
                        current_entry = current_mount.root.clone();
                    }
                }
            }

            resolved_path.push('/');
            resolved_path.push_str(&component);
        }

        Ok((current_entry, current_mount))
    }

    /// Resolve a single path component without following symlinks
    fn resolve_component_no_symlink(&self, entry: VfsEntryRef, component: &str) -> VfsResult<VfsEntryRef> {
        // Handle special cases
        if component == "." {
            return Ok(entry);
        }

        // Check cache first (fast path)
        let component_string = component.to_string();
        if let Some(cached_child) = entry.get_child(&component_string) {
            return Ok(cached_child);
        }

        // Cache miss - perform filesystem lookup without symlink resolution
        let parent_node = entry.node();
        debug_assert!(parent_node.filesystem().is_some(), "resolve_component_no_symlink: parent_node.filesystem() is None");
        let filesystem = parent_node.filesystem()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| vfs_error(FileSystemErrorKind::NotSupported, "No filesystem reference"))?;
        
        // Ask filesystem to lookup the component
        let child_node = filesystem.lookup(&parent_node, &component_string)
            .map_err(|e| vfs_error(e.kind, &e.message))?;

        // Don't resolve symlinks - just create VfsEntry as-is
        let child_entry = VfsEntry::new(
            Some(Arc::downgrade(&entry)),
            component_string.clone(),
            child_node,
        );

        // Add to parent's cache
        entry.add_child(component_string, child_entry.clone());

        Ok(child_entry)
    }

    // Helper methods

    /// Parse a path into components
    pub fn parse_path(&self, path: &str) -> Vec<String> {
        path.split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .map(|s| s.to_string())
            .collect()
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
        self.resolve_component_with_depth(entry, component, 0)
    }

    /// Resolve a single path component with symlink depth tracking
    fn resolve_component_with_depth(&self, entry: VfsEntryRef, component: &str, symlink_depth: u32) -> VfsResult<VfsEntryRef> {
        const MAX_SYMLINK_DEPTH: u32 = 32; // Prevent infinite symlink loops
        
        if symlink_depth > MAX_SYMLINK_DEPTH {
            return Err(vfs_error(FileSystemErrorKind::InvalidPath, "Too many symbolic links"));
        }

        // Handle special cases
        if component == "." {
            return Ok(entry);
        }

        // Check cache first (fast path)
        let component_string = component.to_string();
        if let Some(cached_child) = entry.get_child(&component_string) {
            // Check if cached entry is a symlink that needs resolution
            if cached_child.node().is_symlink()? {
                let link_target = cached_child.node().read_link()
                    .map_err(|e| vfs_error(e.kind, &e.message))?;
                return self.resolve_symlink_target_with_depth(&entry, &link_target, symlink_depth + 1);
            }
            return Ok(cached_child);
        }

        // Cache miss - perform filesystem lookup
        let parent_node = entry.node();
        debug_assert!(parent_node.filesystem().is_some(), "resolve_component: parent_node.filesystem() is None");
        let filesystem = parent_node.filesystem()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| vfs_error(FileSystemErrorKind::NotSupported, "No filesystem reference"))?;
        // Ask filesystem to lookup the component
        let child_node = filesystem.lookup(&parent_node, &component_string)
            .map_err(|e| vfs_error(e.kind, &e.message))?;

        // Check if the resolved node is a symbolic link
        if child_node.is_symlink()? {
            // Resolve the symbolic link
            let link_target = child_node.read_link()
                .map_err(|e| vfs_error(e.kind, &e.message))?;
            
            // Recursively resolve the link target
            return self.resolve_symlink_target_with_depth(&entry, &link_target, symlink_depth + 1);
        }

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

    /// Resolve a symbolic link target
    fn resolve_symlink_target(&self, base_entry: &VfsEntryRef, target: &str) -> VfsResult<VfsEntryRef> {
        self.resolve_symlink_target_with_depth(base_entry, target, 0)
    }

    /// Resolve a symbolic link target with depth tracking
    fn resolve_symlink_target_with_depth(&self, base_entry: &VfsEntryRef, target: &str, symlink_depth: u32) -> VfsResult<VfsEntryRef> {
        const MAX_SYMLINK_DEPTH: u32 = 32; // Prevent infinite symlink loops
        
        if symlink_depth > MAX_SYMLINK_DEPTH {
            return Err(vfs_error(FileSystemErrorKind::InvalidPath, "Too many symbolic links"));
        }

        if target.starts_with('/') {
            // Absolute path - resolve from root
            let (resolved_entry, _mount) = self.resolve_path_internal(target, false, &PathResolutionOptions { no_follow: false })?;
            Ok(resolved_entry)
        } else {
            // Relative path - resolve from current directory
            let components = self.parse_path(target);
            let mut current_entry = base_entry.clone();
            
            for component in components {
                current_entry = self.resolve_component_with_depth(current_entry, &component, symlink_depth)?;
            }
            
            Ok(current_entry)
        }
    }

    // /// Resolve cross-VFS path for bind mounts
    // fn resolve_cross_vfs_path(
    //     &self, 
    //     mount_point: &MountPoint, 
    //     relative_path: &str
    // ) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
    //     if let Some((source_vfs, source_path, _cache_timeout)) = mount_point.get_cross_vfs_info() {
    //         let source_vfs = source_vfs.upgrade()
    //             .ok_or_else(|| vfs_error(FileSystemErrorKind::NotFound, "Source VFS no longer available"))?;

    //         let full_source_path = if relative_path.is_empty() || relative_path == "/" {
    //             source_path.to_string()
    //         } else {
    //             format!("{}/{}", source_path.trim_end_matches('/'), relative_path.trim_start_matches('/'))
    //         };

    //         // Delegate to source VFS for complete resolution (including child mounts)
    //         source_vfs.resolve_path_cross_vfs(&full_source_path)
    //     } else {
    //         Err(vfs_error(FileSystemErrorKind::NotSupported, "Not a cross-VFS mount"))
    //     }
    // }
}