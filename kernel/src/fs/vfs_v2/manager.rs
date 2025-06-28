//! VFS Manager v2 - Enhanced Virtual File System Management
//!
//! This module provides the next-generation VFS management system for Scarlet,
//! built on the improved VFS v2 architecture with enhanced mount tree management,
//! VfsEntry-based caching, and better isolation support.

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
    format,
};
use spin::RwLock;

use crate::fs::{
    FileSystemError, FileSystemErrorKind, FileMetadata, FileType, 
    DeviceFileInfo
};
use crate::object::KernelObject;

use super::{
    core::{VfsEntry, FileSystemOperations, DirectoryEntryInternal},
    mount_tree::{MountTree, MountOptionsV2, MountPoint, VfsManagerId, MountId, MountType},
};

// Helper function to create FileSystemError
fn vfs_error(kind: FileSystemErrorKind, message: &str) -> FileSystemError {
    FileSystemError::new(kind, message)
}

/// VFS Manager v2 - Enhanced VFS architecture implementation
/// 
/// This manager provides advanced VFS functionality with proper mount tree
/// management, enhanced caching, and better support for containerization.
pub struct VfsManager {
    /// Unique identifier for this VfsManager instance
    pub id: VfsManagerId,
    
    /// Mount tree for hierarchical mount point management
    mount_tree: MountTree,
    
    /// Registered filesystems by name
    filesystems: RwLock<BTreeMap<String, Arc<dyn FileSystemOperations>>>,
    
    /// Current working directory
    cwd: RwLock<Option<Arc<VfsEntry>>>,
}

impl VfsManager {
    /// Create a new VFS manager instance with a dummy root
    pub fn new() -> Self {
        // Create a dummy root filesystem for initialization
        use super::tmpfs::TmpFS;
        let root_fs: Arc<dyn FileSystemOperations> = TmpFS::new(0); // 0 = unlimited memory
        let root_node = root_fs.root_node();
        let dummy_root_entry = VfsEntry::new(None, "/".to_string(), root_node);
        
        let mount_tree = MountTree::new(dummy_root_entry.clone());
        
        let mut filesystems = BTreeMap::new();
        filesystems.insert("/".to_string(), root_fs);

        Self {
            id: VfsManagerId::new(),
            mount_tree,
            filesystems: RwLock::new(filesystems),
            cwd: RwLock::new(None),
            // cross_vfs_refs: RwLock::new(BTreeMap::new()),
        }
    }
    
    /// Mount a filesystem at the specified path
    pub fn mount(
        &self,
        filesystem: Arc<dyn FileSystemOperations>,
        mount_point_str: &str,
        flags: u32,
    ) -> Result<(), FileSystemError> {
        if mount_point_str == "/" {
            // Special case: replacing the root filesystem
            let new_root_node = filesystem.root_node();
            let new_root_entry = VfsEntry::new(None, "/".to_string(), new_root_node);
            let new_root_mount = MountPoint::new_regular("/".to_string(), new_root_entry);
            self.mount_tree.replace_root(new_root_mount);
            let mut fs_map = self.filesystems.write();
            fs_map.clear();
            fs_map.insert("/".to_string(), filesystem);
            return Ok(());
        }

        // Convert flags to mount options (for future use)
        let _mount_options = MountOptionsV2 {
            readonly: (flags & 0x01) != 0,
            flags,
        };
        
        let (target_entry, target_mount_point) = self.mount_tree.resolve_path(mount_point_str)?;

        // Use MountTreeV2 for mounting
        self.mount_tree.mount(target_entry, target_mount_point, filesystem.clone())?;

        // Register filesystem
        self.filesystems.write().insert(mount_point_str.to_string(), filesystem);
        
        Ok(())
    }
    
    /// Unmount a filesystem from the specified path
    pub fn unmount(&self, mount_point_str: &str) -> Result<(), FileSystemError> {
        // Resolve to the mount point entry (not the mounted content)
        let (entry, mount_point) = self.mount_tree.resolve_mount_point(mount_point_str)?;

        // Check if the resolved entry is actually a mount point.
        if !self.mount_tree.is_mount_point(&entry, &mount_point) {
            return Err(vfs_error(FileSystemErrorKind::InvalidPath, "Path is not a mount point"));
        }

        // Unmount using the MountTree.
        self.mount_tree.unmount(&entry, &mount_point)?;
        
        // Remove from the filesystem registry.
        self.filesystems.write().remove(mount_point_str);
        
        Ok(())
    }

    pub fn bind_mount(
        &self,
        source_path: &str,
        target_path: &str
    ) -> Result<(), FileSystemError> {
        // Resolve the target mount point
        let (target_entry, target_mount_point) = self.mount_tree.resolve_path(target_path)?;
        // Resolve the source entry
        let (source_entry, source_mount_point) = self.mount_tree.resolve_path(source_path)?;
        // Check if source is a valid entry
        if !source_entry.node().is_directory()? {
            return Err(vfs_error(FileSystemErrorKind::NotADirectory, "Source path must be a directory"));
        }
        // Check if target is not already a mount point
        if self.mount_tree.is_mount_point(&target_entry, &target_mount_point) {
            return Err(vfs_error(FileSystemErrorKind::InvalidPath, "Target path is already a mount point"));
        }
        // Check if source is a directory (bind mounts only support directories)
        if !source_entry.node().is_directory()? {
            return Err(vfs_error(FileSystemErrorKind::NotADirectory, "Source path must be a directory"));
        }
        // Create the bind mount entry
        self.bind_mount_entry(
            source_entry,
            source_mount_point,
            target_entry,
            target_mount_point
        )
    }

    /// Create a bind mount from source_entry to target_entry
    pub fn bind_mount_entry(
        &self,
        source_entry: Arc<VfsEntry>,
        source_mount_point: Arc<MountPoint>,
        target_entry: Arc<VfsEntry>,
        target_mount_point: Arc<MountPoint>,
    ) -> Result<(), FileSystemError> {
        // Create a new MountPoint for the bind mount
        let bind_mount = MountPoint::new_bind(target_entry.name().clone(), source_entry);
        // Set parent/parent_entry
        unsafe {
            let mut_ptr = Arc::as_ptr(&bind_mount) as *mut MountPoint;
            (*mut_ptr).parent = Some(Arc::downgrade(&target_mount_point));
            (*mut_ptr).parent_entry = Some(target_entry.clone());
        }
        // Connect the bind mount to the source mount point
        *(bind_mount.children.write()) = source_mount_point.children.read().clone();
        // Add as child to target_mount_point
        target_mount_point.children.write().insert(target_entry.node().id(), bind_mount);
        Ok(())
    }
    
    /// Open a file at the specified path
    pub fn open(&self, path: &str, flags: u32) -> Result<KernelObject, FileSystemError> {
        // Use MountTreeV2 to resolve filesystem and relative path, then open
        let entry = self.mount_tree.resolve_path(path)?.0;
        let node = entry.node();
        let filesystem = node.filesystem()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "No filesystem reference"))?;
        let file_obj = filesystem.open(node, flags)?;
        Ok(KernelObject::File(file_obj))
    }
    
    /// Create a file at the specified path
    pub fn create_file(&self, path: &str, file_type: FileType) -> Result<(), FileSystemError> {
        // Split path into parent and filename
        let (parent_path, filename) = self.split_parent_child(path)?;
        
        // Resolve parent directory using MountTreeV2
        let parent_entry = self.mount_tree.resolve_path(&parent_path)?.0;
        let parent_node = parent_entry.node();
        debug_assert!(parent_node.filesystem().is_some(), "VfsManager::create_file - parent_node.filesystem() is None for path '{}'", parent_path);
        // crate::println!("Creating file '{}' in parent '{}'", filename, parent_path);
        
        // Create file using filesystem
        let filesystem = parent_node.filesystem()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "No filesystem reference"))?;
        let new_node = filesystem.create(
            parent_node,
            &filename,
            file_type,
            0o644, // Default permissions
        )?;
        
        // Create VfsEntry and add to parent cache
        let new_entry = VfsEntry::new(
            Some(Arc::downgrade(&parent_entry)),
            filename.clone(),
            new_node,
        );
        
        
        parent_entry.add_child(filename, new_entry);
    
        
        Ok(())
    }
    
    /// Create a directory at the specified path
    pub fn create_dir(&self, path: &str) -> Result<(), FileSystemError> {
        self.create_file(path, FileType::Directory)
    }
    
    /// Remove a file at the specified path
    pub fn remove(&self, path: &str) -> Result<(), FileSystemError> {
        // Resolve the entry to be removed
        let (entry_to_remove, mount_point) = self.mount_tree.resolve_path(path)?;

        // Check if the entry is involved in any mount, which would make it busy
        if self.mount_tree.is_entry_used_in_mount(&entry_to_remove, &mount_point) {
            return Err(vfs_error(FileSystemErrorKind::NotSupported, "Resource is busy"));
        }

        // Split path into parent and filename
        let (parent_path, filename) = self.split_parent_child(path)?;
        
        // Resolve parent directory using MountTreeV2
        let parent_entry = self.mount_tree.resolve_path(&parent_path)?.0;
        let parent_node = parent_entry.node();
        
        // Remove from filesystem
        let filesystem = parent_node.filesystem()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "No filesystem reference"))?;
        filesystem.remove(parent_node, &filename)?;
        
        // Remove from parent cache
        let _ = parent_entry.remove_child(&filename);

        Ok(())
    }
    
    /// Get metadata for a file at the specified path
    pub fn metadata(&self, path: &str) -> Result<FileMetadata, FileSystemError> {
        // Resolve path to VfsEntry
        let entry = self.mount_tree.resolve_path(path)?.0;
        
        // Get VfsNode and return metadata
        let node = entry.node();
        
        node.metadata()
    }
    
    /// Read directory entries at the specified path
    pub fn readdir(&self, path: &str) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        // Resolve path to VfsEntry
        let entry = self.mount_tree.resolve_path(path)?.0;
        
        // Get VfsNode
        let node = entry.node();
        
        // Check if it's a directory
        if !node.is_directory()? {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Not a directory"
            ));
        }
        
        // Get filesystem from node
        let fs_ref = node.filesystem()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Node has no filesystem reference"
            ))?;
            
        let filesystem = fs_ref.upgrade()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Filesystem reference is dead"
            ))?;
        
        // Call filesystem's readdir
        filesystem.readdir(node)
    }
    
    /// Set current working directory
    pub fn set_cwd(&self, path: &str) -> Result<(), FileSystemError> {
        let entry = self.mount_tree.resolve_path(path)?.0;
        
        // Verify it's a directory
        let node = entry.node();
        
        if !node.is_directory()? {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Not a directory"
            ));
        }
        
        *self.cwd.write() = Some(entry);
        Ok(())
    }
    
    /// Get current working directory
    pub fn get_cwd(&self) -> Option<Arc<VfsEntry>> {
        self.cwd.read().clone()
    }
    
    /// Create a device file
    pub fn create_device_file(
        &self,
        path: &str,
        device_info: DeviceFileInfo,
    ) -> Result<(), FileSystemError> {
        let file_type = match device_info.device_type {
            crate::device::DeviceType::Char => FileType::CharDevice(device_info),
            crate::device::DeviceType::Block => FileType::BlockDevice(device_info),
            _ => return Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Unsupported device type"
            )),
        };
        
        self.create_file(path, file_type)
    }
    
    /// Resolve a path in this VFS (public interface for cross-VFS access)
    pub fn resolve_path_cross_vfs(&self, path: &str) -> Result<(Arc<VfsEntry>, Arc<MountPoint>), FileSystemError> {
        self.mount_tree.resolve_path(path)
    }
    
    /// Resolve a path to a VfsEntry (public interface)
    pub fn resolve_path(&self, path: &str) -> Result<Arc<VfsEntry>, FileSystemError> {
        let (entry, _) = self.mount_tree.resolve_path(path)?;
        Ok(entry)
    }

    /// Get the unique ID of this VfsManager
    pub fn id(&self) -> VfsManagerId {
        self.id
    }

    // /// Get the number of cross-VFS references
    // pub fn get_cross_vfs_ref_count(&self) -> usize {
    //     self.cross_vfs_refs.read().len()
    // }

    // /// Register cross-VFS reference
    // pub fn register_cross_vfs_ref(&self, other: Arc<VfsManager>) -> Result<(), FileSystemError> {
    //     self.cross_vfs_refs.write().insert(other.id, Arc::downgrade(&other));
    //     Ok(())
    // }

    // /// Create a cross-VFS bind mount
    // pub fn cross_vfs_bind_mount(
    //     &self,
    //     source_vfs_id: VfsManagerId,
    //     source_path: &str,
    //     target_path: &str,
    //     _recursive: bool,
    // ) -> Result<(), FileSystemError> {
    //     // Check if we're trying to bind to ourselves (recursive bind)
    //     if source_vfs_id == self.id {
    //         return Err(vfs_error(FileSystemErrorKind::InvalidPath, "Recursive bind mount is not allowed"));
    //     }

    //     // Get the source VFS reference
    //     let source_vfs = {
    //         let refs = self.cross_vfs_refs.read();
    //         refs.get(&source_vfs_id)
    //             .ok_or_else(|| vfs_error(FileSystemErrorKind::NotFound, "Source VFS not registered"))?
    //             .upgrade()
    //             .ok_or_else(|| vfs_error(FileSystemErrorKind::NotFound, "Source VFS no longer available"))?
    //     };

    //     // Verify the source path exists
    //     let _ = source_vfs.mount_tree.resolve_path(source_path)
    //         .map_err(|_| vfs_error(FileSystemErrorKind::NotFound, "Source path not found"))?;

    //     // Verify the target path exists
    //     let (target_entry, target_mount_point) = self.mount_tree.resolve_path(target_path)?;

    //     // Create cross-VFS bind mount
    //     let cross_vfs_mount = MountPoint::new_cross_vfs_bind(
    //         target_entry.name().clone(),
    //         Arc::downgrade(&source_vfs),
    //         source_path.to_string(),
    //         target_entry.clone(),
    //         5, // Default 5 seconds cache
    //     );

    //     // Add the mount
    //     target_mount_point.add_child(&target_entry, cross_vfs_mount.clone())?;
    //     self.mount_tree.register_mount(cross_vfs_mount);

    //     Ok(())
    // }
    
    // Helper methods
    
    /// Split a path into parent directory and filename
    fn split_parent_child(&self, path: &str) -> Result<(String, String), FileSystemError> {
        // Simple path normalization: remove trailing slash except for root
        let normalized = if path != "/" && path.ends_with('/') {
            path.trim_end_matches('/').to_string()
        } else {
            path.to_string()
        };
        
        if normalized == "/" {
            return Err(FileSystemError::new(
                FileSystemErrorKind::InvalidPath,
                "Cannot split root path"
            ));
        }
        
        if let Some(last_slash) = normalized.rfind('/') {
            let parent = if last_slash == 0 {
                "/".to_string()
            } else {
                normalized[..last_slash].to_string()
            };
            let filename = normalized[last_slash + 1..].to_string();
            Ok((parent, filename))
        } else {
            Err(FileSystemError::new(
                FileSystemErrorKind::InvalidPath,
                "Invalid path format"
            ))
        }
    }
}

