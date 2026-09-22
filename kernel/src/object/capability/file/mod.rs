//! File operations capability module
//!
//! This module provides system calls and traits for FileObject capability,
//! which extends StreamOps with file-specific operations like seek and metadata.

use core::any::Any;

use crate::object::capability::Selectable;
use crate::object::capability::control::ControlOps;
use crate::object::capability::memory_mapping::MemoryMappingOps;
use crate::object::capability::stream::{StreamError, StreamOps};

pub mod syscall;

pub use syscall::{sys_file_metadata, sys_file_seek, sys_file_truncate};

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
/// This trait represents a file-like object exposing stream operations,
/// file-specific operations like seeking and metadata access, control
/// operations for device-specific functionality, and memory mapping interfaces.
/// Individual operations may be unsupported. Directory reading is handled
/// through normal `read()` operations.
pub trait FileObject: StreamOps + ControlOps + MemoryMappingOps + Selectable {
    /// Seek to a position in the file stream
    ///
    /// # Arguments
    /// * `whence` - Absolute position or signed displacement relative to the cursor or EOF.
    ///
    /// # Returns
    /// The resulting absolute byte position, or an error if seeking fails or is unsupported.
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError>;

    /// Seek using a signed POSIX offset, rejecting negative or overflowing
    /// results without changing the cursor. `whence` is 0, 1, or 2 for the
    /// beginning, current position, or end of the file respectively.
    fn seek_signed(&self, _offset: i64, _whence: u32) -> Result<u64, StreamError> {
        Err(StreamError::NotSupported)
    }

    /// Get metadata about the file
    ///
    /// # Arguments
    /// * `self` - File object to inspect.
    ///
    /// # Returns
    /// A metadata snapshot, or a stream error if it cannot be obtained.
    fn metadata(&self) -> Result<crate::fs::FileMetadata, StreamError>;

    /// Update selected timestamps without reopening the pathname.
    fn set_times(&self, _times: crate::fs::FileTimeUpdate) -> Result<(), StreamError> {
        Err(StreamError::NotSupported)
    }

    /// Read data from a specific offset without changing internal position
    ///
    /// This method performs a random-access read operation that must not
    /// modify the file's current seek position. Filesystems that cannot
    /// support position-independent reads may return `StreamError::NotSupported`.
    ///
    /// # Arguments
    /// * `offset` - Absolute byte offset to read from.
    /// * `buffer` - Destination for the bytes read.
    ///
    /// # Returns
    /// The byte count, possibly shorter than `buffer.len()`, or an error.
    /// The default implementation returns `StreamError::NotSupported`.
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let _ = (offset, buffer);
        Err(StreamError::NotSupported)
    }

    /// Write data to a specific offset without changing internal position
    ///
    /// Similar to [`Self::read_at`], this operation must not adjust the file's
    /// internal cursor. Implementations can default to returning
    /// `StreamError::NotSupported` when random-access writes are unavailable.
    ///
    /// # Arguments
    /// * `offset` - Absolute byte offset to write to.
    /// * `buffer` - Source bytes; successful writes may consume only a prefix.
    ///
    /// # Returns
    /// The byte count written, or an error. The default implementation returns
    /// `StreamError::NotSupported`; success does not itself guarantee durability.
    fn write_at(&self, offset: u64, buffer: &[u8]) -> Result<usize, StreamError> {
        let _ = (offset, buffer);
        Err(StreamError::NotSupported)
    }

    /// Whether this object implements atomic append for its file type.
    fn supports_append(&self) -> bool {
        false
    }

    /// Write at the current EOF and move this open file's cursor past the
    /// bytes written. Selecting EOF and publishing the write must be one
    /// operation with respect to other writes and truncation on the inode.
    /// Implementations must not emulate this with an unlocked seek + write.
    fn append(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }

    /// Truncate the file to the specified size
    ///
    /// This method changes the size of the file to the specified length.
    /// If the new size is smaller than the current size, the file is truncated.
    /// If the new size is larger, the file is extended with zero bytes.
    /// The open-file cursor is unchanged, including when it lies beyond the
    /// resulting EOF. Unsupported lengths must fail without size-proportional
    /// infallible allocations.
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
    /// Implementations that cache file content must override this hook to write
    /// buffered changes to their backing storage, or report that synchronization
    /// is unsupported. The default implementation returns success without doing
    /// I/O; it is intended for objects with no pending content to flush, not as
    /// a general guarantee of storage-device durability.
    ///
    /// # Arguments
    ///
    /// * `self` - File object whose buffered content is to be synchronized.
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

    /// Borrow the concrete file object for downcasting.
    ///
    /// # Arguments
    /// * `self` - File object to inspect.
    ///
    /// # Returns
    /// A type-erased borrow with the same lifetime as `self`.
    fn as_any(&self) -> &dyn Any;
}

pub(crate) fn checked_seek_position(base: u64, offset: i64) -> Result<u64, StreamError> {
    let position = i128::from(base) + i128::from(offset);
    if position < 0 {
        return Err(StreamError::InvalidArgument);
    }
    if position > i128::from(i64::MAX) {
        return Err(crate::fs::FileSystemError::new(
            crate::fs::FileSystemErrorKind::ValueOverflow,
            "File offset exceeds signed 64-bit range",
        )
        .into());
    }
    Ok(position as u64)
}

/// Kernel callers may already hold a preemption guard. Preserve their
/// uncontended access, but never sleep behind another file operation there.
pub(crate) fn lock_file_operation(
    lock: &crate::sync::Mutex<()>,
) -> Result<crate::sync::MutexGuard<'_, ()>, crate::fs::FileSystemError> {
    if let Some(guard) = lock.try_lock() {
        return Ok(guard);
    }
    if !crate::sync::preemptible() {
        return Err(crate::fs::FileSystemError::new(
            crate::fs::FileSystemErrorKind::Busy,
            "File operation would block with preemption disabled",
        ));
    }
    Ok(lock.lock())
}

#[cfg(test)]
mod tests {
    #[test_case]
    fn file_operation_lock_never_sleeps_with_preemption_disabled() {
        let lock = crate::sync::Mutex::new(());
        let _preempt = crate::sync::PreemptGuard::new();
        let guard = super::lock_file_operation(&lock).unwrap();
        assert!(matches!(
            super::lock_file_operation(&lock),
            Err(error) if error.kind == crate::fs::FileSystemErrorKind::Busy
        ));
        drop(guard);
        assert!(super::lock_file_operation(&lock).is_ok());
    }
}
