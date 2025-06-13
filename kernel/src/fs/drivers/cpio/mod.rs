//! CPIO (Copy In/Out) filesystem implementation for the kernel.
//!
//! This module provides a read-only implementation of the CPIO archive format,
//! typically used for the initial ramdisk (initramfs). The CPIO format is a
//! simple archive format that stores files sequentially with headers containing
//! metadata.
//!
//! # Features
//! 
//! - Parsing of CPIO archives in the "new ASCII" format (magic "070701")
//! - Read-only access to files stored in the archive
//! - Directory traversal and metadata retrieval
//! 
//! # Limitations
//!
//! - Write operations are not supported as the filesystem is read-only
//! - Block operations are not implemented since CPIO is not a block-based filesystem
//!
//! # Components
//!
//! - `CpiofsEntry`: Represents an individual file or directory within the archive
//! - `Cpiofs`: The main filesystem implementation handling mounting and file operations
//! - `CpiofsFileObject`: Handles file read operations and seeking
//! - `CpiofsDriver`: Driver that creates CPIO filesystems from memory areas
//!
//! # Usage
//!
//! The CPIO filesystem is typically created from a memory region containing the
//! archive data, such as an initramfs loaded by the bootloader:
//!
//! ```rust
//! let cpio_driver = CpiofsDriver;
//! let fs = cpio_driver.create_from_memory(&initramfs_memory_area)?;
//! vfs_manager.register_filesystem(fs)?;
//! ```
use alloc::{boxed::Box, format, string::{String, ToString}, sync::Arc, vec::Vec};
use spin::{Mutex, RwLock};

use crate::{driver_initcall, fs::{
    get_fs_driver_manager, Directory, DirectoryEntry, FileObject, FileMetadata, FileOperations, FileSystem, FileSystemDriver, FileSystemError, FileSystemErrorKind, FileSystemType, FileType, VirtualFileSystem, SeekFrom
}, vm::vmem::MemoryArea, object::capability::{StreamOps, StreamError}};

/// Structure representing an Initramfs entry
#[derive(Debug, Clone)]
pub struct CpiofsEntry {
    pub name: String,
    pub file_type: FileType,
    pub size: usize,
    pub modified_time: u64,
    pub data: Option<Vec<u8>>, // File data (None for directories)
}

/// Structure representing the entire Initramfs
pub struct Cpiofs {
    name: &'static str,
    entries: Mutex<Vec<CpiofsEntry>>, // List of entries
    mounted: bool,
    mount_point: String,
}

impl Cpiofs {
    /// Create a new Initramfs
    /// 
    /// # Arguments
    /// 
    /// * `name` - The name of the filesystem
    /// * `cpio_data` - The CPIO data to parse
    /// 
    /// # Returns
    /// 
    /// A result containing the created Cpiofs instance or an error
    /// 
    pub fn new(name: &'static str, cpio_data: &[u8]) -> Result<Self, FileSystemError> {
        let entries = Self::parse_cpio(cpio_data)?;
        Ok(Self {
            name,
            entries: Mutex::new(entries),
            mounted: false,
            mount_point: String::new(),
        })
    }

    /// Parse CPIO data to generate entries
    /// 
    /// # Arguments
    /// 
    /// * `cpio_data` - The CPIO data to parse
    /// 
    /// # Returns
    /// 
    /// A result containing a vector of CpiofsEntry or an error
    /// 
    fn parse_cpio(cpio_data: &[u8]) -> Result<Vec<CpiofsEntry>, FileSystemError> {
        let mut entries = Vec::new();
        let mut offset = 0;

        while offset < cpio_data.len() {
            if offset + 110 > cpio_data.len() {
                break; // Exit if the header is incomplete
            }

            // Parse the header
            let header = &cpio_data[offset..offset + 110];
            let name_size = usize::from_str_radix(
                core::str::from_utf8(&header[94..102]).unwrap_or("0"),
                16,
            )
            .unwrap_or(0);

            // Check magic number
            if &header[0..6] != b"070701" {
                return Err(FileSystemError {
                    kind: FileSystemErrorKind::InvalidData,
                    message: format!("Invalid CPIO magic number {:#x} at offset {}", 
                        u32::from_str_radix(core::str::from_utf8(&header[0..6]).unwrap_or("0"), 16).unwrap_or(0),
                        offset,
                    ),
                });
            }

            // Get the file name
            let name_offset = offset + 110;
            let name_end = name_offset + name_size - 1; // Exclude `\0`
            if name_end > cpio_data.len() {
                break; // Exit if the file name is incomplete
            }

            let name = core::str::from_utf8(&cpio_data[name_offset..name_end])
                .unwrap_or("")
                .to_string();

            // Exit if the end marker is found
            if name == "TRAILER!!!" {
                break;
            }

            // Get the mode
            let mode = usize::from_str_radix(
                core::str::from_utf8(&header[14..22]).unwrap_or("0"),
                16,
            ).unwrap_or(0);

            let file_type = if mode & 0o170000 == 0o040000 {
                FileType::Directory
            } else if mode & 0o170000 == 0o100000 {
                FileType::RegularFile
            } else {
                FileType::Unknown
            };

            let modified_time = u64::from_str_radix(
                core::str::from_utf8(&header[46..54]).unwrap_or("0"),
                16,
            ).unwrap_or(0);

            // Get the size
            let file_size = usize::from_str_radix(
                core::str::from_utf8(&header[54..62]).unwrap_or("0"),
                16,
            )
            .unwrap_or(0);

            // Get the file data
            let data_offset = (name_offset + name_size + 3) & !3; // Align to 4-byte boundary
            let data_end = data_offset + file_size;
            let data = if file_size > 0 && data_end <= cpio_data.len() {
                Some(cpio_data[data_offset..data_end].to_vec())
            } else {
                None
            };

            entries.push(CpiofsEntry {
                name,
                file_type,
                size: file_size,
                modified_time,
                data,
            });

            // Move to the next entry
            offset = (data_end + 3) & !3; // Align to 4-byte boundary
        }

        Ok(entries)
    }

    fn normalize_path(&self, path: &str) -> String {
        // Convert absolute paths to relative paths considering the mount point
        if path.starts_with('/') {
            path.trim_start_matches('/').to_string()
        } else {
            path.to_string()
        }
    }
    
}

impl FileSystem for Cpiofs {
    fn mount(&mut self, mount_point: &str) -> Result<(), FileSystemError> {
        if self.mounted {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::AlreadyExists,
                message: "File system already mounted".to_string(),
            });
        }
        self.mounted = true;
        self.mount_point = mount_point.to_string();
        Ok(())
    }

    fn unmount(&mut self) -> Result<(), FileSystemError> {
        if !self.mounted {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: "File system not mounted".to_string(),
            });
        }
        self.mounted = false;
        self.mount_point = String::new();
        Ok(())
    }

    fn name(&self) -> &str {
        self.name
    }
}

impl Cpiofs {
    /// Calculate file_id from path (for hardlink support)
    /// 
    /// CPIO doesn't natively support hardlinks, so we use path hash as file_id
    fn calculate_file_id(&self, path: &str) -> u64 {
        // Simple hash function since we don't have DefaultHasher in no_std
        let mut hash: u64 = 0;
        for byte in path.bytes() {
            hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
        }
        hash
    }
}

impl FileOperations for Cpiofs {
    fn open(&self, path: &str, _flags: u32) -> Result<Arc<dyn crate::fs::FileObject>, FileSystemError> {
        let path = self.normalize_path(path);
        let entries = self.entries.lock();
        if let Some(entry) = entries.iter().find(|e| e.name == path) {
            Ok(Arc::new(CpiofsFileObject {
                content: RwLock::new(entry.data.clone().unwrap_or_default()),
                position: RwLock::new(0),
            }))
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File not found: {}", path),
            })
        }
    }

    fn read_dir(&self, _path: &str) -> Result<Vec<DirectoryEntry>, FileSystemError> {
        let path = self.normalize_path(_path);
        let entries = self.entries.lock();
    
        // Filter entries in the specified directory
        let filtered_entries: Vec<DirectoryEntry> = entries
            .iter()
            .filter_map(|e| {
                // Determine entries within the directory
                let parent_path = e.name.rfind('/').map_or("", |idx| &e.name[..idx]);
                if parent_path == path {
                    // Extract only the file name
                    let file_name = e.name.rfind('/').map_or(&e.name[..], |idx| &e.name[idx + 1..]);
                    Some(DirectoryEntry {
                        name: file_name.to_string(),
                        file_type: e.file_type,
                        size: e.size,
                        file_id: self.calculate_file_id(&e.name), // Use path hash as file_id
                        metadata: None,
                    })
                } else {
                    None
                }
            })
            .collect();
    
        if filtered_entries.is_empty() && path != "" && path != "/" {
            // Return an error if the specified directory does not exist
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("Directory not found: {}", _path),
            });
        }
    
        Ok(filtered_entries)
    }

    fn create_file(&self, _path: &str, _file_type: FileType) -> Result<(), FileSystemError> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::ReadOnly,
            message: "Initramfs is read-only".to_string(),
        })
    }

    fn create_dir(&self, _path: &str) -> Result<(), FileSystemError> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::ReadOnly,
            message: "Initramfs is read-only".to_string(),
        })
    }

    fn remove(&self, _path: &str) -> Result<(), FileSystemError> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::ReadOnly,
            message: "Initramfs is read-only".to_string(),
        })
    }

    fn metadata(&self, path: &str) -> Result<FileMetadata, FileSystemError> {
        let path = self.normalize_path(path);
        let entries = self.entries.lock();
        if let Some(entry) = entries.iter().find(|e| e.name == path) {
            Ok(FileMetadata {
                file_type: entry.file_type,
                size: entry.size,
                permissions: crate::fs::FilePermission {
                    read: true,
                    write: false,
                    execute: false,
                },
                created_time: 0,
                modified_time: entry.modified_time,
                accessed_time: 0,
                file_id: self.calculate_file_id(&path),
                link_count: 1, // CPIO doesn't support hardlinks, so always 1
            })
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File not found: {}", path),
            })
        }
    }
    
    fn root_dir(&self) -> Result<crate::fs::Directory, FileSystemError> {
        Ok(Directory::open(self.mount_point.clone() + "/"))
    }
}

struct CpiofsFileObject {
    content: RwLock<Vec<u8>>,
    position: RwLock<usize>,
}

impl StreamOps for CpiofsFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        let content = self.content.read();
        let mut position = self.position.write();
        let available = content.len() - *position;
        let to_read = buffer.len().min(available);
        buffer[..to_read].copy_from_slice(&content[*position..*position + to_read]);
        *position += to_read;
        Ok(to_read)
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }

    fn release(&self) -> Result<(), StreamError> {
        Ok(())
    }
}

impl FileObject for CpiofsFileObject {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut position = self.position.write();
        let content = self.content.read();
        let new_pos = match whence {
            SeekFrom::Start(offset) => offset as usize,
            SeekFrom::Current(offset) => {
                if offset < 0 && *position < offset.abs() as usize {
                    0
                } else if offset < 0 {
                    *position - offset.abs() as usize
                } else {
                    *position + offset as usize
                }
            },
            SeekFrom::End(offset) => {
                let end = content.len();
                if offset < 0 && end < offset.abs() as usize {
                    0
                } else if offset < 0 {
                    end - offset.abs() as usize
                } else {
                    end + offset as usize
                }
            },
        };
        
        *position = new_pos;
        Ok(*position as u64)
    }

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        let content = self.content.read();
        Ok(FileMetadata {
            file_type: FileType::RegularFile,
            size: content.len(),
            permissions: crate::fs::FilePermission {
                read: true,
                write: false,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: 0, // CPIO file handle doesn't know the path, so use 0
            link_count: 1,
        })
    }
    
    fn readdir(&self) -> Result<Vec<DirectoryEntry>, StreamError> {
        Err(StreamError::NotSupported)
    }
}

/// Driver for CPIO-format filesystems (initramfs)
/// 
/// This driver creates filesystems from memory areas only.
pub struct CpiofsDriver;

impl FileSystemDriver for CpiofsDriver {
    fn name(&self) -> &'static str {
        "cpiofs"
    }
    
    /// This filesystem only supports creation from memory
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Memory
    }
    
    /// Create a file system from memory area
    /// 
    /// # Arguments
    /// 
    /// * `memory_area` - A reference to the memory area containing the CPIO filesystem data
    /// 
    /// # Returns
    /// 
    /// A result containing a boxed CPIO filesystem or an error
    /// 
    fn create_from_memory(&self, memory_area: &MemoryArea) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
        let data = unsafe { memory_area.as_slice() };
        // Create the Cpiofs from the memory data
        match Cpiofs::new("cpiofs", data) {
            Ok(cpio_fs) => Ok(Box::new(cpio_fs)),
            Err(err) => Err(FileSystemError {
                kind: FileSystemErrorKind::InvalidData,
                message: format!("Failed to create CPIO filesystem from memory: {}", err.message),
            })
        }
    }
    
    fn create_with_params(&self, params: &dyn crate::fs::params::FileSystemParams) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
        use crate::fs::params::*;
        
        // Try to downcast to CpioFSParams
        if let Some(_cpio_params) = params.as_any().downcast_ref::<CpioFSParams>() {
            // CPIO filesystem requires memory area for creation, so we cannot create from parameters alone
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "CPIO filesystem requires memory area for creation. Use create_from_memory instead.".to_string(),
            });
        }
        
        // Try to downcast to BasicFSParams for compatibility
        if let Some(_basic_params) = params.as_any().downcast_ref::<BasicFSParams>() {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotSupported,
                message: "CPIO filesystem requires memory area for creation. Use create_from_memory instead.".to_string(),
            });
        }
        
        // If all downcasts fail, return error
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "CPIO filesystem requires CpioFSParams and memory area for creation".to_string(),
        })
    }
}

fn register_driver() {
    let fs_driver_manager = get_fs_driver_manager();
    fs_driver_manager.register_driver(Box::new(CpiofsDriver));
}

driver_initcall!(register_driver);

#[cfg(test)]
mod tests;