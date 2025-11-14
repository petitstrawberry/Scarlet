//! File operations capability module
//! 
//! This module provides system calls and traits for FileObject capability,
//! which extends StreamOps with file-specific operations like seek and metadata.

use core::any::Any;

use crate::object::capability::stream::{StreamOps, StreamError};
use crate::object::capability::control::ControlOps;
use crate::object::capability::memory_mapping::MemoryMappingOps;

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
/// This trait represents a file-like object that supports stream operations,
/// file-specific operations like seeking and metadata access, control
/// operations for device-specific functionality, and memory mapping operations.
/// Directory reading is handled through normal read() operations.
pub trait FileObject: StreamOps + ControlOps + MemoryMappingOps {
    /// Seek to a position in the file stream
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError>;
    
    /// Get metadata about the file
    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError>;

    /// Read data from a specific offset without changing internal position
    ///
    /// This method performs a random-access read operation that must not
    /// modify the file's current seek position. Filesystems that cannot
    /// support position-independent reads may return `StreamError::NotSupported`.
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let _ = (offset, buffer);
        Err(StreamError::NotSupported)
    }

    /// Write data to a specific offset without changing internal position
    ///
    /// Similar to [`read_at`], this operation must not adjust the file's
    /// internal cursor. Implementations can default to returning
    /// `StreamError::NotSupported` when random-access writes are unavailable.
    fn write_at(&self, offset: u64, buffer: &[u8]) -> Result<usize, StreamError> {
        let _ = (offset, buffer);
        Err(StreamError::NotSupported)
    }

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
    
    /// Synchronize file content to storage
    /// 
    /// This method ensures that any buffered changes to the file are written
    /// to the underlying storage device. This is important for filesystems
    /// that cache file content in memory.
    /// 
    /// # Returns
    /// 
    /// * `Result<(), StreamError>` - Ok if the sync was successful
    /// 
    /// # Errors
    /// 
    /// * `StreamError` - If the sync operation fails or is not supported
    fn sync(&self) -> Result<(), StreamError> {
        // Default implementation does nothing - files that don't cache content
        // don't need to sync
        Ok(())
    }
    
    fn as_any(&self) -> &dyn Any;
}
