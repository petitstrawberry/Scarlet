//! File Object Capability for Scarlet Native API
//!
//! This module provides type-safe file operations (seek, truncate, metadata) for
//! KernelObjects that support the FileObject capability.

use crate::handle::Handle;
use scarlet_sys::{
    FILE_TYPE_DIRECTORY, FILE_TYPE_REGULAR, FILE_TYPE_SYMLINK, RawFileMetadata, Syscall, syscall2,
    syscall3,
};

/// Result type for file operations
pub type FileResult<T> = Result<T, FileError>;

/// Errors that can occur during file operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileError {
    /// Operation not supported by this object type
    Unsupported,
    /// Invalid handle
    InvalidHandle,
    /// Invalid seek position
    InvalidSeek,
    /// Input/output error
    IoError,
    /// Permission denied
    PermissionDenied,
    /// Invalid parameters
    InvalidParameter,
    /// Other system error
    SystemError(i32),
}

impl FileError {
    pub fn from_syscall_result(result: usize) -> Result<usize, Self> {
        if result == usize::MAX {
            Err(FileError::SystemError(-1)) // Generic error
        } else {
            Ok(result)
        }
    }
}

/// Seek origin for file positioning
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekFrom {
    /// Seek from the start of the file
    Start(u64),
    /// Seek relative to the current position
    Current(i64),
    /// Seek from the end of the file
    End(i64),
}

impl SeekFrom {
    /// Convert to the kernel's representation for syscalls
    pub(crate) fn to_syscall_args(self) -> (i64, i32) {
        match self {
            SeekFrom::Start(offset) => (offset as i64, 0),
            SeekFrom::Current(offset) => (offset, 1),
            SeekFrom::End(offset) => (offset, 2),
        }
    }
}

/// File metadata information
#[derive(Debug, Clone)]
#[repr(C)]
pub struct FileMetadata {
    /// Size of the file in bytes
    pub size: u64,
    /// File type flags
    pub file_type: u32,
    /// Permissions flags
    pub permissions: u32,
    /// Creation timestamp (if available)
    pub created: u64,
    /// Last modification timestamp
    pub modified: u64,
    /// Last access timestamp
    pub accessed: u64,
    /// Filesystem-local stable file identifier
    pub file_id: u64,
    /// Number of hard links to this file
    pub link_count: u32,
}

impl FileMetadata {
    /// Check if this entry is a directory
    pub fn is_directory(&self) -> bool {
        self.file_type == FILE_TYPE_DIRECTORY
    }

    /// Check if this entry is a regular file
    pub fn is_file(&self) -> bool {
        self.file_type == FILE_TYPE_REGULAR
    }

    /// Check if this entry is a symbolic link
    pub fn is_symlink(&self) -> bool {
        self.file_type == FILE_TYPE_SYMLINK
    }

    /// Get file type as a human-readable string
    pub fn file_type_str(&self) -> &'static str {
        match self.file_type {
            0 => "file",
            1 => "directory",
            2 => "symlink",
            3 => "device",
            4 => "pipe",
            5 => "socket",
            _ => "unknown",
        }
    }
}

impl From<RawFileMetadata> for FileMetadata {
    fn from(raw: RawFileMetadata) -> Self {
        Self {
            size: raw.size,
            file_type: raw.file_type,
            permissions: raw.permissions,
            created: raw.created,
            modified: raw.modified,
            accessed: raw.accessed,
            file_id: raw.file_id,
            link_count: raw.link_count,
        }
    }
}

/// File object capability for file-specific operations
pub struct FileObject<'a> {
    handle: &'a Handle,
}

impl<'a> FileObject<'a> {
    /// Construct a `FileObject` capability from a [`Handle`] reference.
    ///
    /// This is crate-internal to prevent bypassing `Handle::as_file` validation.
    pub(crate) fn from_handle(handle: &'a Handle) -> Self {
        Self { handle }
    }

    /// Seek to a position in the file
    ///
    /// # Arguments
    /// * `pos` - Position to seek to
    ///
    /// # Returns
    /// New absolute position from the start of the file, or FileError on failure
    pub fn seek(&self, pos: SeekFrom) -> FileResult<u64> {
        let (offset, whence) = pos.to_syscall_args();

        let result = syscall3(
            Syscall::FileSeek,
            self.handle.as_raw() as usize,
            offset as usize,
            whence as usize,
        );

        FileError::from_syscall_result(result).map(|pos| pos as u64)
    }

    /// Truncate the file to the specified size
    ///
    /// # Arguments
    /// * `size` - New size of the file in bytes
    ///
    /// # Returns
    /// Success or FileError on failure
    pub fn truncate(&self, size: u64) -> FileResult<()> {
        let result = syscall2(
            Syscall::FileTruncate,
            self.handle.as_raw() as usize,
            size as usize,
        );

        FileError::from_syscall_result(result).map(|_| ())
    }

    /// Get metadata about the file
    ///
    /// # Returns
    /// FileMetadata structure or FileError on failure
    pub fn metadata(&self) -> FileResult<FileMetadata> {
        let mut metadata = RawFileMetadata::default();
        let result = syscall2(
            Syscall::FileMetadata,
            self.handle.as_raw() as usize,
            (&mut metadata as *mut RawFileMetadata) as usize,
        );

        FileError::from_syscall_result(result).map(|_| metadata.into())
    }

    /// Get the current position in the file
    ///
    /// This is a convenience method equivalent to seek(SeekFrom::Current(0))
    pub fn position(&self) -> FileResult<u64> {
        self.seek(SeekFrom::Current(0))
    }

    /// Get the size of the file
    ///
    /// This is a convenience method that gets metadata and returns just the size
    pub fn size(&self) -> FileResult<u64> {
        self.metadata().map(|meta| meta.size)
    }
}
