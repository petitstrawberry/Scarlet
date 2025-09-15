//! ext2 VFS Node Implementation
//!
//! This module implements the VFS node interface for ext2 filesystem nodes,
//! providing file and directory objects that integrate with the VFS v2 architecture.

use alloc::{sync::Weak, string::String, vec::Vec, format};
use spin::{RwLock, Mutex};
use core::{any::Any, fmt::Debug};

use crate::{
    fs::{
        FileObject, FileSystemError, FileSystemErrorKind, FileType, SeekFrom,
        FileMetadata, FilePermission, DeviceFileInfo
    },
    object::capability::{StreamOps, ControlOps, MemoryMappingOps, StreamError},
    DeviceManager
};

use crate::fs::vfs_v2::core::{VfsNode, FileSystemOperations};
use super::{Ext2FileSystem, structures::{EXT2_S_IFMT, EXT2_S_IFREG, EXT2_S_IFDIR}};

/// ext2 VFS Node
///
/// Represents a file or directory in the ext2 filesystem. This node
/// implements the VfsNode trait and provides access to ext2-specific
/// file operations.
#[derive(Debug)]
pub struct Ext2Node {
    /// Inode number in the ext2 filesystem
    inode_number: u32,
    /// File type (directory, regular file, etc.)
    file_type: FileType,
    /// Unique file ID for VFS
    file_id: u64,
    /// Weak reference to the filesystem
    filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
}

impl Ext2Node {
    /// Create a new ext2 node
    pub fn new(inode_number: u32, file_type: FileType, file_id: u64) -> Self {
        Self {
            inode_number,
            file_type,
            file_id,
            filesystem: RwLock::new(None),
        }
    }

    /// Get the inode number
    pub fn inode_number(&self) -> u32 {
        self.inode_number
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }

    /// Get the filesystem reference
    pub fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }
}

impl VfsNode for Ext2Node {
    fn id(&self) -> u64 {
        self.file_id
    }

    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }

    fn file_type(&self) -> Result<FileType, FileSystemError> {
        Ok(self.file_type.clone())
    }

    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        crate::profile_scope!("ext2::node::metadata");
        
        // Read the actual inode to get real metadata
        let filesystem = self.filesystem()
            .and_then(|weak_fs| weak_fs.upgrade())
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Filesystem not available"
            ))?;
        
        let ext2_fs = filesystem.as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid filesystem type"
            ))?;
        
        let inode = ext2_fs.read_inode(self.inode_number)?;
        
        // Convert inode mode to permissions
        let mode = inode.get_mode();
        let permissions = FilePermission {
            read: (mode & 0o444) != 0,
            write: (mode & 0o222) != 0,
            execute: (mode & 0o111) != 0,
        };
        
        Ok(FileMetadata {
            file_type: self.file_type.clone(),
            size: inode.get_size() as usize,
            permissions,
            created_time: inode.get_ctime() as u64,
            modified_time: inode.get_mtime() as u64,
            accessed_time: 0,
            file_id: self.file_id,
            link_count: 1,
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn read_link(&self) -> Result<String, FileSystemError> {
        // Check if this is actually a symbolic link
        if !matches!(self.file_type, FileType::SymbolicLink(_)) {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Not a symbolic link"
            ));
        }

        // Get filesystem reference
        let filesystem = self.filesystem()
            .and_then(|weak_fs| weak_fs.upgrade())
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Filesystem not available"
            ))?;
        
        let ext2_fs = filesystem.as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid filesystem type"
            ))?;
        
        // Read the inode and use the new read_symlink_target method
        let inode = ext2_fs.read_inode(self.inode_number)?;
        inode.read_symlink_target(ext2_fs)
    }
}

/// ext2 File Object
///
/// Handles file operations for regular files in the ext2 filesystem.
#[derive(Debug)]
pub struct Ext2FileObject {
    /// Inode number of the file
    inode_number: u32,
    /// File ID
    file_id: u64,
    /// Current position in the file
    position: Mutex<u64>,
    /// Weak reference to the filesystem
    filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
    /// Cached file content in memory (lazily loaded)
    cached_content: RwLock<Option<Vec<u8>>>,
    /// Whether the cached content has been modified
    is_dirty: RwLock<bool>,
}

impl Ext2FileObject {
    /// Create a new ext2 file object
    pub fn new(inode_number: u32, file_id: u64) -> Self {
        Self {
            inode_number,
            file_id,
            position: Mutex::new(0),
            filesystem: RwLock::new(None),
            cached_content: RwLock::new(None),
            is_dirty: RwLock::new(false),
        }
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }

    /// Get the file ID
    pub fn file_id(&self) -> u64 {
        self.file_id
    }

    /// Load file content from disk into cache if not already loaded
    fn ensure_content_loaded(&self) -> Result<(), StreamError> {
        crate::profile_scope!("ext2::node::ensure_content_loaded");
        
        let mut cached = self.cached_content.write();
        
        // If already loaded, nothing to do
        if cached.is_some() {
            return Ok(());
        }
        
        // Get filesystem reference
        let fs = self.filesystem.read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        
        // Downcast to Ext2FileSystem
        let ext2_fs = fs.as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;
        
        // Read inode to get file size
        let inode = ext2_fs.read_inode(self.inode_number)
            .map_err(|_| StreamError::IoError)?;
        
        // Read entire file content from disk
        let content = if inode.size > 0 {
            ext2_fs.read_file_content(self.inode_number, inode.size as usize)
                .map_err(|_| StreamError::IoError)?
        } else {
            Vec::new()
        };
        
        *cached = Some(content);
        Ok(())
    }

    /// Sync cached content to disk
    fn sync_to_disk(&self) -> Result<(), StreamError> {
        crate::profile_scope!("ext2::node::sync_to_disk");
        
        let is_dirty = *self.is_dirty.read();
        if !is_dirty {
            return Ok(());
        }

        #[cfg(test)]
        crate::early_println!("[ext2] sync_to_disk: Starting sync for inode {}", self.inode_number);

        // Get filesystem reference
        let fs = self.filesystem.read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        
        // Downcast to Ext2FileSystem
        let ext2_fs = fs.as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;
        
        // Get cached content
        let cached = self.cached_content.read();
        let content = cached.as_ref().ok_or(StreamError::IoError)?;
        
        #[cfg(test)]
        crate::early_println!("[ext2] sync_to_disk: Writing {} bytes to inode {}", content.len(), self.inode_number);
        
        // Write content to disk
        ext2_fs.write_file_content(self.inode_number, content)
            .map_err(|_e| {
                #[cfg(test)]
                crate::early_println!("[ext2] sync_to_disk: Error writing to disk: {:?}", _e);
                StreamError::IoError
            })?;
        
        // Mark as clean
        *self.is_dirty.write() = false;
        #[cfg(test)]
        crate::early_println!("[ext2] sync_to_disk: Successfully synced inode {}", self.inode_number);
        Ok(())
    }
}

impl StreamOps for Ext2FileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        // Ensure file content is loaded into cache
        self.ensure_content_loaded()?;
        
        let content = self.cached_content.read();
        let content = content.as_ref().ok_or(StreamError::IoError)?;
        
        let mut position_guard = self.position.lock();
        let current_pos = *position_guard as usize;
        
        // If position is beyond file size, return 0 bytes read
        if current_pos >= content.len() {
            return Ok(0);
        }
        
        // Calculate how many bytes to read
        let bytes_available = content.len() - current_pos;
        let bytes_to_read = core::cmp::min(buffer.len(), bytes_available);
        
        // Copy data from cached content to buffer
        buffer[..bytes_to_read].copy_from_slice(&content[current_pos..current_pos + bytes_to_read]);
        
        // Update position
        *position_guard += bytes_to_read as u64;
        
        Ok(bytes_to_read)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        // Ensure content is loaded into cache
        self.ensure_content_loaded()?;
        
        let pos = *self.position.lock() as usize;
        let mut cached = self.cached_content.write();
        let content = cached.as_mut().ok_or(StreamError::IoError)?;
        
        // Calculate new size
        let new_size = core::cmp::max(content.len(), pos + buffer.len());
        
        // Extend content if needed
        if new_size > content.len() {
            content.resize(new_size, 0);
        }
        
        // Write new data to cached content
        content[pos..pos + buffer.len()].copy_from_slice(buffer);
        
        // Mark as dirty
        *self.is_dirty.write() = true;
        
        // Update position
        {
            let mut position = self.position.lock();
            *position += buffer.len() as u64;
        }
        
        Ok(buffer.len())
    }
}

impl ControlOps for Ext2FileObject {
}

impl MemoryMappingOps for Ext2FileObject {
    fn get_mapping_info(&self, offset: usize, length: usize) -> Result<(usize, usize, bool), &'static str> {
        // Ensure content is loaded into cache
        self.ensure_content_loaded().map_err(|_| "Failed to load file content")?;
        
        let cached = self.cached_content.read();
        let content = cached.as_ref().ok_or("No cached content available")?;
        
        // Check bounds
        if offset >= content.len() {
            return Err("Offset beyond file size");
        }
        
        let available_length = content.len() - offset;
        if length > available_length {
            return Err("Length extends beyond file size");
        }
        
        // Return the virtual address of the cached content as the physical address
        // This is a simplified implementation - in a real OS, this would involve
        // proper virtual-to-physical address translation
        let content_ptr = content.as_ptr() as usize;
        let paddr = content_ptr + offset;
        
        // Return read/write permissions (0x3 = read | write)
        // Not shared between processes (false)
        Ok((paddr, 0x3, false))
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // For a simple implementation, we don't need to track mappings
        // In a more complex system, we might track active mappings here
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // For a simple implementation, we don't need to track unmappings
        // In a more complex system, we might want to sync dirty pages here
        
        // Optionally sync to disk when unmapped
        let _ = self.sync_to_disk();
    }
    
    fn supports_mmap(&self) -> bool {
        true
    }
}

impl FileObject for Ext2FileObject {
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        // Get filesystem reference
        let fs = self.filesystem.read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        
        // Downcast to Ext2FileSystem
        let ext2_fs = fs.as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;
        
        // Read inode metadata
        let inode = ext2_fs.read_inode(self.inode_number)
            .map_err(|_| StreamError::IoError)?;
        
        // Convert inode permissions to FilePermission
        let permissions = FilePermission {
            read: (inode.mode & 0o444) != 0,
            write: (inode.mode & 0o222) != 0,
            execute: (inode.mode & 0o111) != 0,
        };
        
        // Determine file type from inode mode
        let file_type = if (inode.mode & EXT2_S_IFMT) == EXT2_S_IFREG {
            FileType::RegularFile
        } else if (inode.mode & EXT2_S_IFMT) == EXT2_S_IFDIR {
            FileType::Directory
        } else {
            FileType::RegularFile // Default fallback
        };
        
        Ok(FileMetadata {
            file_type,
            size: inode.size as usize,
            permissions,
            created_time: inode.ctime as u64,
            modified_time: inode.mtime as u64,
            accessed_time: inode.atime as u64,
            file_id: self.file_id,
            link_count: inode.links_count as u32,
        })
    }

    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut pos = self.position.lock();
        
        match whence {
            SeekFrom::Start(offset) => {
                *pos = offset;
                Ok(*pos)
            },
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *pos += offset as u64;
                } else {
                    let abs_offset = (-offset) as u64;
                    if abs_offset > *pos {
                        *pos = 0;
                    } else {
                        *pos -= abs_offset;
                    }
                }
                Ok(*pos)
            },
            SeekFrom::End(_offset) => {
                // TODO: Get actual file size from inode
                Err(StreamError::IoError)
            }
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Drop for Ext2FileObject {
    fn drop(&mut self) {
        #[cfg(test)]
        crate::early_println!("[ext2] Drop: syncing inode {} to disk", self.inode_number);
        // Sync to disk when the file object is dropped
        let _ = self.sync_to_disk();
    }
}

/// ext2 Directory Object
///
/// Handles directory operations for directories in the ext2 filesystem.
#[derive(Debug)]
pub struct Ext2DirectoryObject {
    /// Inode number of the directory
    inode_number: u32,
    /// File ID
    file_id: u64,
    /// Current position in directory listing
    position: Mutex<u64>,
    /// Weak reference to the filesystem
    filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
    /// Cached directory entries to avoid re-reading on every access
    cached_entries: Mutex<Option<Vec<crate::fs::DirectoryEntryInternal>>>,
    /// Cache generation (based on directory modification time) to detect stale cache
    cache_generation: Mutex<u32>,
}

impl Ext2DirectoryObject {
    /// Create a new ext2 directory object
    pub fn new(inode_number: u32, file_id: u64) -> Self {
        Self {
            inode_number,
            file_id,
            position: Mutex::new(0),
            filesystem: RwLock::new(None),
            cached_entries: Mutex::new(None),
            cache_generation: Mutex::new(0),
        }
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }

    /// Get cached directory entries or read them if not cached
    fn get_cached_entries(&self) -> Result<Vec<crate::fs::DirectoryEntryInternal>, StreamError> {
        let filesystem = self.filesystem.read()
            .as_ref()
            .and_then(|weak_fs| weak_fs.upgrade())
            .ok_or(StreamError::IoError)?;
        
        let ext2_fs = filesystem.as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::IoError)?;

        // Get current directory inode to check modification time
        let current_inode = match ext2_fs.read_inode(self.inode_number) {
            Ok(inode) => inode,
            Err(_) => return Err(StreamError::IoError),
        };
        
        let current_generation = current_inode.mtime;
        
        // Check if we have cached entries and if they're still valid
        {
            let cached = self.cached_entries.lock();
            let cache_gen = *self.cache_generation.lock();
            if let Some(ref entries) = *cached {
                if cache_gen == current_generation {
                    return Ok(entries.clone());
                }
            }
        }

        // Read directory entries
        let entries = match ext2_fs.read_directory_entries(&current_inode) {
            Ok(entries) => entries,
            Err(_) => return Err(StreamError::IoError),
        };
        
        // Convert to internal directory entries with detailed file type detection
        let mut all_entries = Vec::new();
        
        for entry in entries {
            if entry.entry.inode == 0 {
                continue; // Skip deleted entries
            }
            
            // Detailed file type detection based on ext2 file_type field
            let inode_num = entry.entry.inode; // Copy to avoid alignment issues
            let file_type = match entry.entry.file_type {
                1 => FileType::RegularFile,     // EXT2_FT_REG_FILE
                2 => FileType::Directory,       // EXT2_FT_DIR
                3 => {
                    // EXT2_FT_CHRDEV - Character device
                    // For device files, we need device information
                    // Extract device ID from inode's block array
                    let device_id = match ext2_fs.read_inode(inode_num) {
                        Ok(inode) => {
                            // ext2 stores device ID in block[0] for special files
                            inode.block[0] as usize
                        }
                        Err(_) => 0,
                    };
                    FileType::CharDevice(DeviceFileInfo {
                        device_id,
                        device_type: crate::device::DeviceType::Char,
                    })
                },
                4 => {
                    // EXT2_FT_BLKDEV - Block device
                    // Extract device ID from inode's block array
                    let device_id = match ext2_fs.read_inode(inode_num) {
                        Ok(inode) => {
                            // ext2 stores device ID in block[0] for special files
                            inode.block[0] as usize
                        }
                        Err(_) => 0,
                    };
                    FileType::BlockDevice(DeviceFileInfo {
                        device_id,
                        device_type: crate::device::DeviceType::Block,
                    })
                },
                5 => FileType::Pipe,            // EXT2_FT_FIFO
                6 => FileType::Socket,          // EXT2_FT_SOCK
                7 => {
                    // EXT2_FT_SYMLINK - Symbolic link
                    // Read the actual symlink target from the inode using the new method
                    let target = match ext2_fs.read_inode(inode_num) {
                        Ok(inode) => {
                            inode.read_symlink_target(ext2_fs).unwrap_or_else(|_| {
                                format!("<symlink:{}>", inode_num)
                            })
                        }
                        Err(_) => String::new(),
                    };
                    FileType::SymbolicLink(target)
                },
                _ => FileType::Unknown,         // Unknown file type
            };
            
            all_entries.push(crate::fs::DirectoryEntryInternal {
                name: entry.name,
                file_type,
                size: 0, // Size not immediately available
                file_id: inode_num as u64, // Use copied inode number
                metadata: None,
            });
        }
        
        // Sort entries by file_id for consistent ordering
        all_entries.sort_by_key(|entry| entry.file_id);
        
        // Cache the entries with current generation
        {
            let mut cached = self.cached_entries.lock();
            let mut cache_gen = self.cache_generation.lock();
            *cached = Some(all_entries.clone());
            *cache_gen = current_generation;
        }
        
        Ok(all_entries)
    }
}

impl StreamOps for Ext2DirectoryObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        // Use cached entries to avoid re-reading directory on every call
        let all_entries = self.get_cached_entries()?;
        
        // position is the entry index
        let position = *self.position.lock() as usize;
        
        if position >= all_entries.len() {
            return Ok(0); // EOF
        }
        
        // Get current entry
        let internal_entry = &all_entries[position];
        
        // Convert to binary format
        let dir_entry = crate::fs::DirectoryEntry::from_internal(internal_entry);
        
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
        *self.position.lock() += 1;
        
        Ok(entry_size)
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::IoError)
    }
}

impl ControlOps for Ext2DirectoryObject {
}

impl MemoryMappingOps for Ext2DirectoryObject {
    fn get_mapping_info(&self, _offset: usize, _length: usize) -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for directories")
    }
}

impl FileObject for Ext2DirectoryObject {
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        // Get filesystem reference
        let fs = self.filesystem.read()
            .as_ref()
            .and_then(|weak| weak.upgrade())
            .ok_or(StreamError::Closed)?;
        
        // Downcast to Ext2FileSystem
        let ext2_fs = fs.as_any()
            .downcast_ref::<Ext2FileSystem>()
            .ok_or(StreamError::NotSupported)?;
        
        // Read inode metadata
        let inode = ext2_fs.read_inode(self.inode_number)
            .map_err(|_| StreamError::IoError)?;
        
        // Convert inode permissions to FilePermission
        let permissions = FilePermission {
            read: (inode.mode & 0o444) != 0,
            write: (inode.mode & 0o222) != 0,
            execute: (inode.mode & 0o111) != 0,
        };
        
        Ok(FileMetadata {
            file_type: FileType::Directory,
            size: inode.size as usize,
            permissions,
            created_time: inode.ctime as u64,
            modified_time: inode.mtime as u64,
            accessed_time: inode.atime as u64,
            file_id: self.file_id,
            link_count: inode.links_count as u32,
        })
    }

    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut pos = self.position.lock();
        
        match whence {
            SeekFrom::Start(offset) => {
                *pos = offset;
                Ok(*pos)
            },
            _ => Err(StreamError::IoError)
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// ext2 Character Device File Object
///
/// Handles character device operations through ext2 device files.
#[derive(Debug)]
pub struct Ext2CharDeviceFileObject {
    /// Device file info
    device_info: DeviceFileInfo,
    /// File ID
    file_id: u64,
    /// Current position in the device (for seekable devices)
    position: Mutex<u64>,
    /// Weak reference to the filesystem
    filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
}

impl Ext2CharDeviceFileObject {
    /// Create a new ext2 character device file object
    pub fn new(device_info: DeviceFileInfo, file_id: u64) -> Self {
        Self {
            device_info,
            file_id,
            position: Mutex::new(0),
            filesystem: RwLock::new(None),
        }
    }

    /// Set the filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }
}

impl StreamOps for Ext2CharDeviceFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        #[cfg(test)]
        crate::early_println!("[ext2] CharDevice read: device_id={}", self.device_info.device_id);
        
        // Get the device from device manager
        let device = DeviceManager::get_manager()
            .get_device(self.device_info.device_id)
            .ok_or_else(|| {
                #[cfg(test)]
                crate::early_println!("[ext2] CharDevice: Device with ID {} not found in DeviceManager", self.device_info.device_id);
                StreamError::NotSupported
            })?;

        #[cfg(test)]
        crate::early_println!("[ext2] CharDevice: Found device with ID {}", self.device_info.device_id);

        // Try to cast to CharDevice
        if let Some(char_device) = device.as_char_device() {
            #[cfg(test)]
            crate::early_println!("[ext2] CharDevice: Successfully cast to CharDevice");
            // Use the CharDevice read method
            Ok(char_device.read(buffer))
        } else {
            #[cfg(test)]
            crate::early_println!("[ext2] CharDevice: Device is not a CharDevice");
            Err(StreamError::NotSupported)
        }
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        #[cfg(test)]
        crate::early_println!("[ext2] CharDevice write: device_id={}, buffer_len={}", self.device_info.device_id, buffer.len());
        
        // Get the device from device manager
        let device = DeviceManager::get_manager()
            .get_device(self.device_info.device_id)
            .ok_or_else(|| {
                #[cfg(test)]
                crate::early_println!("[ext2] CharDevice: Device with ID {} not found in DeviceManager", self.device_info.device_id);
                StreamError::NotSupported
            })?;

        #[cfg(test)]
        crate::early_println!("[ext2] CharDevice: Found device with ID {}", self.device_info.device_id);

        // Try to cast to CharDevice
        if let Some(char_device) = device.as_char_device() {
            #[cfg(test)]
            crate::early_println!("[ext2] CharDevice: Successfully cast to CharDevice");
            // Use the CharDevice write method
            char_device.write(buffer).map_err(|_err| {
                #[cfg(test)]
                crate::early_println!("[ext2] CharDevice write error");
                StreamError::IoError
            })
        } else {
            #[cfg(test)]
            crate::early_println!("[ext2] CharDevice: Device is not a CharDevice");
            Err(StreamError::NotSupported)
        }
    }
}

impl ControlOps for Ext2CharDeviceFileObject {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        // Character devices can support control operations
        // For now, return not supported
        let _ = (command, arg);
        Err("Control operation not supported")
    }
}

impl MemoryMappingOps for Ext2CharDeviceFileObject {
    fn get_mapping_info(&self, _offset: usize, _length: usize) -> Result<(usize, usize, bool), &'static str> {
        // Most character devices don't support memory mapping
        Err("Memory mapping not supported")
    }
}

impl FileObject for Ext2CharDeviceFileObject {
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        Ok(FileMetadata {
            file_type: FileType::CharDevice(self.device_info),
            size: 0, // Character devices don't have a meaningful size
            permissions: FilePermission {
                read: true,
                write: true,
                execute: false,
            },
            created_time: 0,
            modified_time: 0,
            accessed_time: 0,
            file_id: self.file_id,
            link_count: 1,
        })
    }

    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        // Get the device to check if it supports seeking
        let device = DeviceManager::get_manager()
            .get_device(self.device_info.device_id)
            .ok_or(StreamError::NotSupported)?;

        if let Some(char_device) = device.as_char_device() {
            if char_device.can_seek() {
                let mut pos = self.position.lock();
                match whence {
                    SeekFrom::Start(offset) => {
                        *pos = offset;
                        Ok(*pos)
                    },
                    SeekFrom::Current(offset) => {
                        if offset >= 0 {
                            *pos = (*pos).saturating_add(offset as u64);
                        } else {
                            *pos = (*pos).saturating_sub((-offset) as u64);
                        }
                        Ok(*pos)
                    },
                    SeekFrom::End(_) => {
                        // Most character devices don't have a meaningful end
                        Err(StreamError::NotSupported)
                    }
                }
            } else {
                Err(StreamError::NotSupported)
            }
        } else {
            Err(StreamError::NotSupported)
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
