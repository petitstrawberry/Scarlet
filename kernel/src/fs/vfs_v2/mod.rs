//! VFS Version 2 - New Architecture
//!
//! This module implements the new VFS architecture with clear separation of concerns:
//! - VfsEntry: Path hierarchy "names" and "links" (similar to Linux dentry)
//! - VfsNode: File "entity" interface (similar to Linux inode/BSD vnode)
//! - FileSystemOperations: Driver API for filesystem operations
//!
//! This new design provides:
//! - Better scalability and extensibility
//! - Standard OS compatibility
//! - Improved symbolic link resolution
//! - Cleaner interface for complex filesystems like ext4

pub mod core;
pub mod tmpfs;
pub mod cpiofs;
pub mod overlayfs;
pub mod manager;
pub mod mount_tree;

// VFS v2 test modules
#[cfg(test)]
pub mod tests;
#[cfg(test)]
pub mod advanced_tests;
#[cfg(test)]
pub mod performance_tests;
#[cfg(test)]
pub mod cross_vfs_tests;

#[cfg(test)]
pub mod overlayfs_tests;


pub use core::*;
pub use tmpfs::*;
pub use cpiofs::*;
pub use overlayfs::*;
