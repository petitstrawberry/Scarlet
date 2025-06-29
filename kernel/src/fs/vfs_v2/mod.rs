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
pub mod drivers;
pub mod manager;
pub mod mount_tree;
pub mod syscall;

// VFS v2 test modules
#[cfg(test)]
pub mod tests;



pub use core::*;
