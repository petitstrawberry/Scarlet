//! Core VFS v2 types and traits
//!
//! This module defines the fundamental types and traits for the new VFS architecture:
//! - VfsEntry: Represents path hierarchy nodes with caching
//! - VfsNode: Abstract interface for file entities
//! - FileSystemOperations: Driver API for filesystem operations

use alloc::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    string::String,
};
use spin::RwLock;
use core::any::Any;

use crate::fs::{FileSystemError, FileMetadata, FileObject, FileType};

/// Reference to a filesystem instance
pub type FileSystemRef = Arc<dyn FileSystemOperations>;

/// VfsEntry represents a node in the VFS path hierarchy (similar to Linux dentry)
/// 
/// This structure represents the VFS's in-memory filesystem hierarchy graph.
/// It serves as:
/// - A "name" representation within a directory
/// - A "link" that constructs parent-child relationships in the VFS graph
/// - A cache for fast re-access to already resolved paths
pub struct VfsEntry {
    /// Weak reference to parent VfsEntry (prevents circular references)
    parent: Weak<RwLock<VfsEntry>>,

    /// Name of this VfsEntry (e.g., "user", "file.txt")
    name: String,

    /// Reference to the corresponding file entity (VfsNode)
    node: Arc<dyn VfsNode>,

    /// Cache of child VfsEntries for fast lookup (using Weak to prevent memory leaks)
    children: RwLock<BTreeMap<String, Weak<RwLock<VfsEntry>>>>,

    /// Reference to VfsMount if this is a mount point
    mount: Option<Arc<VfsMount>>,
}

impl VfsEntry {
    /// Create a new VfsEntry
    pub fn new(
        parent: Option<Weak<RwLock<VfsEntry>>>,
        name: String,
        node: Arc<dyn VfsNode>,
    ) -> Arc<RwLock<Self>> {
        Arc::new(RwLock::new(Self {
            parent: parent.unwrap_or_else(|| Weak::new()),
            name,
            node,
            children: RwLock::new(BTreeMap::new()),
            mount: None,
        }))
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
    pub fn parent(&self) -> Option<Arc<RwLock<VfsEntry>>> {
        self.parent.upgrade()
    }

    /// Add a child to the cache
    pub fn add_child(&self, name: String, child: Arc<RwLock<VfsEntry>>) {
        let mut children = self.children.write();
        children.insert(name, Arc::downgrade(&child));
    }

    /// Get a child from the cache
    pub fn get_child(&self, name: &String) -> Option<Arc<RwLock<VfsEntry>>> {
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
    pub fn remove_child(&self, name: &String) -> Option<Arc<RwLock<VfsEntry>>> {
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

    /// Set mount point information
    pub fn set_mount(&mut self, mount: Arc<VfsMount>) {
        self.mount = Some(mount);
    }

    /// Get mount point information
    pub fn mount(&self) -> Option<Arc<VfsMount>> {
        self.mount.clone()
    }

    /// Check if this is a mount point
    pub fn is_mount_point(&self) -> bool {
        self.mount.is_some()
    }
}

impl Clone for VfsEntry {
    fn clone(&self) -> Self {
         Self {
            parent: self.parent.clone(),
            name: self.name.clone(),
            node: Arc::clone(&self.node),
            children: RwLock::new(self.children.read().clone()),
            mount: None, // Don't copy mount info
        }
    }
}

/// VfsNode trait represents the "entity" interface for files and directories
/// 
/// This trait abstracts the actual file/directory entity, providing a contract
/// between VFS and individual filesystem drivers. It's similar to Linux inode
/// or BSD vnode.
pub trait VfsNode: Send + Sync {
    /// Get the filesystem this VfsNode belongs to
    fn filesystem(&self) -> FileSystemRef;

    /// Get metadata for this VfsNode
    fn metadata(&self) -> Result<FileMetadata, FileSystemError>;

    /// Get the file type of this node
    fn file_type(&self) -> Result<FileType, FileSystemError> {
        Ok(self.metadata()?.file_type)
    }

    /// Helper method for downcasting
    fn as_any(&self) -> &dyn Any;

    /// Check if this node represents a directory
    fn is_directory(&self) -> Result<bool, FileSystemError> {
        Ok(self.file_type()? == FileType::Directory)
    }

    /// Check if this node represents a symbolic link
    fn is_symlink(&self) -> Result<bool, FileSystemError> {
        Ok(self.file_type()? == FileType::SymbolicLink)
    }

    /// Read symbolic link target (only valid for symlinks)
    fn read_link(&self) -> Result<String, FileSystemError> {
        Err(FileSystemError::new(
            crate::fs::FileSystemErrorKind::NotSupported,
            "Not a symbolic link"
        ))
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
        parent_node: Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError>;

    /// Open a file represented by a VfsNode
    /// 
    /// This method takes a VfsNode (file entity) and opens it, returning
    /// a stateful FileObject for read/write operations.
    fn open(
        &self,
        node: Arc<dyn VfsNode>,
        flags: u32,
    ) -> Result<Arc<dyn FileObject>, FileSystemError>;

    /// Create a new file in the specified directory
    fn create(
        &self,
        parent_node: Arc<dyn VfsNode>,
        name: &String,
        file_type: FileType,
        mode: u32,
    ) -> Result<Arc<dyn VfsNode>, FileSystemError>;

    /// Remove a file from the specified directory
    fn remove(
        &self,
        parent_node: Arc<dyn VfsNode>,
        name: &String,
    ) -> Result<(), FileSystemError>;

    /// Get the root VfsNode for this filesystem
    fn root_node(&self) -> Arc<dyn VfsNode>;

    /// Get filesystem name
    fn name(&self) -> &str;

    /// Check if filesystem is read-only
    fn is_read_only(&self) -> bool {
        false
    }
}

/// Mount information for VFS entries
pub struct VfsMount {
    /// The mounted filesystem
    pub filesystem: FileSystemRef,
    
    /// Mount flags
    pub flags: u32,
    
    /// Mount point path
    pub mount_point: String,
}

impl VfsMount {
    pub fn new(filesystem: FileSystemRef, flags: u32, mount_point: String) -> Self {
        Self {
            filesystem,
            flags,
            mount_point,
        }
    }
}
