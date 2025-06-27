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
    mount_tree::{MountTree, MountOptionsV2},
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
        // Create a dummy root entry for initialization
        // In practice, this should be replaced with the actual root filesystem
        use super::tmpfs::TmpFS;
        let tmpfs = TmpFS::new(0); // 0 = unlimited memory
        let root_node = tmpfs.root_node();
        let dummy_root = VfsEntry::new(None, "/".to_string(), root_node);
        
        let mount_tree = MountTree::new(dummy_root.clone());
        
        Self {
            mount_tree,
            filesystems: RwLock::new(BTreeMap::new()),
            cwd: RwLock::new(None),
        }
    }
    
    /// Mount a filesystem at the specified path
    pub fn mount(
        &self,
        filesystem: Arc<dyn FileSystemOperations>,
        mount_point: &str,
        flags: u32,
    ) -> Result<(), FileSystemError> {
        // Convert flags to mount options (for future use)
        let _mount_options = MountOptionsV2 {
            readonly: (flags & 0x01) != 0,
            flags,
        };
        
        // Create a root VfsEntry for the filesystem
        // Note: This is a simplified approach - in reality, you'd need to get the actual root from the filesystem
        // Note: Use the root node directly from filesystem (which already has fs reference)
        let root_node = filesystem.root_node();
        // Verify that root_node has filesystem reference
        debug_assert!(root_node.filesystem().is_some(), "VfsManager::mount - root_node.filesystem() is None");
        
        let root_entry = VfsEntry::new(None, "/".to_string(), root_node.clone());
        
        // Verify VfsEntry's node also has filesystem reference  
        debug_assert!(root_entry.node().filesystem().is_some(), "VfsManager::mount - root_entry.node().filesystem() is None");
        debug_assert!(root_entry.node().filesystem().unwrap().upgrade().is_some(), "VfsManager::mount - root_entry.node().filesystem().upgrade() failed");
        
        // Use MountTreeV2 for mounting
        self.mount_tree.mount(mount_point, root_entry)?;

        // Register filesystem
        let fs_name = format!("{}:{}", filesystem.name(), mount_point);
        self.filesystems.write().insert(fs_name, filesystem);
        
        Ok(())
    }
    
    /// Unmount a filesystem from the specified path
    pub fn unmount(&self, mount_point: &str) -> Result<(), FileSystemError> {
        // Find the mount ID for the given path
        let mount_id = match self.mount_tree.resolve_path(mount_point) {
            Ok(res) => {
                self.mount_tree.get_mount_info(res.0)
            },
            Err(_) => {
                return Err(vfs_error(FileSystemErrorKind::InvalidPath, "Mount point not found"));
            }
        }.map_err(|_| {
            vfs_error(FileSystemErrorKind::InvalidPath, "Mount point not found")
        })?;
            
        // Use MountTreeV2 for unmounting
        self.mount_tree.unmount(mount_id)?;
        
        // Remove from filesystem registry
        self.filesystems.write().retain(|k, _| !k.ends_with(&format!(":{}", mount_point)));
        
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

