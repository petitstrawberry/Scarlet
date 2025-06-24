//! VFS Manager v2 - New VFS architecture implementation
//!
//! This module implements the new VFS manager that uses VfsEntry, VfsNode,
//! and the path_walk algorithm for path resolution.

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
    format,
};
use spin::RwLock;

use crate::fs::{
    FileSystemError, FileSystemErrorKind, FileMetadata, FileObject, FileType, 
    DeviceFileInfo, DirectoryEntryInternal
};
use crate::object::KernelObject;

use super::core::{VfsEntry, VfsNode, FileSystemOperations, FileSystemRef, VfsMount};
use super::path_walk::PathWalkContext;

/// VFS Manager v2 - New VFS architecture implementation
pub struct VfsManagerV2 {
    /// Root VfsEntry for this VFS namespace
    root: Arc<RwLock<VfsEntry>>,
    
    /// Path walking context
    path_walker: PathWalkContext,
    
    /// Mounted filesystems
    filesystems: RwLock<BTreeMap<String, Arc<dyn FileSystemOperations>>>,
    
    /// Current working directory cache
    cwd: RwLock<Option<Arc<RwLock<VfsEntry>>>>,
}

impl VfsManagerV2 {
    /// Create a new VFS manager instance
    pub fn new() -> Self {
        // Create a placeholder root node (will be replaced by first mount)
        let root_node = Arc::new(PlaceholderNode::new());
        let root_entry = VfsEntry::new(
            None,
            String::from("/"),
            root_node as Arc<dyn VfsNode>,
        );
        
        let path_walker = PathWalkContext::new(Arc::clone(&root_entry));
        
        Self {
            root: root_entry,
            path_walker,
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
        // Normalize mount point
        let normalized_path = PathWalkContext::normalize_path(mount_point)?;
        
        if normalized_path == "/" {
            // Root mount - replace root entry
            let root_node = filesystem.root_node();
            let new_root = VfsEntry::new(
                None,
                String::from("/"),
                root_node,
            );
            
            // Set mount information
            let mount_info = Arc::new(VfsMount::new(
                Arc::clone(&filesystem),
                flags,
                normalized_path.clone(),
            ));
            new_root.write().set_mount(mount_info);
            
            // Replace root
            let new_root_entry = new_root.read().clone();
            *self.root.write() = new_root_entry;
            
            // Update path walker
            // TODO: Update path walker root reference
        } else {
            // Non-root mount - create mount point
            self.create_mount_point(&normalized_path, filesystem.clone(), flags)?;
        }
        
        // Register filesystem
        let fs_name = format!("{}:{}", filesystem.name(), mount_point);
        self.filesystems.write().insert(fs_name, filesystem);
        
        Ok(())
    }
    
    /// Unmount a filesystem from the specified path
    pub fn unmount(&self, mount_point: &str) -> Result<(), FileSystemError> {
        let normalized_path = PathWalkContext::normalize_path(mount_point)?;
        
        // Find and remove the mount
        // TODO: Implement unmount logic
        
        Ok(())
    }
    
    /// Open a file at the specified path
    pub fn open(&self, path: &str, flags: u32) -> Result<KernelObject, FileSystemError> {
        // Resolve path to VfsEntry
        let entry = self.path_walker.path_walk(path, self.get_cwd())?;
        
        // Get VfsNode from entry
        let node = {
            let entry_guard = entry.read();
            entry_guard.node()
        };
        
        // Get filesystem and open the file
        let filesystem = node.filesystem();
        let file_obj = filesystem.open(node, flags)?;
        
        Ok(KernelObject::File(file_obj))
    }
    
    /// Create a file at the specified path
    pub fn create_file(&self, path: &str, file_type: FileType) -> Result<(), FileSystemError> {
        // Split path into parent and filename
        let (parent_path, filename) = self.split_parent_child(path)?;
        
        // Resolve parent directory
        let parent_entry = self.path_walker.path_walk(&parent_path, self.get_cwd())?;
        let parent_node = {
            let entry_guard = parent_entry.read();
            entry_guard.node()
        };
        
        // Create file using filesystem
        let filesystem = parent_node.filesystem();
        let new_node = filesystem.create(
            parent_node,
            &String::from(&filename),
            file_type,
            0o644, // Default permissions
        )?;
        
        // Create VfsEntry and add to parent cache
        let new_entry = VfsEntry::new(
            Some(Arc::downgrade(&parent_entry)),
            String::from(&filename),
            new_node,
        );
        
        {
            let parent_guard = parent_entry.read();
            parent_guard.add_child(String::from(filename), new_entry);
        }
        
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
        
        // Resolve parent directory
        let parent_entry = self.path_walker.path_walk(&parent_path, self.get_cwd())?;
        let parent_node = {
            let entry_guard = parent_entry.read();
            entry_guard.node()
        };
        
        // Remove from filesystem
        let filesystem = parent_node.filesystem();
        filesystem.remove(parent_node, &String::from(&filename))?;
        
        // Remove from parent cache
        {
            let parent_guard = parent_entry.read();
            parent_guard.remove_child(&String::from(filename));
        }
        
        Ok(())
    }
    
    /// Get metadata for a file at the specified path
    pub fn metadata(&self, path: &str) -> Result<FileMetadata, FileSystemError> {
        // Resolve path to VfsEntry
        let entry = self.path_walker.path_walk(path, self.get_cwd())?;
        
        // Get VfsNode and return metadata
        let node = {
            let entry_guard = entry.read();
            entry_guard.node()
        };
        
        node.metadata()
    }
    
    /// Read directory entries at the specified path
    pub fn readdir(&self, path: &str) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        // Resolve path to VfsEntry
        let entry = self.path_walker.path_walk(path, self.get_cwd())?;
        
        // Get VfsNode
        let node = {
            let entry_guard = entry.read();
            entry_guard.node()
        };
        
        // Check if it's a directory
        if !node.is_directory()? {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Not a directory"
            ));
        }
        
        // TODO: Implement directory reading using new architecture
        // For now, return empty list
        Ok(Vec::new())
    }
    
    /// Set current working directory
    pub fn set_cwd(&self, path: &str) -> Result<(), FileSystemError> {
        let entry = self.path_walker.path_walk(path, self.get_cwd())?;
        
        // Verify it's a directory
        let node = {
            let entry_guard = entry.read();
            entry_guard.node()
        };
        
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
    pub fn get_cwd(&self) -> Option<Arc<RwLock<VfsEntry>>> {
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
        let normalized = PathWalkContext::normalize_path(path)?;
        
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
    
    /// Create a mount point at the specified path
    fn create_mount_point(
        &self,
        mount_point: &str,
        filesystem: Arc<dyn FileSystemOperations>,
        flags: u32,
    ) -> Result<(), FileSystemError> {
        // TODO: Implement mount point creation for non-root mounts
        Ok(())
    }
}

/// Placeholder node for uninitialized root
struct PlaceholderNode;

impl PlaceholderNode {
    fn new() -> Self {
        Self
    }
}

impl VfsNode for PlaceholderNode {
    fn filesystem(&self) -> FileSystemRef {
        // This should never be called
        panic!("PlaceholderNode::filesystem() called - VFS not properly initialized")
    }
    
    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "PlaceholderNode has no metadata"
        ))
    }
    
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}
