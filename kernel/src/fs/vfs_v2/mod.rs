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
pub mod path_walk;
pub mod tmpfs_v2;
pub mod cpiofs_v2;
pub mod manager_v2;

// Reference design validation (test/benchmark code)
#[cfg(test)]
pub mod reference_design_test;

pub use core::*;
pub use path_walk::*;
pub use tmpfs_v2::*;
