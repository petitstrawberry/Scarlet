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
    mount_tree::{MountTree, MountOptionsV2, MountPoint, VfsManagerId, VfsResult, VfsEntryRef},
};

/// Filesystem ID type
pub type FSId = u64;

/// Path resolution options for VFS operations
#[derive(Debug, Clone)]
pub struct PathResolutionOptions {
    /// Don't follow symbolic links in the final component (like lstat behavior)
    pub no_follow: bool,
}

impl PathResolutionOptions {
    /// Create options with no_follow flag set (don't follow final symlink)
    pub fn no_follow() -> Self {
        Self {
            no_follow: true,
        }
    }
}

impl Default for PathResolutionOptions {
    fn default() -> Self {
        Self {
            no_follow: false,
        }
    }
}

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
    /// Current working directory: (VfsEntry, MountPoint) pair
    pub cwd: RwLock<Option<(Arc<VfsEntry>, Arc<MountPoint>)>>,
    /// Strong references to all currently mounted filesystems
    pub mounted_filesystems: RwLock<Vec<Arc<dyn FileSystemOperations>>>,
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
        let (target_entry, target_mount_point) = self.resolve_path(mount_point_str)?;
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
        let (entry, mount_point) = self.resolve_mount_point(mount_point_str)?;
        if !self.mount_tree.is_mount_point(&entry, &mount_point) {
            return Err(vfs_error(FileSystemErrorKind::InvalidPath, "Path is not a mount point"));
        }
        let unmounted_mount = self.mount_tree.unmount(&entry, &mount_point)?;
        // Identify the unmounted fs and remove it from the holding list
        // If mount_point is a bind mount, we do not remove the filesystem
        if !unmounted_mount.is_bind_mount() {
            if let Some(fs) = unmounted_mount.root.node().filesystem().unwrap().upgrade() {
                let fs_ptr = Arc::as_ptr(&fs) as *const () as usize;
                self.mounted_filesystems.write().retain(|fs| Arc::as_ptr(fs) as *const () as usize != fs_ptr);
            }
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
        let (target_entry, target_mount_point) = self.resolve_path(target_path)?;
        // Resolve the source entry
        let (source_entry, source_mount_point) = self.resolve_path(source_path)?;
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
        source_vfs: &Arc<VfsManager>,
        source_path: &str,
        target_path: &str,
    ) -> Result<(), FileSystemError> {
        // Resolve the source and target paths
        let (source_entry, source_mount_point) = source_vfs.resolve_path(source_path)?;
        let (target_entry, target_mount_point) = self.resolve_path(target_path)?;

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
        let (entry, mount_point) = self.resolve_path(path)?;
        let node = entry.node();
        let filesystem = node.filesystem()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "No filesystem reference"))?;
        
        // Get the underlying FileSystem implementation
        let inner_file_obj = filesystem.open(&node, flags)?;
        
        // Wrap with VFS-layer information
        let vfs_file_obj = super::core::VfsFileObject::new(
            inner_file_obj,
            entry,
            mount_point,
            path.to_string()
        );
        
        Ok(KernelObject::File(Arc::new(vfs_file_obj)))
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
        let parent_entry = self.resolve_path(&parent_path)?.0;
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
    
    /// Create a symbolic link at the specified path
    /// 
    /// This will create a new symbolic link in the filesystem at the given path,
    /// pointing to the specified target path.
    /// 
    /// # Arguments
    /// * `path` - The path where the symbolic link should be created.
    /// * `target_path` - The path that the symbolic link should point to.
    /// 
    /// # Errors
    /// Returns an error if the parent directory does not exist, the filesystem cannot be resolved,
    /// or if the symbolic link cannot be created.
    /// 
    pub fn create_symlink(&self, path: &str, target_path: &str) -> Result<(), FileSystemError> {
        self.create_file(path, FileType::SymbolicLink(target_path.to_string()))
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
        // Resolve the entry to be removed - use no_follow to follow intermediate symlinks
        // but not the final component (like POSIX rm behavior)
        let options = PathResolutionOptions::no_follow();
        let (entry_to_remove, mount_point) = self.resolve_path_with_options(path, &options)?;

        // Check if the entry is involved in any mount, which would make it busy
        if self.mount_tree.is_entry_used_in_mount(&entry_to_remove, &mount_point) {
            return Err(vfs_error(FileSystemErrorKind::NotSupported, "Resource is busy"));
        }

        // Split path into parent and filename
        let (parent_path, filename) = self.split_parent_child(path)?;
        
        // Resolve parent directory using MountTreeV2 (follow all symlinks for parent path)
        let parent_entry = self.resolve_path(&parent_path)?.0;
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
        let entry = self.resolve_path(path)?.0;
        
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
        // Resolve path to VfsEntry and MountPoint
        let (entry, mount_point) = self.resolve_path(path)?;
        
        // Check if this is a bind mount
        let is_bind_mount = matches!(mount_point.mount_type, super::mount_tree::MountType::Bind);
        
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
        let mut entries = filesystem.readdir(&node)?;
        
        // For bind mounts, ensure we only return unique entries
        // This prevents any potential duplication issues
        if is_bind_mount {
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            entries.dedup_by(|a, b| a.name == b.name);
        }
        
        Ok(entries)
    }
    
    /// Set current working directory by path
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
    pub fn set_cwd_by_path(&self, path: &str) -> Result<(), FileSystemError> {
        let (entry, mount_point) = self.resolve_path(path)?;
        
        // Verify it's a directory
        let node = entry.node();
        
        if !node.is_directory()? {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Not a directory"
            ));
        }
        
        self.set_cwd(entry, mount_point);
        Ok(())
    }
    
    /// Set current working directory
    /// 
    /// This sets the current working directory to the specified VfsEntry and MountPoint.
    /// The entry must be a directory.
    /// 
    /// # Arguments
    /// * `entry` - The VfsEntry representing the directory
    /// * `mount_point` - The MountPoint where the directory is mounted
    /// 
    pub fn set_cwd(&self, entry: Arc<VfsEntry>, mount_point: Arc<MountPoint>) {
        *self.cwd.write() = Some((entry, mount_point));
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
    pub fn get_cwd(&self) -> Option<(Arc<VfsEntry>, Arc<MountPoint>)> {
        self.cwd.read().clone()
    }
    
    /// Get current working directory as path string
    /// 
    /// This returns the current working directory as a path string.
    /// If the current working directory is not set, it returns "/".
    ///
    /// # Returns
    /// A `String` containing the current working directory path.
    /// 
    pub fn get_cwd_path(&self) -> String {
        if let Some((entry, mount_point)) = self.get_cwd() {
            self.build_absolute_path(&entry, &mount_point)
        } else {
            "/".to_string()
        }
    }

    /// Build absolute path from VfsEntry and MountPoint
    /// 
    /// This safely constructs the absolute path for a given VfsEntry by using
    /// MountPoint information, avoiding potential issues with Weak references.
    /// 
    /// # Arguments
    /// * `entry` - The VfsEntry to build the path for
    /// * `mount_point` - The MountPoint containing this entry
    /// 
    /// # Returns
    /// A `String` containing the absolute path
    pub fn build_absolute_path(&self, entry: &Arc<VfsEntry>, mount_point: &Arc<MountPoint>) -> String {
        // Build relative path within the mount point
        let mut path_components = Vec::new();
        let mut current = Some(entry.clone());
        let mount_root = &mount_point.root;
        
        // Traverse up to the mount root
        while let Some(entry) = current {
            // Stop if we've reached the mount root
            if Arc::ptr_eq(&entry, mount_root) {
                break;
            }
            
            path_components.push(entry.name().clone());
            current = entry.parent();
        }
        
        // Get the mount path using MountTree's method
        let mount_path = self.mount_tree.get_mount_absolute_path(mount_point);
        
        if path_components.is_empty() {
            // This is the mount root itself
            mount_path
        } else {
            path_components.reverse();
            let relative_path = path_components.join("/");
            
            if mount_path == "/" {
                alloc::format!("/{}", relative_path)
            } else {
                alloc::format!("{}/{}", mount_path, relative_path)
            }
        }
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

    /// Resolve a path to both VfsEntry and MountPoint
    /// 
    /// Automatically handles both absolute paths (starting with '/') and relative paths
    /// (resolved from current working directory). This is the main path resolution API.
    /// 
    /// Returns both VfsEntry and MountPoint for consistency with other resolution APIs.
    pub fn resolve_path(&self, path: &str) -> Result<(Arc<VfsEntry>, Arc<MountPoint>), FileSystemError> {
        self.resolve_path_with_options(path, &PathResolutionOptions::default())
    }

    /// Resolve a path with specified options
    pub fn resolve_path_with_options(&self, path: &str, options: &PathResolutionOptions) -> Result<(Arc<VfsEntry>, Arc<MountPoint>), FileSystemError> {
        // Check if the path is absolute
        if path.starts_with('/') {
            // Absolute path - resolve from root
            self.mount_tree.resolve_path_with_options(path, options)
        } else {
            // Relative path - resolve from current working directory
            let cwd = self.get_cwd();
            if let Some((base_entry, base_mount)) = cwd {
                // Resolve relative to current working directory
                self.resolve_path_from_with_options(&base_entry, &base_mount, path, options)
            } else {
                Err(FileSystemError::new(
                    FileSystemErrorKind::InvalidPath,
                    "Relative path resolution requires a current working directory"
                ))
            }
        }
    }
    
    /// Resolve a path from a specific base directory (for *at system calls)
    /// 
    /// This method resolves a path starting from the specified base directory.
    /// It's specifically designed for *at system calls (openat, fstatat, etc.).
    /// 
    /// Returns both VfsEntry and MountPoint for efficient use.
    pub fn resolve_path_from(
        &self, 
        base_entry: &Arc<VfsEntry>, 
        base_mount: &Arc<MountPoint>, 
        path: &str
    ) -> Result<(Arc<VfsEntry>, Arc<MountPoint>), FileSystemError> {
        self.resolve_path_from_with_options(base_entry, base_mount, path, &PathResolutionOptions::default())
    }
    
    /// Resolve a path from an optional base directory with options
    pub fn resolve_path_from_with_options(
        &self,
        base_entry: &Arc<VfsEntry>,
        base_mount: &Arc<MountPoint>,
        path: &str,
        options: &PathResolutionOptions
    ) -> Result<(Arc<VfsEntry>, Arc<MountPoint>), FileSystemError> {
        if path.starts_with('/') {
            // Absolute path - ignore base and resolve from root
            self.mount_tree.resolve_path_with_options(path, options)
        } else {
            // Relative path with explicit base (for *at syscalls)
            self.mount_tree.resolve_path_from_with_options(Some(base_entry), Some(base_mount), path, options)
        }
    }

    /// Resolve a path to mount point (returns VfsEntryRef instead of Arc<VfsEntry>)
    /// 
    /// This is useful for operations that need to work with mount point information
    /// but don't necessarily need strong references to entries.
    /// 
    /// # Arguments
    /// * `path` - The path to resolve
    /// 
    /// # Returns
    /// Returns a tuple of (VfsEntryRef, Arc<MountPoint>) on success
    pub fn resolve_mount_point(&self, path: &str) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        self.resolve_mount_point_with_options(path, &PathResolutionOptions::default())
    }

    /// Resolve a path to mount point with specified options
    pub fn resolve_mount_point_with_options(&self, path: &str, options: &PathResolutionOptions) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        // Check if the path is absolute
        if path.starts_with('/') {
            // Absolute path - resolve from root
            self.mount_tree.resolve_mount_point_with_options(path, options)
        } else {
            // Relative path - resolve from current working directory
            let cwd = self.get_cwd();
            if let Some((base_entry, base_mount)) = cwd {
                // Resolve relative to current working directory
                self.resolve_mount_point_from_with_options(&base_entry, &base_mount, path, options)
            } else {
                Err(vfs_error(FileSystemErrorKind::InvalidPath, "Relative path resolution requires a current working directory"))
            }
        }
    }

    pub fn resolve_mount_point_from(
        &self,
        base_entry: &Arc<VfsEntry>,
        base_mount: &Arc<MountPoint>,
        path: &str,
        options: &PathResolutionOptions
    ) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        self.resolve_mount_point_from_with_options(base_entry, base_mount, path, options)
    }

    pub fn resolve_mount_point_from_with_options(
        &self,
        base_entry: &Arc<VfsEntry>,
        base_mount: &Arc<MountPoint>,
        path: &str,
        options: &PathResolutionOptions
    ) -> VfsResult<(VfsEntryRef, Arc<MountPoint>)> {
        if path.starts_with('/') {
            // Absolute path - resolve from root
            self.mount_tree.resolve_mount_point_with_options(path, options)
        } else {
            // Relative path with explicit base (for *at syscalls)
            self.mount_tree.resolve_mount_point_from_with_options(Some(base_entry), Some(base_mount), path, options)
        }
    }

    /// Create a hard link
    /// 
    /// This will create a hard link where the source file is linked to the target path.
    /// Both paths will refer to the same underlying file data.
    /// 
    /// # Arguments
    /// * `source_path` - Path of the existing file to link to
    /// * `target_path` - Path where the hard link should be created
    /// 
    /// # Errors
    /// Returns an error if the source doesn't exist, target already exists, 
    /// filesystems don't match, or hard links aren't supported.
    /// 
    pub fn create_hardlink(
        &self,
        source_path: &str,
        target_path: &str,
    ) -> Result<(), FileSystemError> {
        // Resolve source file
        let (source_entry, _source_mount) = self.resolve_path(source_path)?;

        let source_node = source_entry.node();
        
        // Check that source is a regular file (most filesystems don't support directory hard links)
        if source_node.is_directory()? {
            return Err(vfs_error(
                FileSystemErrorKind::InvalidOperation, 
                "Cannot create hard link to directory"
            ));
        }
        
        // Split target path into parent and filename
        let (target_parent_path, target_filename) = self.split_parent_child(target_path)?;
        
        // Resolve target parent directory
        let (target_parent_entry, _target_mount) = self.resolve_path(&target_parent_path)?;
        let target_parent_node = target_parent_entry.node();
        
        // Check that target parent is a directory
        if !target_parent_node.is_directory()? {
            return Err(vfs_error(
                FileSystemErrorKind::NotADirectory,
                "Target parent is not a directory"
            ));
        }
        
        // Get filesystems for both source and target
        let source_fs = source_node.filesystem()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "No filesystem reference for source"))?;
        
        let target_fs = target_parent_node.filesystem()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "No filesystem reference for target"))?;
        
        // Check that both files are on the same filesystem (hard links can't cross filesystem boundaries)
        if !Arc::ptr_eq(&source_fs, &target_fs) {
            return Err(vfs_error(
                FileSystemErrorKind::CrossDevice,
                "Hard links cannot cross filesystem boundaries"
            ));
        }
        
        // Check if target already exists
        if target_parent_entry.get_child(&target_filename).is_some() {
            return Err(vfs_error(
                FileSystemErrorKind::FileExists,
                "Target file already exists"
            ));
        }
        
        // Create the hard link
        let link_node = source_fs.create_hardlink(&target_parent_node, &target_filename, &source_node)?;
        
        // Create VfsEntry and add to parent cache
        let link_entry = VfsEntry::new(
            Some(Arc::downgrade(&target_parent_entry)),
            target_filename.clone(),
            link_node,
        );
        target_parent_entry.add_child(target_filename, link_entry);
        
        Ok(())
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

    /// Open a file relative to a given base entry and mount (for *at syscalls)
    /// 
    /// # Arguments
    /// * `base_entry` - Base VfsEntry to resolve relative path from
    /// * `base_mount` - Base MountPoint for the base entry
    /// * `path` - Relative or absolute path
    /// * `flags` - Open flags
    ///
    /// # Returns
    /// KernelObject::File(VfsFileObject)
    /// Open a file with optional base directory (unified openat implementation)
    /// 
    /// If base_entry and base_mount are None, behaves like regular open().
    /// If base is provided, resolves relative paths from that base (for *at syscalls).
    pub fn open_from(
        &self,
        base_entry: &Arc<VfsEntry>,
        base_mount: &Arc<MountPoint>,
        path: &str,
        flags: u32
    ) -> Result<KernelObject, FileSystemError> {
        let (entry, mount_point) = self.resolve_path_from(base_entry, base_mount, path)?;
        let node = entry.node();
        let filesystem = node.filesystem()
            .and_then(|w| w.upgrade())
            .ok_or_else(|| FileSystemError::new(FileSystemErrorKind::NotSupported, "No filesystem reference"))?;
        let inner_file_obj = filesystem.open(&node, flags)?;
        let vfs_file_obj = super::core::VfsFileObject::new(
            inner_file_obj,
            entry,
            mount_point,
            path.to_string()
        );
        Ok(KernelObject::File(Arc::new(vfs_file_obj)))
    }

    /// Resolve a relative path to an absolute path using the current working directory
    /// 
    /// If the path is already absolute, returns it as-is.
    /// If the path is relative, combines it with the current working directory.
    /// 
    /// # Arguments
    /// * `path` - The path to resolve (relative or absolute)
    /// 
    /// # Returns
    /// An absolute path string
    pub fn resolve_path_to_absolute(&self, path: &str) -> String {
        if path.starts_with('/') {
            // Already absolute path
            path.to_string()
        } else {
            // Relative path - combine with current working directory
            self.get_cwd_path() + "/" + path
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

