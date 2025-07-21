//! Core VFS v2 types and traits
//!
//! This module defines the fundamental types and traits for the new VFS architecture:
//! - VfsEntry: Represents path hierarchy nodes with caching
//! - VfsNode: Abstract interface for file entities
//! - FileSystemOperations: Driver API for filesystem operations

use alloc::{
    collections::BTreeMap, string::{String, ToString}, sync::{Arc, Weak}, vec::Vec
};
use spin::RwLock;
use core::{any::Any, fmt::Debug};
use core::fmt;

use crate::fs::{FileSystemError, FileSystemErrorKind, FileMetadata, FileObject, FileType, SeekFrom};
use crate::object::capability::{StreamOps, ControlOps, StreamError};
use super::mount_tree::MountPoint;

/// DirectoryEntry structure used by readdir
#[derive(Debug, Clone)]
pub struct DirectoryEntryInternal {
    pub name: String,
    pub file_type: FileType,
    pub file_id: u64,
}

/// Reference to a filesystem instance
pub type FileSystemRef = Arc<dyn FileSystemOperations>;

/// VfsEntry represents a node in the VFS path hierarchy (similar to Linux dentry)
/// 
/// This structure represents the VFS's in-memory filesystem hierarchy graph.
/// It serves as:
/// - A "name" representation within a directory
/// - A "link" that constructs parent-child relationships in the VFS graph
/// - A cache for fast re-access to already resolved paths
/// 
/// VfsEntry is designed to be thread-safe and can be shared across threads.
pub struct VfsEntry {
    /// Weak reference to parent VfsEntry (prevents circular references)
    parent: RwLock<Weak<VfsEntry>>,

    /// Name of this VfsEntry (e.g., "user", "file.txt")
    name: String,

    /// Reference to the corresponding file entity (VfsNode)
    node: Arc<dyn VfsNode>,

    /// Cache of child VfsEntries for fast lookup (using Weak to prevent memory leaks)
    children: RwLock<BTreeMap<String, Weak<VfsEntry>>>,
}

impl VfsEntry {
    /// Create a new VfsEntry
    pub fn new(
        parent: Option<Weak<VfsEntry>>,
        name: String,
        node: Arc<dyn VfsNode>,
    ) -> Arc<Self> {
        // Verify that node has filesystem reference when creating VfsEntry
        debug_assert!(node.filesystem().is_some(), "VfsEntry::new - node.filesystem() is None for name '{}'", name);
        debug_assert!(node.filesystem().unwrap().upgrade().is_some(), "VfsEntry::new - node.filesystem().upgrade() failed for name '{}'", name);
        
        Arc::new(Self {
            parent: RwLock::new(parent.unwrap_or_else(|| Weak::new())),
            name,
            node,
            children: RwLock::new(BTreeMap::new()),
        })
    }

    /// Get the name of this entry
    pub fn name(&self) -> &String {
        &self.name
    }

    /// Get the VfsNode for this entry
    pub fn node(&self) -> Arc<dyn VfsNode> {
        Arc::clone(&self.node)
    }

    /// Get parent VfsEntry if it exists
    pub fn parent(&self) -> Option<Arc<VfsEntry>> {
        self.parent.read().upgrade()
    }

    pub fn set_parent(&self, parent: Weak<VfsEntry>) {
        *self.parent.write() = parent;
    }

    /// Add a child to the cache
    pub fn add_child(self: &Arc<Self>, name: String, child: Arc<VfsEntry>) {
        child.set_parent(Arc::downgrade(self));
        let mut children = self.children.write();
        children.insert(name, Arc::downgrade(&child));
    }

    /// Get a child from the cache
    pub fn get_child(&self, name: &String) -> Option<Arc<VfsEntry>> {
        let mut children = self.children.write();
        
        // Try to upgrade the weak reference
        if let Some(weak_ref) = children.get(name) {
            if let Some(strong_ref) = weak_ref.upgrade() {
                return Some(strong_ref);
            } else {
                // Clean up dead weak reference
                children.remove(name);
            }
        }
        
        None
    }

    /// Remove a child from the cache
    pub fn remove_child(&self, name: &String) -> Option<Arc<VfsEntry>> {
        let mut children = self.children.write();
        if let Some(weak_ref) = children.remove(name) {
            weak_ref.upgrade()
        } else {
            None
        }
    }

    /// Clean up expired weak references in the cache
    pub fn cleanup_cache(&self) {
        let mut children = self.children.write();
        children.retain(|_, weak_ref| weak_ref.strong_count() > 0);
    }
}

impl Clone for VfsEntry {
    fn clone(&self) -> Self {
         Self {
            parent: RwLock::new(self.parent.read().clone()),
            name: self.name.clone(),
            node: Arc::clone(&self.node),
            children: RwLock::new(self.children.read().clone()),
        }
    }
}

impl fmt::Debug for VfsEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VfsEntry")
            .field("name", &self.name)
            .field("node", &self.node)
            .field("children_count", &self.children.read().len())
            .finish()
    }
}

/// VfsNode trait represents the "entity" interface for files and directories
///
/// This trait provides only basic APIs for file/directory attributes, type checks, fs reference, and downcasting.
/// All operation APIs (lookup, create, remove, open, etc.) are consolidated in FileSystemOperations for clear separation of concerns.
pub trait VfsNode: Send + Sync + Any {
    /// Returns the unique identifier in the filesystem
    fn id(&self) -> u64;

    /// Returns a (Weak) reference to the filesystem this node belongs to
    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>>;

    /// Get metadata for this node
    fn metadata(&self) -> Result<FileMetadata, FileSystemError>;

    /// Get the file type of this node
    fn file_type(&self) -> Result<FileType, FileSystemError> {
        Ok(self.metadata()?.file_type)
    }

    /// Helper for downcasting
    fn as_any(&self) -> &dyn Any;

    /// Returns true if this node is a directory
    fn is_directory(&self) -> Result<bool, FileSystemError> {
        Ok(self.file_type()? == FileType::Directory)
    }

    /// Returns true if this node is a symbolic link
    fn is_symlink(&self) -> Result<bool, FileSystemError> {
        Ok(matches!(self.file_type()?, FileType::SymbolicLink(_)))
    }

    /// Read the target of a symbolic link (returns error if not a symlink)
    fn read_link(&self) -> Result<String, FileSystemError> {
        Err(FileSystemError::new(
            crate::fs::FileSystemErrorKind::NotSupported,
            "Not a symbolic link"
        ))
    }
}

// Impl debug for VfsNode
impl fmt::Debug for dyn VfsNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VfsNode")
            .field("id", &self.id())
            .field("file_type", &self.file_type().unwrap_or(FileType::Unknown))
            .field("metadata", &self.metadata())
            .field("filesystem", &self.filesystem().and_then(|fs| fs.upgrade().map(|fs| fs.name().to_string())))
            .finish()
    }
}

/// FileSystemOperations trait defines the driver API for filesystem operations
/// 
/// This trait consolidates filesystem operations that were previously scattered
/// across different interfaces. It provides a clean contract between VFS and
/// filesystem drivers.
pub trait FileSystemOperations: Send + Sync {
    /// Look up a child node by name within a parent directory
    /// 
    /// This is the heart of the new driver API. It takes a parent directory's
    /// VfsNode and a name, returning the child's VfsNode.
    fn lookup(
        &self,
        parent_node: &Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError>;

    /// Open a file represented by a VfsNode
    /// 
    /// This method takes a VfsNode (file entity) and opens it, returning
    /// a stateful FileObject for read/write operations.
    fn open(
        &self,
        node: &Arc<dyn VfsNode>,
        flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError>;

    /// Create a new file in the specified directory
    fn create(
        &self,
        parent_node: &Arc<dyn VfsNode>,
        name: &String,
        file_type: FileType,
        mode: u32,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError>;

    /// Remove a file from the specified directory
    fn remove(
        &self,
        parent_node: &Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<(), FileSystemError>;

    /// Read directory entries from a directory node
    fn readdir(
        &self,
        node: &Arc<dyn VfsNode>,
    ) -> Result<Vec<DirectoryEntryInternal>, FileSystemError>;

    /// Get the root VfsNode for this filesystem
    fn root_node(&self) -> Arc<dyn VfsNode>;

    /// Get filesystem name
    fn name(&self) -> &str;

    /// Check if filesystem is read-only
    fn is_read_only(&self) -> bool {
        false
    }

    /// Create a hard link to an existing file
    /// 
    /// This method creates a hard link from `link_name` in `link_parent` to the existing
    /// file represented by `target_node`. Both the link and target will refer to the
    /// same underlying file data.
    /// 
    /// # Arguments
    /// * `link_parent` - Parent directory where the link will be created
    /// * `link_name` - Name for the new hard link
    /// * `target_node` - Existing file to link to
    /// 
    /// # Returns
    /// Returns the VfsNode representing the new hard link on success
    /// 
    /// # Errors
    /// * `NotSupported` - Filesystem doesn't support hard links
    /// * `InvalidOperation` - Target is a directory (most filesystems don't support directory hard links)
    /// * `CrossDevice` - Target and link are on different filesystems
    /// * `FileExists` - Link name already exists in parent directory
    fn create_hardlink(
        &self,
        link_parent: &Arc<dyn VfsNode>,
        link_name: &String,
        target_node: &Arc<dyn VfsNode>,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        // Default implementation: not supported
        let _ = (link_parent, link_name, target_node);
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "Hard links not supported by this filesystem"
        ))
    }

}

impl fmt::Debug for dyn FileSystemOperations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileSystemOperations")
            .field("name", &self.name())
            .field("root", &self.root_node())
            .finish()
    }
}

/// VfsFileObject wraps a filesystem-specific FileObject with VFS-layer information
///
/// This wrapper provides the VFS layer with access to path hierarchy information
/// while delegating actual file operations to the underlying FileSystem implementation.
pub struct VfsFileObject {
    /// The underlying FileObject from the filesystem implementation
    inner: Arc<dyn FileObject>,
    /// The VfsEntry this FileObject was created from (for *at syscalls)
    vfs_entry: Arc<VfsEntry>,
    /// The mount point containing this VfsEntry
    mount_point: Arc<MountPoint>,
    /// The original path used to open this file (for debugging/logging)
    original_path: String,
}

impl VfsFileObject {
    /// Create a new VfsFileObject
    pub fn new(
        inner: Arc<dyn FileObject>,
        vfs_entry: Arc<VfsEntry>,
        mount_point: Arc<MountPoint>,
        original_path: String,
    ) -> Self {
        Self {
            inner,
            vfs_entry,
            mount_point,
            original_path,
        }
    }
    
    /// Get the VfsEntry this FileObject was created from
    pub fn get_vfs_entry(&self) -> &Arc<VfsEntry> {
        &self.vfs_entry
    }
    
    /// Get the mount point containing this VfsEntry
    pub fn get_mount_point(&self) -> &Arc<MountPoint> {
        &self.mount_point
    }
    
    /// Get the original path used to open this file
    pub fn get_original_path(&self) -> &str {
        &self.original_path
    }
    
    /// Enable downcasting for VfsFileObject detection
    pub fn as_any(&self) -> &dyn Any {
        self
    }
}

impl StreamOps for VfsFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        self.inner.read(buffer)
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        self.inner.write(buffer)
    }
}

impl ControlOps for VfsFileObject {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        self.inner.control(command, arg)
    }
}

impl FileObject for VfsFileObject {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        self.inner.seek(whence)
    }
    
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        self.inner.metadata()
    }
    
    fn truncate(&self, size: u64) -> Result<(), StreamError> {
        self.inner.truncate(size)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}