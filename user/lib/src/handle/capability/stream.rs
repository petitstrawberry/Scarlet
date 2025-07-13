//! Stream Operations Capability for Scarlet Native API
//!
//! This module provides type-safe stream operations (read/write) for KernelObjects
//! that support the StreamOps capability.

use crate::syscall::{syscall3, Syscall};

/// Result type for stream operations
pub type StreamResult<T> = Result<T, StreamError>;

/// Errors that can occur during stream operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamError {
    /// Operation not supported by this object type
    Unsupported,
    /// Invalid handle
    InvalidHandle,
    /// End of stream reached
    EndOfStream,
    /// Input/output error
    IoError,
    /// Permission denied
    PermissionDenied,
    /// Invalid buffer or parameters
    InvalidParameter,
    /// Other system error
    SystemError(i32),
}

impl StreamError {
    pub fn from_syscall_result(result: usize) -> Result<usize, Self> {
        if result == usize::MAX {
            Err(StreamError::SystemError(-1)) // Generic error
        } else {
            Ok(result)
        }
    }
}

/// Stream operations capability for reading and writing data
pub struct StreamOps {
    handle: i32,
}

impl StreamOps {
    /// Create a StreamOps capability from a raw handle
    /// 
    /// # Safety
    /// The caller must ensure that the handle is valid and supports StreamOps
    pub fn from_handle(handle: i32) -> Self {
        Self { handle }
    }

    /// Read data from the stream
    /// 
    /// # Arguments
    /// * `buffer` - Buffer to read data into
    /// 
    /// # Returns
    /// Number of bytes actually read, or StreamError on failure
    pub fn read(&self, buffer: &mut [u8]) -> StreamResult<usize> {
        let result = syscall3(
            Syscall::StreamRead,
            self.handle as usize,
            buffer.as_mut_ptr() as usize,
            buffer.len(),
        );
        
        StreamError::from_syscall_result(result)
    }

    /// Write data to the stream
    /// 
    /// # Arguments
    /// * `buffer` - Data to write
    /// 
    /// # Returns
    /// Number of bytes actually written, or StreamError on failure
    pub fn write(&self, buffer: &[u8]) -> StreamResult<usize> {
        let result = syscall3(
            Syscall::StreamWrite,
            self.handle as usize,
            buffer.as_ptr() as usize,
            buffer.len(),
        );
        
        StreamError::from_syscall_result(result)
    }

    /// Write all data to the stream
    /// 
    /// This is a convenience method that calls write() repeatedly until
    /// all data is written or an error occurs.
    pub fn write_all(&self, mut buffer: &[u8]) -> StreamResult<()> {
        while !buffer.is_empty() {
            let bytes_written = self.write(buffer)?;
            if bytes_written == 0 {
                return Err(StreamError::IoError);
            }
            buffer = &buffer[bytes_written..];
        }
        Ok(())
    }

    /// Read exact amount of data from the stream
    /// 
    /// This is a convenience method that calls read() repeatedly until
    /// the buffer is filled or an error occurs.
    pub fn read_exact(&self, mut buffer: &mut [u8]) -> StreamResult<()> {
        while !buffer.is_empty() {
            let bytes_read = self.read(buffer)?;
            if bytes_read == 0 {
                return Err(StreamError::EndOfStream);
            }
            buffer = &mut buffer[bytes_read..];
        }
        Ok(())
    }
}
