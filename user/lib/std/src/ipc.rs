//! IPC (Inter-Process Communication) Module
//!
//! This module provides user-space interfaces for IPC mechanisms including
//! pipes, shared memory, and event channels.

use crate::syscall::{Syscall, syscall2};

/// Handle to a shared memory object
pub type SharedMemoryHandle = u32;

/// Permissions for shared memory
pub mod permissions {
    /// Read permission
    pub const READ: usize = 0x1;
    /// Write permission
    pub const WRITE: usize = 0x2;
    /// Execute permission
    pub const EXECUTE: usize = 0x4;
    /// Read and write permissions
    pub const READ_WRITE: usize = READ | WRITE;
}

/// Create a shared memory region
///
/// Creates a new shared memory object that can be mapped into the address space
/// of multiple processes for efficient zero-copy data sharing.
///
/// # Arguments
///
/// * `size` - Size of the shared memory region in bytes
/// * `permissions` - Access permissions using constants from `permissions` module
///
/// # Returns
///
/// Returns a handle to the shared memory object on success, or `None` on failure.
///
/// # Examples
///
/// ```no_run
/// use scarlet_std::ipc::{shared_memory_create, permissions};
///
/// // Create a 4KB shared memory region with read-write permissions
/// let handle = shared_memory_create(4096, permissions::READ_WRITE).unwrap();
/// ```
pub fn shared_memory_create(size: usize, permissions: usize) -> Option<SharedMemoryHandle> {
    let result = syscall2(Syscall::SharedMemoryCreate, size, permissions);
    if result == usize::MAX {
        None
    } else {
        Some(result as u32)
    }
}
