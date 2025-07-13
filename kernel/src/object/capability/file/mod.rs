//! File operations capability module
//! 
//! This module provides system calls and traits for FileObject capability,
//! which extends StreamOps with file-specific operations like seek and metadata.

use crate::object::capability::stream::{StreamOps, StreamError};

pub mod syscall;

pub use syscall::{sys_file_seek, sys_file_truncate};

/// Seek operations for file positioning
#[derive(Debug, Clone, Copy)]
pub enum SeekFrom {
    /// Seek from the beginning of the file
    Start(u64),
    /// Seek relative to the current position
    Current(i64),
    /// Seek relative to the end of the file
    End(i64),
}

/// Trait for file objects
/// 
/// This trait represents a file-like object that supports both stream operations
/// and file-specific operations like seeking and metadata access.
/// Directory reading is handled through normal read() operations.
pub trait FileObject: StreamOps {
    /// Seek to a position in the file stream
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError>;
    
    /// Get metadata about the file
    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError>;

    /// Truncate the file to the specified size
    /// 
    /// This method changes the size of the file to the specified length.
    /// If the new size is smaller than the current size, the file is truncated.
    /// If the new size is larger, the file is extended with zero bytes.
    /// 
    /// # Arguments
    /// 
    /// * `size` - New size of the file in bytes
    /// 
    /// # Returns
    /// 
    /// * `Result<(), StreamError>` - Ok if the file was truncated successfully
    /// 
    /// # Errors
    /// 
    /// * `StreamError` - If the file is a directory or the operation is not supported
    fn truncate(&self, size: u64) -> Result<(), StreamError> {
        let _ = size;
        Err(StreamError::NotSupported)
    }
}
