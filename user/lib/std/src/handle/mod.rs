//! Handle Management for Scarlet Native API
//!
//! This module defines the core [`Handle`] type and the rules around ownership and
//! type validation.
//!
//! ## Ownership
//!
//! - [`RawHandle`] is the raw integer handle value.
//! - [`Handle`] is the *only* owning wrapper in user space.
//!   Dropping a `Handle` closes the underlying kernel handle.
//!
//! ## Introspection and Validation
//!
//! When a `Handle` is constructed (e.g. [`Handle::open`] or [`Handle::from_raw`]),
//! user space queries the kernel for [`KernelObjectInfo`] via `Syscall::HandleQuery`.
//! The result is cached inside the `Handle` and used to validate conversions:
//!
//! - `Handle::as_stream` checks `info.capabilities.stream_ops`
//! - `Handle::as_file` checks `info.capabilities.file_ops`
//! - `Handle::as_socket` checks `info.object_type == Socket`
//! - `Handle::as_shared_memory` checks `info.object_type == SharedMemory`
//!
//! Capability types are borrowed views (e.g. `StreamOps<'_>`) and do not own handles.
//! To avoid bypassing validation, capability constructors are crate-internal; prefer
//! using `Handle::as_*`.

pub mod capability;
pub mod introspection;

use crate::ffi::str_to_cstr_bytes;
use crate::syscall::{Syscall, syscall1, syscall2, syscall3};
use capability::{FileObject, MemoryMappingOps, SharedMemoryObject, SocketObject, StreamOps};
use introspection::{KernelObjectInfo, KernelObjectType};

/// Result type for handle operations
pub type HandleResult<T> = Result<T, HandleError>;

/// Raw kernel handle type used throughout userlib.
///
/// This is the canonical representation of a kernel object handle at the
/// userlib boundary. Public APIs may expose other integer widths for
/// compatibility (e.g., `u32`), but internally we normalize to `RawHandle`.
pub type RawHandle = i32;

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
    raw: RawHandle,
    info: KernelObjectInfo,
}

impl Handle {
    fn query_info(raw: RawHandle) -> HandleResult<KernelObjectInfo> {
        let mut info = KernelObjectInfo::unknown();
        let result = syscall2(
            Syscall::HandleQuery,
            raw as usize,
            (&mut info as *mut KernelObjectInfo) as usize,
        );

        if result == usize::MAX {
            Err(HandleError::InvalidHandle)
        } else {
            Ok(info)
        }
    }

    fn from_kernel_raw(raw: RawHandle) -> HandleResult<Self> {
        let info = match Self::query_info(raw) {
            Ok(info) => info,
            Err(e) => {
                // Best-effort cleanup to avoid leaking a handle when introspection fails.
                let _ = syscall1(Syscall::HandleClose, raw as usize);
                return Err(e);
            }
        };

        Ok(Self { raw, info })
    }

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

        HandleError::from_syscall_result(result).and_then(Handle::from_kernel_raw)
    }

    /// Create a Handle from a raw handle value
    ///
    /// # Safety
    /// The caller must ensure that the raw handle is valid
    pub unsafe fn from_raw(raw: RawHandle) -> HandleResult<Self> {
        // Caller guarantees ownership/validity; we still query the kernel so that
        // later capability conversions can be validated without extra syscalls.
        Self::from_kernel_raw(raw)
    }

    /// Get the raw handle value
    pub fn as_raw(&self) -> RawHandle {
        self.raw
    }

    /// Get cached kernel object information for this handle.
    pub fn object_info(&self) -> KernelObjectInfo {
        self.info
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
        HandleError::from_syscall_result(result).map(|raw| Handle {
            raw,
            info: self.info,
        })
    }

    /// Set role metadata for this handle
    ///
    /// # Arguments
    /// * `role` - New role for the handle
    ///
    /// # Returns
    /// Success or HandleError on failure
    pub fn set_role(&self, role: u32) -> HandleResult<()> {
        let result = syscall2(Syscall::HandleSetRole, self.raw as usize, role as usize);
        HandleError::from_syscall_result(result).map(|_| ())
    }

    /// Get a StreamOps capability for this handle
    ///
    /// # Returns
    /// StreamOps capability if the handle supports stream operations
    pub fn as_stream(&self) -> HandleResult<StreamOps<'_>> {
        if !self.info.capabilities.stream_ops {
            return Err(HandleError::Unsupported);
        }
        Ok(StreamOps::from_handle(self))
    }

    /// Get a FileObject capability for this handle
    ///
    /// # Returns
    /// FileObject capability if the handle supports file operations
    pub fn as_file(&self) -> HandleResult<FileObject<'_>> {
        if !self.info.capabilities.file_ops {
            return Err(HandleError::Unsupported);
        }
        Ok(FileObject::from_handle(self))
    }

    /// Get a SocketObject capability for this handle
    ///
    /// # Returns
    /// SocketObject capability if the handle supports socket operations
    pub fn as_socket(&self) -> HandleResult<SocketObject<'_>> {
        if self.info.object_type != KernelObjectType::Socket {
            return Err(HandleError::Unsupported);
        }
        Ok(SocketObject::from_handle(self))
    }

    /// Get a SharedMemoryObject capability for this handle
    ///
    /// # Returns
    /// SharedMemoryObject capability if the handle supports shared memory operations
    pub fn as_shared_memory(&self) -> HandleResult<SharedMemoryObject<'_>> {
        if self.info.object_type != KernelObjectType::SharedMemory {
            return Err(HandleError::Unsupported);
        }
        Ok(SharedMemoryObject::from_handle(self))
    }

    /// Get a MemoryMappingOps capability for this handle
    ///
    /// # Returns
    /// MemoryMappingOps capability if the handle supports memory mapping operations
    pub fn as_memory_mapping(&self) -> HandleResult<MemoryMappingOps<'_>> {
        // Kernel introspection currently does not expose a dedicated
        // MemoryMappingOps capability flag. We conservatively allow it for
        // file-like objects and shared memory based on the kernel object type.
        match self.info.object_type {
            KernelObjectType::File
            | KernelObjectType::CharDevice
            | KernelObjectType::BlockDevice
            | KernelObjectType::SharedMemory => Ok(MemoryMappingOps::from_handle(self)),
            _ => Err(HandleError::Unsupported),
        }
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
