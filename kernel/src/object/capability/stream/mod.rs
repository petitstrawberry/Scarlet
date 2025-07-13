//! Stream operations capability module
//! 
//! This module provides system calls and traits for StreamOps capability,
//! which enables read and write operations on KernelObjects.

use crate::fs::FileSystemError;
use alloc::string::String;

pub mod syscall;

pub use syscall::{sys_stream_read, sys_stream_write};

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
