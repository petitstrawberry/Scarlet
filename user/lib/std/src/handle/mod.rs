//! Handle Management for Scarlet Native API
//!
//! This module provides the core Handle type and operations for managing
//! KernelObject handles in a type-safe manner.

pub mod capability;

use crate::syscall::{syscall1, syscall2, syscall3, Syscall};
use crate::ffi::str_to_cstr_bytes;
use capability::{StreamOps, FileObject};

/// Result type for handle operations
pub type HandleResult<T> = Result<T, HandleError>;

/// Errors that can occur during handle operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleError {
    /// Invalid handle value
    InvalidHandle,
    /// Operation not supported by this KernelObject type
    Unsupported,
    /// Permission denied
    PermissionDenied,
    /// Out of memory or resources
    OutOfResources,
    /// File or resource not found
    NotFound,
    /// Invalid path or parameters
    InvalidParameter,
    /// Other system error
    SystemError(i32),
}

impl HandleError {
    pub fn from_syscall_result(result: usize) -> Result<i32, Self> {
        if result == usize::MAX {
            Err(HandleError::SystemError(-1))
        } else {
            Ok(result as i32)
        }
    }
}

/// A typed handle to a KernelObject
/// 
/// Handles represent ownership of a KernelObject and provide type-safe
/// access to the object's capabilities. Handles are not cloneable to
/// ensure clear ownership semantics.
#[derive(Debug)]
pub struct Handle {
    raw: i32,
}

impl Handle {
    /// Open a file or resource and return a Handle
    /// 
    /// # Arguments
    /// * `path` - Path to the resource
    /// * `flags` - Open flags (implementation-specific)
    /// 
    /// # Returns
    /// Handle to the opened resource, or HandleError on failure
    pub fn open(path: &str, flags: usize) -> HandleResult<Self> {
        let path_bytes = match str_to_cstr_bytes(path) {
            Ok(bytes) => bytes,
            Err(_) => return Err(HandleError::InvalidParameter),
        };
        
        let result = syscall3(
            Syscall::VfsOpen,
            path_bytes.as_ptr() as usize,
            flags,
            0, // mode (unused for now)
        );
        
        HandleError::from_syscall_result(result).map(|raw| Handle { raw })
    }

    /// Create a Handle from a raw handle value
    /// 
    /// # Safety
    /// The caller must ensure that the raw handle is valid
    pub unsafe fn from_raw(raw: i32) -> Self {
        Self { raw }
    }

    /// Get the raw handle value
    pub fn as_raw(&self) -> i32 {
        self.raw
    }

    /// Close the handle and release the underlying KernelObject
    /// 
    /// After calling this method, the Handle becomes invalid
    pub fn close(self) -> HandleResult<()> {
        let result = syscall1(Syscall::HandleClose, self.raw as usize);
        HandleError::from_syscall_result(result).map(|_| ())
    }

    /// Duplicate this handle
    /// 
    /// Creates a new Handle pointing to the same KernelObject
    pub fn duplicate(&self) -> HandleResult<Handle> {
        let result = syscall1(Syscall::HandleDuplicate, self.raw as usize);
        HandleError::from_syscall_result(result).map(|raw| Handle { raw })
    }

    /// Query the capabilities supported by this handle
    /// 
    /// # Returns
    /// A bitmask of supported capabilities
    pub fn query_capabilities(&self) -> HandleResult<u64> {
        let result = syscall1(Syscall::HandleQuery, self.raw as usize);
        HandleError::from_syscall_result(result).map(|caps| caps as u64)
    }

    /// Set role metadata for this handle
    /// 
    /// # Arguments
    /// * `role` - New role for the handle
    /// 
    /// # Returns
    /// Success or HandleError on failure
    pub fn set_role(&self, role: u32) -> HandleResult<()> {
        let result = syscall2(
            Syscall::HandleSetRole,
            self.raw as usize,
            role as usize,
        );
        HandleError::from_syscall_result(result).map(|_| ())
    }

    /// Get a StreamOps capability for this handle
    /// 
    /// # Returns
    /// StreamOps capability if the handle supports stream operations
    pub fn as_stream(&self) -> HandleResult<StreamOps> {
        // For now, assume all handles support stream operations
        // In the future, we might want to check capabilities
        Ok(StreamOps::from_handle(self.raw))
    }

    /// Get a FileObject capability for this handle
    /// 
    /// # Returns
    /// FileObject capability if the handle supports file operations
    pub fn as_file(&self) -> HandleResult<FileObject> {
        // For now, assume all handles support file operations
        // In the future, we might want to check capabilities
        Ok(FileObject::from_handle(self.raw))
    }

    /// Perform a control operation on this handle (ioctl-equivalent)
    /// 
    /// # Arguments
    /// * `command` - Control command
    /// * `arg` - Argument for the control command
    /// 
    /// # Returns
    /// Result of the control operation
    pub fn control(&self, command: u32, arg: usize) -> HandleResult<i32> {
        let result = syscall3(
            Syscall::HandleControl,
            self.raw as usize,
            command as usize,
            arg,
        );
        HandleError::from_syscall_result(result)
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // Automatically close the handle when it goes out of scope
        // Ignore errors during drop
        let _ = syscall1(Syscall::HandleClose, self.raw as usize);
    }
}