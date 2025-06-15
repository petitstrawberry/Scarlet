//! Capability traits for KernelObject resources
//! 
//! This module defines capability traits that represent the operations
//! that can be performed on different types of kernel objects.

use crate::{fs::FileSystemError, object::KernelObject};
use alloc::string::String;

/// Represents errors that can occur during stream I/O operations
#[derive(Debug, Clone)]
pub enum StreamError {
    /// I/O error occurred
    IoError,
    /// End of stream reached (EOF for reads)
    EndOfStream,
    /// Operation would block (for non-blocking streams)
    WouldBlock,
    /// Stream was closed or is invalid
    Closed,
    /// Invalid arguments provided (e.g., null buffer, invalid offset)
    InvalidArgument,
    /// Operation interrupted by signal
    Interrupted,
    /// Permission denied for this operation
    PermissionDenied,
    /// Device-specific error
    DeviceError,
    /// Operation not supported by this stream type
    NotSupported,
    /// No space left for write operations
    NoSpace,
    /// Broken pipe/connection
    BrokenPipe,
    /// Seek operation failed
    SeekError,
    /// FileSystemError
    FileSystemError(FileSystemError),
    /// Generic error with custom message
    Other(String),
}

impl From<FileSystemError> for StreamError {
    fn from(fs_err: FileSystemError) -> Self {
        StreamError::FileSystemError(fs_err)
    }
}

/// Stream operations capability
/// 
/// This trait represents the ability to perform stream-like operations
/// such as read, write, and seek on a resource.
pub trait StreamOps: Send + Sync {
    /// Read data from the stream
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError>;
    
    /// Write data to the stream
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError>;
}

/// Clone operations capability
/// 
/// This trait represents the ability to properly clone an object
/// with custom semantics. Objects that need special cloning behavior
/// (like pipes that need to update reader/writer counts) should implement this.
/// 
/// The presence of this capability indicates that the object needs custom
/// clone semantics beyond simple Arc::clone.
pub trait CloneOps: Send + Sync {
    /// Perform a custom clone operation and return the cloned object
    /// 
    /// This method should handle any object-specific cloning logic,
    /// such as incrementing reference counts for pipes or other shared resources.
    /// Returns the cloned object as a KernelObject.
    fn custom_clone(&self) -> KernelObject;
}