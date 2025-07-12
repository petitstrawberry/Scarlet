//! Scarlet Native API - Type-safe KernelObject handling for Scarlet applications
//! 
//! This module provides a higher-level, type-safe API for Scarlet Native applications
//! that want to take full advantage of Scarlet's KernelObject design without being
//! limited to POSIX-style file descriptor semantics.

use crate::syscall::{syscall1, syscall2, syscall3, Syscall};
use crate::ffi::str_to_cstr_bytes;
use crate::boxed::Box;

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
    /// Other system error
    SystemError(i32),
}

impl HandleError {
    pub fn from_syscall_error(result: usize) -> Self {
        match result as i32 {
            -1 => HandleError::SystemError(-1),
            -2 => HandleError::InvalidHandle,
            -3 => HandleError::Unsupported,
            -4 => HandleError::PermissionDenied,
            -5 => HandleError::OutOfResources,
            other => HandleError::SystemError(other),
        }
    }
}

/// Simple handle wrapper that just holds a raw handle ID
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle {
    raw: i32,
}

impl Handle {
    /// Create a new Handle from a raw handle value
    pub fn from_raw(raw: i32) -> Self {
        Self { raw }
    }
    
    /// Get the raw handle value
    pub fn as_raw(&self) -> i32 {
        self.raw
    }
    
    /// Check if this is an invalid handle
    pub fn is_invalid(&self) -> bool {
        self.raw < 0
    }
    
    /// Close this handle
    pub fn close(self) -> HandleResult<()> {
        let result = syscall1(Syscall::Close, self.raw as usize);
        if result == 0 {
            Ok(())
        } else {
            Err(HandleError::from_syscall_error(result))
        }
    }
    
    /// Read from this handle
    pub fn read(&self, buf: &mut [u8]) -> HandleResult<usize> {
        let result = syscall3(
            Syscall::Read,
            self.raw as usize,
            buf.as_mut_ptr() as usize,
            buf.len(),
        );
        if result == usize::MAX {
            Err(HandleError::from_syscall_error(result))
        } else {
            Ok(result)
        }
    }
    
    /// Write to this handle
    pub fn write(&self, buf: &[u8]) -> HandleResult<usize> {
        let result = syscall3(
            Syscall::Write,
            self.raw as usize,
            buf.as_ptr() as usize,
            buf.len(),
        );
        if result == usize::MAX {
            Err(HandleError::from_syscall_error(result))
        } else {
            Ok(result)
        }
    }
}

/// Open a file and return a Handle to the resulting KernelObject
/// 
/// # Arguments
/// * `path` - Path to the file
/// * `flags` - Open flags (O_RDONLY, O_WRONLY, etc.)
/// 
/// # Returns
/// A Handle to the opened KernelObject, or an error
pub fn open(path: &str, flags: usize) -> HandleResult<Handle> {
    let path_bytes = str_to_cstr_bytes(path).map_err(|_| HandleError::SystemError(-1))?;
    let path_boxed_slice = path_bytes.into_boxed_slice();
    let path_len = path_boxed_slice.len();
    let path_ptr = Box::into_raw(path_boxed_slice) as *const u8 as usize;
    
    let result = syscall2(Syscall::Open, path_ptr, flags);
    
    // Properly free the allocated memory
    let _ = unsafe { 
        Box::<[u8]>::from_raw(core::slice::from_raw_parts_mut(path_ptr as *mut u8, path_len)) 
    };
    
    if result == usize::MAX {
        Err(HandleError::SystemError(-1))
    } else {
        Ok(Handle::from_raw(result as i32))
    }
}

/// Duplicate a handle
pub fn dup(handle: i32) -> HandleResult<Handle> {
    let result = syscall1(Syscall::Dup, handle as usize);
    
    if result == usize::MAX {
        Err(HandleError::SystemError(-1))
    } else {
        Ok(Handle::from_raw(result as i32))
    }
}

/// Information about a KernelObject returned by handle_query syscall
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct KernelObjectInfo {
    /// The type of the underlying KernelObject
    pub object_type: KernelObjectType,
    /// Capabilities available for this object
    pub capabilities: ObjectCapabilities,
    /// Current role/usage of this handle
    pub handle_role: HandleRole,
}

/// Types of KernelObjects that can be created
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelObjectType {
    File,
    Pipe,
}

/// Capabilities that a KernelObject supports
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ObjectCapabilities {
    /// Can be used with read() operations
    pub supports_stream_read: bool,
    /// Can be used with write() operations  
    pub supports_stream_write: bool,
    /// Can be used with seek() operations (files)
    pub supports_file_seek: bool,
    /// Can be duplicated/cloned
    pub supports_clone: bool,
}

/// Current role/usage intent of a handle
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleRole {
    /// Standard input/output/error streams
    StandardInputOutput(StandardInputOutput),
    /// Inter-process communication channel
    IpcChannel,
    /// Default/generic usage
    Regular,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardInputOutput {
    Stdin,
    Stdout,
    Stderr,
}

/// Query information about a KernelObject handle
/// 
/// This syscall allows user space to discover the type and capabilities
/// of a KernelObject, enabling type-safe wrapper implementations.
pub fn handle_query(handle: i32) -> HandleResult<KernelObjectInfo> {
    let mut info = KernelObjectInfo {
        object_type: KernelObjectType::File, // dummy
        capabilities: ObjectCapabilities {
            supports_stream_read: false,
            supports_stream_write: false,
            supports_file_seek: false,
            supports_clone: false,
        },
        handle_role: HandleRole::Regular,
    };
    
    let result = syscall2(
        Syscall::HandleQuery,
        handle as usize,
        &mut info as *mut KernelObjectInfo as usize,
    );
    
    if result == 0 {
        Ok(info)
    } else {
        Err(HandleError::from_syscall_error(result))
    }
}

/// Standard I/O handles
impl Handle {
    pub const STDIN: Handle = Handle { raw: 0 };
    pub const STDOUT: Handle = Handle { raw: 1 };
    pub const STDERR: Handle = Handle { raw: 2 };
}

// For backward compatibility, provide some trait-like methods
pub trait ReadableHandle {
    fn read(&self, buf: &mut [u8]) -> HandleResult<usize>;
}

pub trait WritableHandle {
    fn write(&self, buf: &[u8]) -> HandleResult<usize>;
}

impl ReadableHandle for Handle {
    fn read(&self, buf: &mut [u8]) -> HandleResult<usize> {
        self.read(buf)
    }
}

impl WritableHandle for Handle {
    fn write(&self, buf: &[u8]) -> HandleResult<usize> {
        self.write(buf)
    }
}
