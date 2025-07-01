//! VFS Version 2 - Modern Architecture Implementation
//!
//! This module implements the new VFS architecture with clear separation of concerns
//! and improved performance characteristics. The design is inspired by modern
//! operating systems like Linux and provides:
//!
//! ## Core Components
//!
//! - **VfsEntry**: Path hierarchy "names" and "links" (similar to Linux dentry)
//!   - Provides caching for fast path resolution
//!   - Manages parent-child relationships in the VFS tree
//!   - Thread-safe with weak reference cleanup
//!
//! - **VfsNode**: File "entity" interface (similar to Linux inode/BSD vnode)
//!   - Abstract representation of files, directories, and special files
//!   - Provides metadata access and type information
//!   - Enables clean downcasting for filesystem-specific operations
//!
//! - **FileSystemOperations**: Driver API for filesystem implementations
//!   - Consolidated interface for all filesystem operations
//!   - Clean separation between VFS and filesystem drivers
//!   - Supports modern filesystems with complex features
//!
//! ## Key Benefits
//!
//! - **Better Scalability**: Improved caching and lookup performance
//! - **Enhanced Extensibility**: Clean interfaces for new filesystem types
//! - **Standard OS Compatibility**: Familiar patterns for kernel developers
//! - **Improved Symbolic Link Resolution**: Proper handling of symlink traversal
//! - **Complex Filesystem Support**: Better support for advanced features like overlays
//!
//! ## Migration from VFS v1
//!
//! VFS v2 provides backward compatibility while offering improved APIs.
//! New code should use the v2 interfaces for better performance and maintainability.
pub mod core;
pub mod drivers;
pub mod manager;
pub mod mount_tree;
pub mod syscall;

// VFS v2 test modules
#[cfg(test)]
pub mod tests;



pub use core::*;
