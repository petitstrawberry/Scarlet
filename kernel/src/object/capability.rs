//! Capability traits for KernelObject resources
//! 
//! This module defines capability traits that represent the operations
//! that can be performed on different types of kernel objects.

use crate::fs::{FileSystemError, FileSystemErrorKind, SeekFrom};
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
    
    /// Release any resources associated with this stream
    fn release(&self) -> Result<(), StreamError>;
}

/// File-specific stream operations capability
/// 
/// This trait extends StreamOps with file-specific operations like seeking
/// and metadata access.
pub trait FileStreamOps: StreamOps {
    /// Seek to a position in the file stream
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError>;
    
    /// Get metadata about the file
    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError>;
}
