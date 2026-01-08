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
    /// Invalid handle type
    InvalidHandle,
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
        let handle = unsafe { Handle::from_raw(result as i32) }
            .map_err(|_| SharedMemoryError::SyscallFailed)?;
        Ok(Self { handle })
    }

    /// Create a `SharedMemory` from an existing [`Handle`].
    ///
    /// This performs a type check using the handle's cached kernel object info.
    /// If the handle does not represent a shared memory object, this returns
    /// [`SharedMemoryError::InvalidHandle`] and does **not** consume the handle.
    pub fn from_handle(handle: Handle) -> SharedMemoryResult<Self> {
        handle
            .as_shared_memory()
            .map_err(|_| SharedMemoryError::InvalidHandle)?;
        Ok(Self { handle })
    }

    /// Get the underlying handle (for advanced usage)
    pub fn as_handle(&self) -> &Handle {
        &self.handle
    }

    /// Get the raw handle value
    pub fn as_raw(&self) -> RawHandle {
        self.handle.as_raw()
    }

    /// Get a `SharedMemoryObject` capability for this shared memory.
    ///
    /// This is fallible to avoid panicking when a `SharedMemory` wrapper was
    /// constructed from an unexpected handle type.
    pub fn as_object(
        &self,
    ) -> core::result::Result<crate::handle::capability::SharedMemoryObject<'_>, SharedMemoryError>
    {
        self.handle
            .as_shared_memory()
            .map_err(|_| SharedMemoryError::InvalidHandle)
    }

    /// Convert the SharedMemory into a Handle
    pub fn into_handle(self) -> Handle {
        self.handle
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
