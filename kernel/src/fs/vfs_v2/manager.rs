//! VFS Manager v2 - Enhanced Virtual File System Management
//!
//! This module provides the next-generation VFS management system for Scarlet,
//! built on the improved VFS v2 architecture with enhanced mount tree management,
//! VfsEntry-based caching, and better isolation support.

use alloc::{
    string::{String, ToString},
    sync::Arc,
    vec,
    vec::Vec,
};
use spin::{RwLock, Once};

use crate::fs::{
    FileSystemError, FileSystemErrorKind, FileMetadata, FileType, 
    DeviceFileInfo
};
use crate::object::KernelObject;

use super::{
    core::{VfsEntry, FileSystemOperations, DirectoryEntryInternal},
    mount_tree::{MountTree, MountOptionsV2, MountPoint, VfsManagerId},
};

/// Filesystem ID type
pub type FSId = u64;

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
    pub mount_tree: MountTree,
    /// Current working directory
    cwd: RwLock<Option<Arc<VfsEntry>>>,
    /// Strong references to all currently mounted filesystems
    mounted_filesystems: RwLock<Vec<Arc<dyn FileSystemOperations>>>,
}

static GLOBAL_VFS_MANAGER: Once<Arc<VfsManager>> = Once::new();

impl VfsManager {
    /// Create a new VFS manager instance with a dummy root
    pub fn new() -> Self {
        // Create a dummy root filesystem for initialization
        use super::drivers::tmpfs::TmpFS;
        let root_fs: Arc<dyn FileSystemOperations> = TmpFS::new(0); // 0 = unlimited memory
        let root_node = root_fs.root_node();
        let dummy_root_entry = VfsEntry::new(None, "/".to_string(), root_node);
        
        let mount_tree = MountTree::new(dummy_root_entry.clone());
        
        Self {
            id: VfsManagerId::new(),
            mount_tree,
            cwd: RwLock::new(None),
            mounted_filesystems: RwLock::new(vec![root_fs.clone()]),
        }
    }

    /// Create a new VFS manager instance with a specified root filesystem
    pub fn new_with_root(root_fs: Arc<dyn FileSystemOperations>) -> Self {
        let root_node = root_fs.root_node();
        let dummy_root_entry = VfsEntry::new(None, "/".to_string(), root_node);
        let mount_tree = MountTree::new(dummy_root_entry.clone());
        Self {
            id: VfsManagerId::new(),
            mount_tree,
            cwd: RwLock::new(None),
            mounted_filesystems: RwLock::new(vec![root_fs.clone()]),
        }
    }
    
    /// Mount a filesystem at the specified path
    /// 
    /// This will mount the given filesystem at the specified mount point.
    /// If the mount point is "/", it will replace the root filesystem.
    /// 
    /// # Arguments
    /// * `filesystem` - The filesystem to mount.
    /// * `mount_point_str` - The path where the filesystem should be mounted.
    /// * `flags` - Flags for the mount operation (e.g., read-only).
    /// 
    /// # Errors
    /// Returns an error if the mount point is invalid, the filesystem cannot be mounted,
    /// or if the mount operation fails.
    /// 
    pub fn mount(
        &self,
        filesystem: Arc<dyn FileSystemOperations>,
        mount_point_str: &str,
        flags: u32,
    ) -> Result<(), FileSystemError> {
        if mount_point_str == "/" {
            // Remove the existing root FS from the list
            let old_root_fs = self.mount_tree.root_mount.read().root.node().filesystem()
                .and_then(|w| w.upgrade());
            if let Some(old_fs) = old_root_fs {
                let old_ptr = Arc::as_ptr(&old_fs) as *const () as usize;
                self.mounted_filesystems.write().retain(|fs| Arc::as_ptr(fs) as *const () as usize != old_ptr);
            }
            // Set the new root
            let new_root_node = filesystem.root_node();
            let new_root_entry = VfsEntry::new(None, "/".to_string(), new_root_node);
            let new_root_mount = MountPoint::new_regular("/".to_string(), new_root_entry);
            self.mount_tree.replace_root(new_root_mount);
            // Push the new FS
            self.mounted_filesystems.write().push(filesystem.clone());
            return Ok(());
        }
        let _mount_options = MountOptionsV2 {
            readonly: (flags & 0x01) != 0,
            flags,
        };
        let (target_entry, target_mount_point) = self.mount_tree.resolve_path(mount_point_str)?;
        self.mount_tree.mount(target_entry, target_mount_point, filesystem.clone())?;
        self.mounted_filesystems.write().push(filesystem);
        Ok(())
    }

    /// Unmount a mount point at the specified path
    /// 
    /// This will remove the mount point from the mount tree and clean up any
    /// associated resources.
    /// 
    /// # Arguments
    /// * `mount_point_str` - The path of the mount point to unmount.
    /// 
    /// # Errors
    /// Returns an error if the mount point is not valid or if the unmount operation fails.
    /// 
    pub fn unmount(&self, mount_point_str: &str) -> Result<(), FileSystemError> {
        let (entry, mount_point) = self.mount_tree.resolve_mount_point(mount_point_str)?;
        if !self.mount_tree.is_mount_point(&entry, &mount_point) {
            return Err(vfs_error(FileSystemErrorKind::InvalidPath, "Path is not a mount point"));
        }
        
        self.mount_tree.unmount(&entry, &mount_point)?;
        // Identify the unmounted fs and remove it from the holding list
        if let Some(fs) = mount_point.root.node().filesystem().unwrap().upgrade() {
            let fs_ptr = Arc::as_ptr(&fs) as *const () as usize;
            self.mounted_filesystems.write().retain(|fs| Arc::as_ptr(fs) as *const () as usize != fs_ptr);
        }
        Ok(())
    }

    /// Bind mount a directory from source_path to target_path
    /// 
    /// This will create a bind mount where the source directory is mounted
    /// at the target path.
    /// 
    /// # Arguments
    /// * `source_path` - The path of the source directory to bind mount.
    /// * `target_path` - The path where the source directory should be mounted.
    /// 
    /// # Errors
    /// Returns an error if the source is not a directory, the target is already a mount point,
    /// or if the source is not a valid directory.
    /// 
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

    /// Bind mount a directory from another VFS instance
    /// 
    /// This will create a bind mount where the source directory from another VFS
    /// is mounted at the target path in this VFS.
    /// 
    /// # Arguments
    /// * `source_vfs` - The source VFS instance containing the directory to bind
    /// * `source_path` - The path of the source directory in the source VFS.
    /// * `target_path` - The path where the source directory should be mounted in this
    /// VFS.
    /// 
    /// # Errors
    /// Returns an error if the source path does not exist, the target is already a mount point,
    /// or if the source is not a valid directory.
    /// 
    pub fn bind_mount_from(
        &self,
        source_vfs: Arc<VfsManager>,
        source_path: &str,
        target_path: &str,
    ) -> Result<(), FileSystemError> {
        // Resolve the source and target paths
        let (source_entry, source_mount_point) = source_vfs.mount_tree.resolve_path(source_path)?;
        let (target_entry, target_mount_point) = self.mount_tree.resolve_path(target_path)?;

        // Create the bind mount entry
        self.bind_mount_entry(
            source_entry,
            source_mount_point,
            target_entry,
            target_mount_point
        )
    }

    /// Create a bind mount from source_entry to target_entry
    fn bind_mount_entry(
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
    /// 
    /// This will resolve the path using the MountTreeV2 and open the file
    /// using the filesystem associated with the resolved VfsEntry.
    /// 
    /// # Arguments
    /// * `path` - The path of the file to open.
    /// * `flags` - Flags for opening the file (e.g., read, write
    /// * `O_CREAT`, etc.).
    /// 
    /// # Errors
    /// Returns an error if the path does not exist, is not a file, or if
    /// the filesystem cannot be resolved.
    /// 
    pub fn open(&self, path: &str, flags: u32) -> Result<KernelObject, FileSystemError> {
        // Use MountTreeV2 to resolve filesystem and relative path, then open
        let entry = self.mount_tree.resolve_path(path)?.0;
        let node = entry.node();
        let filesystem = node.filesystem()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "No filesystem reference"))?;
        let file_obj = filesystem.open(&node, flags)?;
        Ok(KernelObject::File(file_obj))
    }
    
    /// Create a file at the specified path
    /// 
    /// This will create a new file in the filesystem at the given path.
    ///
    /// # Arguments
    /// * `path` - The path where the file should be created.
    /// * `file_type` - The type of file to create (e.g., regular
    /// file, directory, etc.).
    /// 
    /// # Errors
    /// Returns an error if the parent directory does not exist, the filesystem cannot be resolved,
    /// or if the file cannot be created.
    /// 
    pub fn create_file(&self, path: &str, file_type: FileType) -> Result<(), FileSystemError> {
        // Split path into parent and filename
        let (parent_path, filename) = self.split_parent_child(path)?;
        
        // Resolve parent directory using MountTreeV2
        let parent_entry = self.mount_tree.resolve_path(&parent_path)?.0;
        let parent_node = parent_entry.node();
        debug_assert!(parent_node.filesystem().is_some(), "VfsManager::create_file - parent_node.filesystem() is None for path '{}'", parent_path);
        
        // Create file using filesystem
        let filesystem = parent_node.filesystem()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "No filesystem reference"))?;
        let new_node = filesystem.create(
            &parent_node,
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
    /// 
    /// This will create a new directory in the filesystem at the given path.
    /// 
    /// # Arguments
    /// * `path` - The path where the directory should be created.
    /// 
    /// # Errors
    /// Returns an error if the parent directory does not exist, the filesystem cannot be resolved,
    /// or if the directory cannot be created.
    /// 
    pub fn create_dir(&self, path: &str) -> Result<(), FileSystemError> {
        self.create_file(path, FileType::Directory)
    }
    
    /// Remove a file at the specified path
    /// 
    /// This will remove the file from the filesystem and update the mount tree.
    /// 
    /// # Arguments
    /// * `path` - The path of the file to remove.
    /// 
    /// # Errors
    /// Returns an error if the path does not exist, is not a file, or if
    /// the filesystem cannot be resolved.
    /// 
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
        filesystem.remove(&parent_node, &filename)?;
        
        // Remove from parent cache
        let _ = parent_entry.remove_child(&filename);

        Ok(())
    }
    
    /// Get metadata for a file at the specified path
    /// 
    /// This will resolve the path using the MountTreeV2 and return the metadata
    /// for the file represented by the resolved VfsEntry.
    /// 
    /// # Arguments
    /// * `path` - The path of the file to get metadata for.
    /// 
    /// # Errors
    /// Returns an error if the path does not exist, is not a file, or if
    /// the filesystem cannot be resolved.
    /// 
    pub fn metadata(&self, path: &str) -> Result<FileMetadata, FileSystemError> {
        // Resolve path to VfsEntry
        let entry = self.mount_tree.resolve_path(path)?.0;
        
        // Get VfsNode and return metadata
        let node = entry.node();
        
        node.metadata()
    }
    
    /// Read directory entries at the specified path
    /// 
    /// This will resolve the path using the MountTreeV2 and return a list of
    /// directory entries for the directory represented by the resolved VfsEntry.
    /// 
    /// # Arguments
    /// * `path` - The path of the directory to read.
    /// 
    /// # Errors
    /// Returns an error if the path does not exist, is not a directory, or if
    /// the filesystem cannot be resolved.
    /// 
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
        filesystem.readdir(&node)
    }
    
    /// Set current working directory
    /// 
    /// This will change the current working directory to the specified path.
    /// 
    /// # Arguments
    /// * `path` - The path to set as the current working directory.
    /// 
    /// # Errors
    /// Returns an error if the path does not exist, is not a directory, or if
    /// the filesystem cannot be resolved.
    /// 
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
    /// 
    /// This returns the current working directory as an `Arc<VfsEntry>`.
    /// 
    /// If the current working directory is not set, it returns `None`.
    ///
    /// # Returns
    /// An `Option<Arc<VfsEntry>>` containing the current working directory entry,
    /// or `None` if the current working directory is not set.
    /// 
    pub fn get_cwd(&self) -> Option<Arc<VfsEntry>> {
        self.cwd.read().clone()
    }
    
    /// Create a device file
    /// 
    /// This will create a new device file in the filesystem at the given path.
    /// 
    /// # Arguments
    /// * `path` - The path where the device file should be created.
    /// * `device_info` - Information about the device file to create (e.g.,
    /// device type, major/minor numbers, etc.).
    /// 
    /// # Errors
    /// Returns an error if the parent directory does not exist, the filesystem cannot be resolved,
    /// or if the device file cannot be created.
    /// 
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

    pub fn resolve_path(&self, path: &str) -> Result<Arc<VfsEntry>, FileSystemError> {
        // Use MountTreeV2 to resolve the path
        let (entry, _mount_point) = self.mount_tree.resolve_path(path)?;
        Ok(entry)
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

/// Initialize the global VFS manager (Arc) so it can be retrieved later
pub fn init_global_vfs_manager() -> Arc<VfsManager> {
    GLOBAL_VFS_MANAGER.call_once(|| Arc::new(VfsManager::new())).clone()
}

/// Retrieve the global VFS manager (Arc)
pub fn get_global_vfs_manager() -> Arc<VfsManager> {
    GLOBAL_VFS_MANAGER.get().expect("global VFS manager not initialized").clone()
}

