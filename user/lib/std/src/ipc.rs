//! IPC (Inter-Process Communication) Module
//!
//! This module provides user-space interfaces for IPC mechanisms including
//! pipes, shared memory, and event channels.

use crate::handle::Handle;
use crate::handle::RawHandle;
use crate::syscall::{Syscall, syscall2};

/// Shared memory error type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedMemoryError {
    /// System call failed
    SyscallFailed,
}

pub type SharedMemoryResult<T> = core::result::Result<T, SharedMemoryError>;

/// High-level SharedMemory wrapper with automatic resource management
///
/// Owns a kernel shared memory object handle. The handle is automatically
/// closed when the SharedMemory instance is dropped.
#[derive(Debug)]
pub struct SharedMemory {
    handle: Handle,
}

impl SharedMemory {
    /// Create a shared memory region
    pub fn create(size: usize, permissions: usize) -> SharedMemoryResult<Self> {
        let result = syscall2(Syscall::SharedMemoryCreate, size, permissions);
        if result == usize::MAX {
            return Err(SharedMemoryError::SyscallFailed);
        }
        Ok(Self {
            handle: unsafe { Handle::from_raw(result as i32) },
        })
    }

    /// Create a SharedMemory from an existing Handle
    pub fn from_handle(handle: Handle) -> Self {
        Self { handle }
    }

    /// Get the underlying handle (for advanced usage)
    pub fn as_handle(&self) -> &Handle {
        &self.handle
    }

    /// Get the raw handle value
    pub fn as_raw(&self) -> RawHandle {
        self.handle.as_raw()
    }

    /// Get a SharedMemoryObject capability for this shared memory
    pub fn as_object(&self) -> crate::handle::capability::SharedMemoryObject<'_> {
        // as_shared_memory currently cannot fail
        self.handle.as_shared_memory().unwrap()
    }

    /// Convert the SharedMemory into a Handle
    pub fn into_handle(self) -> Handle {
        unsafe {
            let handle_ptr = &self.handle as *const Handle;
            core::mem::forget(self);
            core::ptr::read(handle_ptr)
        }
    }
}

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
