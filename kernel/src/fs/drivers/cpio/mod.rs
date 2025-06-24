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
    get_fs_driver_manager, Directory, DirectoryEntry, DirectoryEntryInternal, FileObject, FileMetadata, FileOperations, FileSystem, FileSystemDriver, FileSystemError, FileSystemErrorKind, FileSystemType, FileType, VirtualFileSystem, SeekFrom
}, vm::vmem::MemoryArea, object::capability::{StreamOps, StreamError}};

/// Structure representing an Initramfs entry
#[derive(Debug, Clone)]
pub struct CpiofsEntry {
    pub name: String,
    pub file_type: FileType,
    pub size: usize,
    pub modified_time: u64,
    pub data_offset: usize,  // Offset in the original CPIO data
    pub data_size: usize,    // Size of the data
}

/// Shared CPIO data that can be referenced by multiple file objects
#[derive(Debug)]
pub struct SharedCpioData {
    pub raw_data_ptr: *const u8,  // Pointer to original CPIO archive data
    pub raw_data_size: usize,     // Size of the original CPIO archive data
}

unsafe impl Send for SharedCpioData {}
unsafe impl Sync for SharedCpioData {}

impl SharedCpioData {
    /// Get a slice reference to the raw data
    pub fn as_slice(&self) -> &[u8] {
        unsafe {
            core::slice::from_raw_parts(self.raw_data_ptr, self.raw_data_size)
        }
    }
}

/// Structure representing the entire Initramfs
pub struct Cpiofs {
    name: &'static str,
    entries: Mutex<Vec<CpiofsEntry>>, // List of entries
    shared_data: Arc<SharedCpioData>,  // Shared reference to original CPIO data
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
        let shared_data = Arc::new(SharedCpioData {
            raw_data_ptr: cpio_data.as_ptr(),
            raw_data_size: cpio_data.len(),
        });
        let entries = Self::parse_cpio(shared_data.as_slice())?;
        Ok(Self {
            name,
            entries: Mutex::new(entries),
            shared_data,
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

            // Get the file data offset and size
            let data_offset = (name_offset + name_size + 3) & !3; // Align to 4-byte boundary
            let data_end = data_offset + file_size;
            
            if data_end > cpio_data.len() {
                break; // Exit if data extends beyond the buffer
            }

            entries.push(CpiofsEntry {
                name,
                file_type,
                size: file_size,
                modified_time,
                data_offset,
                data_size: file_size,
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

    /// Helper method to get directory entries for a given path (assumes lock is already held)
    fn get_directory_entries_internal(&self, path: &str, entries: &Vec<CpiofsEntry>) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        let normalized_path = if path.is_empty() || path == "/" { "" } else { path };
    
        let mut result_entries = Vec::new();
        
        // Add "." entry (current directory)
        result_entries.push(DirectoryEntryInternal {
            name: ".".to_string(),
            file_type: FileType::Directory,
            size: 0,
            file_id: self.calculate_file_id(&path), // Use directory path hash as file_id
            metadata: None,
        });

        // Add ".." entry (parent directory)
        let parent_path = if path.is_empty() || path == "/" {
            "/" // Root's parent is itself
        } else {
            path.rfind('/').map_or("/", |idx| {
                if idx == 0 { "/" } else { &path[..idx] }
            })
        };
        
        result_entries.push(DirectoryEntryInternal {
            name: "..".to_string(),
            file_type: FileType::Directory,
            size: 0,
            file_id: self.calculate_file_id(parent_path), // Use parent directory path hash as file_id
            metadata: None,
        });

        // Filter regular entries in the specified directory
        let mut regular_entries: Vec<DirectoryEntryInternal> = entries
            .iter()
            .filter_map(|e| {
                // Determine entries within the directory
                let parent_path = e.name.rfind('/').map_or("", |idx| &e.name[..idx]);
                if parent_path == normalized_path {
                    // Extract only the file name
                    let file_name = e.name.rfind('/').map_or(&e.name[..], |idx| &e.name[idx + 1..]);
                    // Skip empty names
                    if !file_name.is_empty() {
                        Some(DirectoryEntryInternal {
                            name: file_name.to_string(),
                            file_type: e.file_type,
                            size: e.size,
                            file_id: self.calculate_file_id(&e.name), // Use path hash as file_id
                            metadata: None,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();
    
        // Sort regular entries by file_id (ascending order)
        regular_entries.sort_by_key(|entry| entry.file_id);

        // Append sorted regular entries to the result
        // (Note: "." and ".." are already at the beginning)
        result_entries.extend(regular_entries);
    
        // Always return success, even for empty directories
        Ok(result_entries)
    }
}

impl FileOperations for Cpiofs {
    fn open(&self, path: &str, _flags: u32) -> Result<Arc<dyn crate::fs::FileObject>, FileSystemError> {
        let path = self.normalize_path(path);
        let entries = self.entries.lock();
        
        if let Some(entry) = entries.iter().find(|e| e.name == path) {
            // Found the entry - create appropriate file object
            if entry.file_type == FileType::Directory {
                // For directories, prepare directory entries for streaming
                let dir_entries = self.get_directory_entries_internal(&path, &entries)?;
                Ok(Arc::new(CpiofsFileObject {
                    shared_data: Arc::clone(&self.shared_data),
                    data_offset: 0,  // Not used for directories
                    data_size: 0,    // Not used for directories
                    position: RwLock::new(0),
                    file_type: FileType::Directory,
                    directory_entries: Some(dir_entries),
                }))
            } else {
                // For regular files - reference the data instead of cloning
                Ok(Arc::new(CpiofsFileObject {
                    shared_data: Arc::clone(&self.shared_data),
                    data_offset: entry.data_offset,
                    data_size: entry.data_size,
                    position: RwLock::new(0),
                    file_type: FileType::RegularFile,
                    directory_entries: None,
                }))
            }
        } else if path.is_empty() || path == "/" {
            // Handle root directory case - even if not explicitly in CPIO entries
            let dir_entries = self.get_directory_entries_internal(&path, &entries)?;
            Ok(Arc::new(CpiofsFileObject {
                shared_data: Arc::clone(&self.shared_data),
                data_offset: 0,
                data_size: 0,
                position: RwLock::new(0),
                file_type: FileType::Directory,
                directory_entries: Some(dir_entries),
            }))
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File not found: {}", path),
            })
        }
    }

    fn readdir(&self, _path: &str) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        let path = self.normalize_path(_path);
        let entries = self.entries.lock();
    
        // Check if directory exists first
        let directory_exists = if path.is_empty() || path == "/" {
            true // Root directory always exists
        } else {
            entries.iter().any(|e| {
                let parent_path = e.name.rfind('/').map_or("", |idx| &e.name[..idx]);
                parent_path == path || e.name == path
            })
        };

        if !directory_exists {
            return Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("Directory not found: {}", _path),
            });
        }

        let mut result_entries = Vec::new();

        // Add "." entry (current directory)
        result_entries.push(DirectoryEntryInternal {
            name: ".".to_string(),
            file_type: FileType::Directory,
            size: 0,
            file_id: self.calculate_file_id(&path), // Use directory path hash as file_id
            metadata: None,
        });

        // Add ".." entry (parent directory)
        let parent_path = if path.is_empty() || path == "/" {
            "/" // Root's parent is itself
        } else {
            path.rfind('/').map_or("/", |idx| {
                if idx == 0 { "/" } else { &path[..idx] }
            })
        };
        
        result_entries.push(DirectoryEntryInternal {
            name: "..".to_string(),
            file_type: FileType::Directory,
            size: 0,
            file_id: self.calculate_file_id(parent_path), // Use parent directory path hash as file_id
            metadata: None,
        });

        // Filter regular entries in the specified directory
        let mut regular_entries: Vec<DirectoryEntryInternal> = entries
            .iter()
            .filter_map(|e| {
                // Determine entries within the directory
                let parent_path = e.name.rfind('/').map_or("", |idx| &e.name[..idx]);
                if parent_path == path {
                    // Extract only the file name
                    let file_name = e.name.rfind('/').map_or(&e.name[..], |idx| &e.name[idx + 1..]);
                    Some(DirectoryEntryInternal {
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

        // Sort regular entries by file_id (ascending order)
        regular_entries.sort_by_key(|entry| entry.file_id);

        // Append sorted regular entries to the result
        // (Note: "." and ".." are already at the beginning)
        result_entries.extend(regular_entries);
    
        Ok(result_entries)
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
    shared_data: Arc<SharedCpioData>,   // Reference to shared CPIO data
    data_offset: usize,                 // Offset in the shared data for this file
    data_size: usize,                   // Size of this file's data
    position: RwLock<usize>,            // Current read position
    file_type: FileType,
    directory_entries: Option<Vec<DirectoryEntryInternal>>,
}

impl StreamOps for CpiofsFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        match self.file_type {
            FileType::RegularFile => {
                // Handle regular file reading from shared data
                let mut position = self.position.write();
                let available = self.data_size - *position;
                let to_read = buffer.len().min(available);
                
                if to_read > 0 {
                    let start_offset = self.data_offset + *position;
                    let end_offset = start_offset + to_read;
                    buffer[..to_read].copy_from_slice(&self.shared_data.as_slice()[start_offset..end_offset]);
                    *position += to_read;
                }
                
                Ok(to_read)
            },
            FileType::Directory => {
                // Handle directory reading by streaming directory entries
                if let Some(ref dir_entries) = self.directory_entries {
                    let mut position = self.position.write();
                    
                    // Check if we've reached the end
                    if *position >= dir_entries.len() {
                        return Ok(0); // EOF
                    }
                    
                    // Get the current entry
                    let entry = &dir_entries[*position];
                    
                    // Convert to DirectoryEntry format
                    let dir_entry = DirectoryEntry::from_internal(entry);
                    
                    // Calculate actual entry size
                    let entry_size = dir_entry.entry_size();
                    
                    // Check buffer size
                    if buffer.len() < entry_size {
                        return Err(StreamError::InvalidArgument); // Buffer too small
                    }
                    
                    // Treat struct as byte array
                    let entry_bytes = unsafe {
                        core::slice::from_raw_parts(
                            &dir_entry as *const _ as *const u8,
                            entry_size
                        )
                    };
                    
                    // Copy to buffer
                    buffer[..entry_size].copy_from_slice(entry_bytes);
                    
                    // Move to next entry
                    *position += 1;
                    
                    Ok(entry_size)
                } else {
                    Err(StreamError::from(FileSystemError {
                        kind: FileSystemErrorKind::NotSupported,
                        message: "Cannot read directory from file handle".to_string(),
                    }))
                }
            },
            _ => Err(StreamError::NotSupported),
        }
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::NotSupported)
    }
}

impl FileObject for CpiofsFileObject {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut position = self.position.write();
        
        let max_pos = match self.file_type {
            FileType::RegularFile => self.data_size,
            FileType::Directory => {
                // For directories, position represents entry index
                self.directory_entries.as_ref().map_or(0, |entries| entries.len())
            },
            _ => 0,
        };
        
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
                if offset < 0 && max_pos < offset.abs() as usize {
                    0
                } else if offset < 0 {
                    max_pos - offset.abs() as usize
                } else {
                    max_pos + offset as usize
                }
            },
        };
        
        *position = new_pos;
        Ok(*position as u64)
    }

    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        match self.file_type {
            FileType::RegularFile => {
                Ok(FileMetadata {
                    file_type: FileType::RegularFile,
                    size: self.data_size,
                    permissions: crate::fs::FilePermission {
                        read: true,
                        write: false,
                        execute: false,
                    },
                    created_time: 0,
                    modified_time: 0,
                    accessed_time: 0,
                    file_id: 0, // CPIO file object doesn't know the path, so use 0
                    link_count: 1,
                })
            },
            FileType::Directory => {
                let entry_count = self.directory_entries.as_ref().map_or(0, |entries| entries.len());
                Ok(FileMetadata {
                    file_type: FileType::Directory,
                    size: entry_count, // For directories, size is the number of entries
                    permissions: crate::fs::FilePermission {
                        read: true,
                        write: false,
                        execute: true, // Directories are "executable" for traversal
                    },
                    created_time: 0,
                    modified_time: 0,
                    accessed_time: 0,
                    file_id: 0,
                    link_count: 1,
                })
            },
            _ => Ok(FileMetadata {
                file_type: self.file_type,
                size: 0,
                permissions: crate::fs::FilePermission {
                    read: true,
                    write: false,
                    execute: false,
                },
                created_time: 0,
                modified_time: 0,
                accessed_time: 0,
                file_id: 0,
                link_count: 1,
            }),
        }
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
    
    fn create_from_params(&self, params: &dyn crate::fs::params::FileSystemParams) -> Result<Box<dyn VirtualFileSystem>, FileSystemError> {
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