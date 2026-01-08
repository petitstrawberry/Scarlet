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

pub struct SharedMemoryObject<'a> {
    handle: &'a Handle,
}

impl<'a> SharedMemoryObject<'a> {
    /// Create a SharedMemoryObject capability from a Handle reference.
    ///
    /// This capability does not own the handle; dropping it will not close anything.
    pub fn from_handle(handle: &'a Handle) -> Self {
        Self { handle }
    }

    /// Get the raw handle value
    pub fn as_raw(&self) -> RawHandle {
        self.handle.as_raw()
    }
}
