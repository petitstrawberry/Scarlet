//! Stream Operations Capability for Scarlet Native API
//!
//! This module provides type-safe stream operations (read/write) for KernelObjects
//! that support the StreamOps capability.

use crate::handle::Handle;
use scarlet_sys::{Syscall, syscall3};

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
    /// Operation would block (non-blocking I/O)
    WouldBlock,
    /// Operation was interrupted before any data was transferred
    Interrupted,
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
    /// Convert a raw stream syscall result into a typed result.
    ///
    /// # Arguments
    ///
    /// * `result` - Raw return value from `StreamRead` or `StreamWrite`.
    ///
    /// # Returns
    ///
    /// The transferred byte count, or the corresponding [`StreamError`].
    pub fn from_syscall_result(result: usize) -> Result<usize, Self> {
        // Check for negative error codes (stored as large usize values)
        if result == usize::MAX {
            Err(StreamError::SystemError(-1)) // Generic error
        } else if result > (isize::MAX as usize) {
            // Negative value stored as usize indicates errno
            let errno = -(result as isize) as i32;
            match errno {
                4 => Err(StreamError::Interrupted), // EINTR
                11 => Err(StreamError::WouldBlock), // EAGAIN
                _ => Err(StreamError::SystemError(errno)),
            }
        } else {
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StreamError;

    #[test]
    fn syscall_result_distinguishes_interrupted_from_would_block() {
        assert_eq!(
            StreamError::from_syscall_result((-(4i32)) as usize),
            Err(StreamError::Interrupted)
        );
        assert_eq!(
            StreamError::from_syscall_result((-(11i32)) as usize),
            Err(StreamError::WouldBlock)
        );
        assert_eq!(StreamError::from_syscall_result(7), Ok(7));
    }
}

/// Stream operations capability for reading and writing data
pub struct StreamOps<'a> {
    handle: &'a Handle,
}

impl<'a> StreamOps<'a> {
    /// Construct a `StreamOps` capability from a [`Handle`] reference.
    ///
    /// This is crate-internal to prevent bypassing `Handle::as_stream` validation.
    pub(crate) fn from_handle(handle: &'a Handle) -> Self {
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
            self.handle.as_raw() as usize,
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
            self.handle.as_raw() as usize,
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
