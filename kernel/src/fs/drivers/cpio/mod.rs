use alloc::{boxed::Box, format, string::{String, ToString}, vec::Vec};
use spin::Mutex;

use crate::{fs::{
    Directory, DirectoryEntry, FileHandle, FileMetadata, FileOperations, FileSystem, FileSystemDriver, FileSystemError, FileSystemErrorKind, FileSystemType, FileType, Result, VirtualFileSystem
}, vm::vmem::MemoryArea};

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
    id: usize,
    name: &'static str,
    entries: Mutex<Vec<CpiofsEntry>>, // List of entries
    mounted: bool,
    mount_point: String,
}

impl Cpiofs {
    /// Create a new Initramfs
    pub fn new(id: usize, name: &'static str, cpio_data: &[u8]) -> Result<Self> {
        let entries = Self::parse_cpio(cpio_data)?;
        Ok(Self {
            id,
            name,
            entries: Mutex::new(entries),
            mounted: false,
            mount_point: String::new(),
        })
    }

    /// Parse CPIO data to generate entries
    fn parse_cpio(cpio_data: &[u8]) -> Result<Vec<CpiofsEntry>> {
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
                break;
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
    fn mount(&mut self, mount_point: &str) -> Result<()> {
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

    fn unmount(&mut self) -> Result<()> {
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

    fn set_id(&mut self, id: usize) {
        self.id = id;
    }

    fn get_id(&self) -> usize {
        self.id
    }

    fn get_block_size(&self) -> usize {
        512 // Fixed block size
    }

    fn read_block(&mut self, _block_idx: usize, _buffer: &mut [u8]) -> Result<()> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "Initramfs does not support block operations".to_string(),
        })
    }

    fn write_block(&mut self, _block_idx: usize, _buffer: &[u8]) -> Result<()> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::NotSupported,
            message: "Initramfs does not support block operations".to_string(),
        })
    }
}

impl FileOperations for Cpiofs {
    fn open(&self, path: &str, _flags: u32) -> Result<Box<dyn crate::fs::FileHandle>> {
        let path = self.normalize_path(path);
        let entries = self.entries.lock();
        if let Some(entry) = entries.iter().find(|e| e.name == path) {
            Ok(Box::new(CpiofsFileHandle {
                content: entry.data.clone().unwrap_or_default(),
                position: 0,
            }))
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File not found: {}", path),
            })
        }
    }

    fn read_dir(&self, _path: &str) -> Result<Vec<DirectoryEntry>> {
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

    fn create_file(&self, _path: &str) -> Result<()> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::ReadOnly,
            message: "Initramfs is read-only".to_string(),
        })
    }

    fn create_dir(&self, _path: &str) -> Result<()> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::ReadOnly,
            message: "Initramfs is read-only".to_string(),
        })
    }

    fn remove(&self, _path: &str) -> Result<()> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::ReadOnly,
            message: "Initramfs is read-only".to_string(),
        })
    }

    fn metadata(&self, path: &str) -> Result<FileMetadata> {
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
            })
        } else {
            Err(FileSystemError {
                kind: FileSystemErrorKind::NotFound,
                message: format!("File not found: {}", path),
            })
        }
    }
    
    fn root_dir(&self) -> Result<crate::fs::Directory> {
        Ok(Directory::new(self.mount_point.clone() + "/"))
    }
}

struct CpiofsFileHandle {
    content: Vec<u8>,
    position: usize,
}

impl FileHandle for CpiofsFileHandle {
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize> {
        let available = self.content.len() - self.position;
        let to_read = buffer.len().min(available);
        buffer[..to_read].copy_from_slice(&self.content[self.position..self.position + to_read]);
        self.position += to_read;
        Ok(to_read)
    }

    fn write(&mut self, _buffer: &[u8]) -> Result<usize> {
        Err(FileSystemError {
            kind: FileSystemErrorKind::ReadOnly,
            message: "Initramfs is read-only".to_string(),
        })
    }

    fn seek(&mut self, whence: crate::fs::SeekFrom) -> Result<u64> {
        let new_pos = match whence {
            crate::fs::SeekFrom::Start(offset) => offset as usize,
            crate::fs::SeekFrom::Current(offset) => {
                if offset < 0 && self.position < offset.abs() as usize {
                    0
                } else if offset < 0 {
                    self.position - offset.abs() as usize
                } else {
                    self.position + offset as usize
                }
            },
            crate::fs::SeekFrom::End(offset) => {
                let end = self.content.len();
                if offset < 0 && end < offset.abs() as usize {
                    0
                } else if offset < 0 {
                    end - offset.abs() as usize
                } else {
                    end + offset as usize
                }
            },
        };
        
        self.position = new_pos;
        Ok(self.position as u64)
    }

    fn close(&mut self) -> Result<()> {
        Ok(())
    }

    fn metadata(&self) -> Result<FileMetadata> {
        Ok(FileMetadata {
            file_type: FileType::RegularFile,
            size: self.content.len(),
            permissions: crate::fs::FilePermission {
                read: true,
                write: false,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
        })
    }
}

/// Driver for CPIO-format filesystems (initramfs)
/// 
/// This driver creates filesystems from memory areas only.
pub struct CpioDriver;

impl FileSystemDriver for CpioDriver {
    fn name(&self) -> &'static str {
        "cpiofs"
    }
    
    /// This filesystem only supports creation from memory
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Memory
    }
    
    /// Create a file system from memory area
    fn create_from_memory(&self, memory_area: &MemoryArea) -> Result<Box<dyn VirtualFileSystem>> {
        let data = memory_area.as_slice();
        
        // Create the Cpiofs from the memory data
        match Cpiofs::new(0, "cpiofs", data) {
            Ok(cpio_fs) => Ok(Box::new(cpio_fs)),
            Err(err) => Err(FileSystemError {
                kind: FileSystemErrorKind::InvalidData,
                message: format!("Failed to create CPIO filesystem from memory: {}", err.message),
            })
        }
    }
}

#[cfg(test)]
mod tests;