//! SharedMemoryObject capability for Scarlet Native API
//!
//! This module provides a capability marker and future extension point for
//! shared memory KernelObjects.

/// Result type for shared memory operations
pub type SharedMemoryObjectResult<T> = Result<T, SharedMemoryObjectError>;

/// Errors that can occur during shared memory operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedMemoryObjectError {
    /// Other system error
    SystemError(i32),
}

/// Shared memory object capability
use crate::handle::{Handle, RawHandle};
use crate::syscall::{syscall2, Syscall};

pub struct SharedMemoryObject<'a> {
    handle: &'a Handle,
}

impl<'a> SharedMemoryObject<'a> {
    /// Create a SharedMemoryObject capability from a Handle reference.
    ///
    /// This is crate-internal to prevent bypassing `Handle::as_shared_memory` validation.
    pub(crate) fn from_handle(handle: &'a Handle) -> Self {
        Self { handle }
    }

    /// Get the raw handle value
    pub fn as_raw(&self) -> RawHandle {
        self.handle.as_raw()
    }

    /// Resize the shared memory region
    pub fn resize(&self, new_size: usize) -> SharedMemoryObjectResult<()> {
        let result = syscall2(
            Syscall::SharedMemoryResize,
            self.handle.as_raw() as usize,
            new_size,
        );
        if result == usize::MAX {
            return Err(SharedMemoryObjectError::SystemError(-1));
        }
        Ok(())
    }
}
