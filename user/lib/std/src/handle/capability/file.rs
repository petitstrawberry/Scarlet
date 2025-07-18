//! File Object Capability for Scarlet Native API
//!
//! This module provides type-safe file operations (seek, truncate, metadata) for
//! KernelObjects that support the FileObject capability.

use crate::syscall::{syscall2, syscall3, Syscall};

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
}

impl FileMetadata {
    /// Check if this entry is a directory
    pub fn is_directory(&self) -> bool {
        self.file_type == 1 // FileType::Directory as u8
    }
    
    /// Check if this entry is a regular file
    pub fn is_file(&self) -> bool {
        self.file_type == 0 // FileType::RegularFile as u8
    }
    
    /// Check if this entry is a symbolic link
    pub fn is_symlink(&self) -> bool {
        self.file_type == 2 // FileType::SymbolicLink as u8
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

/// File object capability for file-specific operations
pub struct FileObject {
    handle: i32,
}

impl FileObject {
    /// Create a FileObject capability from a raw handle
    /// 
    /// # Safety
    /// The caller must ensure that the handle is valid and supports FileObject
    pub fn from_handle(handle: i32) -> Self {
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
            self.handle as usize,
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
            self.handle as usize,
            size as usize,
        );
        
        FileError::from_syscall_result(result).map(|_| ())
    }

    // /// Get metadata about the file
    // /// 
    // /// # Returns
    // /// FileMetadata structure or FileError on failure
    // pub fn metadata(&self) -> FileResult<FileMetadata> {
    //     // For now, we'll use a simple implementation
    //     // In the future, this could be enhanced to use a more sophisticated metadata syscall
        
    //     // Allocate space for metadata on the stack
    //     let mut metadata_raw = [0u64; 8]; // Size to hold kernel FileMetadata
        
    //     let result = syscall2(
    //         Syscall::FileMetadata,
    //         self.handle as usize,
    //         metadata_raw.as_mut_ptr() as usize,
    //     );
        
    //     match FileError::from_syscall_result(result) {
    //         Ok(_) => {
    //             Ok(FileMetadata {
    //                 size: metadata_raw[0],
    //                 file_type: metadata_raw[1] as u32,
    //                 permissions: metadata_raw[2] as u32,
    //                 created: metadata_raw[3],
    //                 modified: metadata_raw[4],
    //                 accessed: metadata_raw[5],
    //             })
    //         }
    //         Err(e) => Err(e),
    //     }
    // }

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
        // self.metadata().map(|meta| meta.size)
        todo!("Implement size retrieval using metadata syscall")
    }
}
