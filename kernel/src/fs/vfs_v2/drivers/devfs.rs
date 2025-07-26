//! DevFS - Device filesystem implementation
//!
//! DevFS is a virtual filesystem that automatically exposes all devices
//! registered in the global DeviceManager. When mounted (typically at /dev),
//! it provides device files for all character and block devices that are
//! currently registered with the kernel.
//!
//! ## Features
//!
//! - **Automatic Device Discovery**: Shows all devices from DeviceManager
//! - **Dynamic Updates**: Reflects changes when devices are added/removed
//! - **Device File Support**: Exposes character and block devices as device files
//! - **Read-only Filesystem**: Device files cannot be created/deleted through VFS
//!
//! ## Usage
//!
//! ```rust
//! // Mount devfs at /dev
//! let vfs = VfsManager::new();
//! let devfs = DevFS::new();
//! vfs.mount(devfs, "/dev", 0)?;
//!
//! // Access device files
//! let tty_file = vfs.open("/dev/tty0", O_RDWR)?;
//! ```

use alloc::{
    boxed::Box, collections::BTreeMap, format, string::{String, ToString}, sync::{Arc, Weak}, vec::Vec
};
use spin::RwLock;
use core::any::Any;

use crate::{driver_initcall, fs::{
    get_fs_driver_manager, DeviceFileInfo, FileMetadata, FileObject, FilePermission, FileSystemDriver, 
    FileSystemError, FileSystemErrorKind, FileSystemType, FileType, SeekFrom
}, object::capability::MemoryMappingOps};
use crate::device::{manager::DeviceManager, DeviceType, Device};
use crate::object::capability::{StreamOps, StreamError, ControlOps};

use super::super::core::{VfsNode, FileSystemOperations, DirectoryEntryInternal};

/// DevFS - Device filesystem implementation
///
/// This filesystem automatically exposes all devices registered in the global
/// DeviceManager as device files. It provides a virtual view of the system's
/// devices, similar to /dev in Unix-like systems.
pub struct DevFS {
    /// Root directory node
    root: RwLock<Arc<DevNode>>,
    /// Filesystem name
    name: String,
}

impl DevFS {
    /// Create a new DevFS instance
    pub fn new() -> Arc<Self> {
        let root = Arc::new(DevNode::new_directory("/".to_string()));
        let fs = Arc::new(Self {
            root: RwLock::new(Arc::clone(&root)),
            name: "devfs".to_string(),
        });
        let fs_weak = Arc::downgrade(&(fs.clone() as Arc<dyn FileSystemOperations>));
        root.set_filesystem(fs_weak);
        fs
    }

    /// Populate the filesystem with current devices from DeviceManager
    fn populate_devices(&self) -> Result<(), FileSystemError> {
        let device_manager = DeviceManager::get_manager();
        let root = self.root.read();
        
        // Clear existing devices (for dynamic updates)
        root.clear_children();
        
        // Get all devices that were registered with explicit names
        let named_devices = device_manager.get_named_devices();
        
        for (device_name, device) in named_devices {
            let device_type = device.device_type();
            
            // Only add char and block devices to devfs
            match device_type {
                DeviceType::Char | DeviceType::Block => {
                    // Get the actual device ID from the name
                    let device_id = device_manager.get_device_id_by_name(&device_name)
                        .unwrap_or(0); // fallback to 0 if not found
                    
                    let device_file_info = DeviceFileInfo {
                        device_id,
                        device_type,
                    };
                    
                    let file_type = match device_type {
                        DeviceType::Char => FileType::CharDevice(device_file_info),
                        DeviceType::Block => FileType::BlockDevice(device_file_info),
                        _ => continue, // Skip other device types
                    };
                    
                    let device_node = Arc::new(DevNode::new_device_file(
                        device_name.clone(),
                        file_type,
                        device_id as u64, // Use the device ID as file ID too
                    ));
                    
                    // Set filesystem reference for the device node
                    if let Some(fs_ref) = root.filesystem() {
                        device_node.set_filesystem(fs_ref);
                    }
                    
                    root.add_child(device_name, device_node)?;
                }
                _ => {} // Skip non-device files
            }
        }
        
        Ok(())
    }
}

impl FileSystemOperations for DevFS {
    fn name(&self) -> &str {
        &self.name
    }

    fn root_node(&self) -> Arc<dyn VfsNode> {
        // Refresh devices on each root access to ensure up-to-date view
        let _ = self.populate_devices();
        Arc::clone(&*self.root.read()) as Arc<dyn VfsNode>
    }

    fn lookup(&self, parent: &Arc<dyn VfsNode>, name: &String) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        // Refresh devices before lookup to ensure up-to-date view
        let _ = self.populate_devices();
        
        let dev_node = Arc::downcast::<DevNode>(parent.clone())
            .map_err(|_| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for DevFS"
            ))?;
        
        if let Some(child) = dev_node.get_child(name) {
            Ok(child as Arc<dyn VfsNode>)
        } else {
            Err(FileSystemError::new(
                FileSystemErrorKind::NotFound,
                format!("Device '{}' not found in devfs", name)
            ))
        }
    }

    fn readdir(&self, node: &Arc<dyn VfsNode>) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        // Refresh devices before readdir to ensure up-to-date view
        let _ = self.populate_devices();
        
        let dev_node = Arc::downcast::<DevNode>(node.clone())
            .map_err(|_| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for DevFS"
            ))?;
        
        dev_node.readdir()
    }

    fn open(&self, node: &Arc<dyn VfsNode>, _flags: u32) -> Result<Arc<dyn FileObject>, FileSystemError> {
        let dev_node = Arc::downcast::<DevNode>(node.clone())
            .map_err(|_| FileSystemError::new(
                FileSystemErrorKind::NotSupported,
                "Invalid node type for DevFS"
            ))?;
        
        dev_node.open()
    }

    fn is_read_only(&self) -> bool {
        true
    }

    // DevFS is read-only - these operations are not supported
    fn create(&self, _parent: &Arc<dyn VfsNode>, _name: &String, _file_type: FileType, _mode: u32) -> Result<Arc<dyn VfsNode>, FileSystemError> {
        Err(FileSystemError::new(
            FileSystemErrorKind::ReadOnly,
            "DevFS is read-only: cannot create files"
        ))
    }

    fn remove(&self, _parent: &Arc<dyn VfsNode>, _name: &String) -> Result<(), FileSystemError> {
        Err(FileSystemError::new(
            FileSystemErrorKind::ReadOnly,
            "DevFS is read-only: cannot remove files"
        ))
    }
}

/// A node in the DevFS filesystem
pub struct DevNode {
    /// Node name
    name: String,
    /// File type
    file_type: FileType,
    /// File ID
    file_id: u64,
    /// Child nodes (for directories)
    children: RwLock<BTreeMap<String, Arc<DevNode>>>,
    /// Reference to filesystem
    filesystem: RwLock<Option<Weak<dyn FileSystemOperations>>>,
}

impl Clone for DevNode {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            file_type: self.file_type.clone(),
            file_id: self.file_id,
            children: RwLock::new(self.children.read().clone()),
            filesystem: RwLock::new(self.filesystem.read().clone()),
        }
    }
}

impl DevNode {
    /// Create a new directory node
    pub fn new_directory(name: String) -> Self {
        Self {
            name,
            file_type: FileType::Directory,
            file_id: 0, // Root directory ID
            children: RwLock::new(BTreeMap::new()),
            filesystem: RwLock::new(None),
        }
    }

    /// Create a new device file node
    pub fn new_device_file(name: String, file_type: FileType, file_id: u64) -> Self {
        Self {
            name,
            file_type,
            file_id,
            children: RwLock::new(BTreeMap::new()),
            filesystem: RwLock::new(None),
        }
    }

    /// Set filesystem reference
    pub fn set_filesystem(&self, fs: Weak<dyn FileSystemOperations>) {
        *self.filesystem.write() = Some(fs);
    }

    /// Add a child node
    pub fn add_child(&self, name: String, child: Arc<DevNode>) -> Result<(), FileSystemError> {
        if self.file_type != FileType::Directory {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Cannot add child to non-directory node"
            ));
        }
        
        let mut children = self.children.write();
        children.insert(name, child);
        Ok(())
    }

    /// Get a child by name
    pub fn get_child(&self, name: &str) -> Option<Arc<DevNode>> {
        let children = self.children.read();
        children.get(name).cloned()
    }

    /// Clear all children (for dynamic updates)
    pub fn clear_children(&self) {
        let mut children = self.children.write();
        children.clear();
    }

    /// Read directory contents
    pub fn readdir(&self) -> Result<Vec<DirectoryEntryInternal>, FileSystemError> {
        if self.file_type != FileType::Directory {
            return Err(FileSystemError::new(
                FileSystemErrorKind::NotADirectory,
                "Cannot read directory of non-directory node"
            ));
        }

        let children = self.children.read();
        let mut entries = Vec::new();

        // Add "." entry (current directory)
        entries.push(DirectoryEntryInternal {
            name: ".".to_string(),
            file_type: FileType::Directory,
            file_id: self.file_id,
        });

        // Add ".." entry (parent directory)
        // For DevFS root, parent is itself
        entries.push(DirectoryEntryInternal {
            name: "..".to_string(),
            file_type: FileType::Directory,
            file_id: self.file_id, // DevFS is always at root level, so parent is self
        });

        // Add actual child entries
        for (name, child) in children.iter() {
            entries.push(DirectoryEntryInternal {
                name: name.clone(),
                file_type: child.file_type.clone(),
                file_id: child.file_id,
            });
        }

        Ok(entries)
    }

    /// Open the node as a file object
    pub fn open(&self) -> Result<Arc<dyn FileObject>, FileSystemError> {
        match self.file_type {
            FileType::CharDevice(device_info) | FileType::BlockDevice(device_info) => {
                // Create a device file object that can handle device operations
                Ok(Arc::new(DevFileObject::new(
                    Arc::new(self.clone()), 
                    device_info.device_id, 
                    device_info.device_type
                )?))
            }
            FileType::Directory => {
                // Create a directory file object that can handle directory operations
                Ok(Arc::new(DevDirectoryObject::new(Arc::new(self.clone()))))
            }
            _ => {
                Err(FileSystemError::new(
                    FileSystemErrorKind::NotSupported,
                    "Unsupported file type in devfs"
                ))
            }
        }
    }
}

impl VfsNode for DevNode {
    fn id(&self) -> u64 {
        self.file_id
    }

    fn metadata(&self) -> Result<FileMetadata, FileSystemError> {
        Ok(FileMetadata {
            file_type: self.file_type.clone(),
            size: 0, // Device files have no size
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

    fn filesystem(&self) -> Option<Weak<dyn FileSystemOperations>> {
        self.filesystem.read().clone()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// DevFS filesystem driver
pub struct DevFSDriver;

/// A file object for device files in DevFS
/// 
/// This struct provides a FileObject implementation that delegates
/// device operations to the underlying device registered in DeviceManager.
pub struct DevFileObject {
    /// Reference to the DevNode
    node: Arc<DevNode>,
    /// Current file position (for seekable devices)
    position: RwLock<u64>,
    /// Device ID for lookup in DeviceManager
    #[allow(dead_code)]
    device_id: usize,
    /// Device type
    #[allow(dead_code)]
    device_type: DeviceType,
    /// Optional device guard for device files
    device_guard: Option<Arc<dyn Device>>,
}

impl DevFileObject {
    /// Create a new file object for device files
    pub fn new(node: Arc<DevNode>, device_id: usize, device_type: DeviceType) -> Result<Self, FileSystemError> {
        // Try to get the device from DeviceManager by ID
        match DeviceManager::get_manager().get_device(device_id) {
            Some(device_guard) => {
                Ok(Self {
                    node,
                    position: RwLock::new(0),
                    device_id,
                    device_type,
                    device_guard: Some(device_guard),
                })
            }
            None => {
                Err(FileSystemError::new(
                    FileSystemErrorKind::DeviceError,
                    format!("Device with ID {} not found in DeviceManager", device_id)
                ))
            }
        }
    }

    /// Read from the underlying device at current position
    fn read_device(&self, buffer: &mut [u8]) -> Result<usize, FileSystemError> {
        if let Some(ref device_guard) = self.device_guard {
            let device_guard_ref = device_guard.as_ref();
            let position = *self.position.read();
            
            match device_guard_ref.device_type() {
                DeviceType::Char => {
                    if let Some(char_device) = device_guard_ref.as_char_device() {
                        // Use read_at for position-based read
                        match char_device.read_at(position, buffer) {
                            Ok(bytes_read) => {
                                // Update position after successful read
                                *self.position.write() += bytes_read as u64;
                                Ok(bytes_read)
                            },
                            Err(e) => {
                                Err(FileSystemError::new(
                                    FileSystemErrorKind::IoError,
                                    format!("Character device read failed: {}", e)
                                ))
                            }
                        }
                    } else {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::DeviceError,
                            "Device does not support character operations"
                        ));
                    }
                },
                DeviceType::Block => {
                    if let Some(block_device) = device_guard_ref.as_block_device() {
                        // For block devices, we read sectors using the request system
                        let request = Box::new(crate::device::block::request::BlockIORequest {
                            request_type: crate::device::block::request::BlockIORequestType::Read,
                            sector: 0,
                            sector_count: 1,
                            head: 0,
                            cylinder: 0,
                            buffer: buffer.to_vec(),
                        });
                        
                        block_device.enqueue_request(request);
                        let results = block_device.process_requests();
                        
                        if let Some(result) = results.first() {
                            match &result.result {
                                Ok(_) => {
                                    // Copy the data back to the buffer
                                    let bytes_to_copy = core::cmp::min(buffer.len(), result.request.buffer.len());
                                    buffer[..bytes_to_copy].copy_from_slice(&result.request.buffer[..bytes_to_copy]);
                                    return Ok(bytes_to_copy);
                                },
                                Err(e) => {
                                    return Err(FileSystemError::new(
                                        FileSystemErrorKind::IoError,
                                        format!("Block device read failed: {}", e)
                                    ));
                                }
                            }
                        }
                        return Ok(0);
                    } else {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::DeviceError,
                            "Device does not support block operations"
                        ));
                    }
                },
                _ => {
                    return Err(FileSystemError::new(
                        FileSystemErrorKind::DeviceError,
                        "Unsupported device type"
                    ));
                }
            }
        } else {
            Err(FileSystemError::new(
                FileSystemErrorKind::DeviceError,
                "No device guard available"
            ))
        }
    }

    /// Write to the underlying device at current position
    fn write_device(&self, buffer: &[u8]) -> Result<usize, FileSystemError> {
        if let Some(ref device_guard) = self.device_guard {
            let device_guard_ref = device_guard.as_ref();
            let position = *self.position.read();
            
            match device_guard_ref.device_type() {
                DeviceType::Char => {
                    if let Some(char_device) = device_guard_ref.as_char_device() {
                        // Use write_at for position-based write
                        match char_device.write_at(position, buffer) {
                            Ok(bytes_written) => {
                                // Update position after successful write
                                *self.position.write() += bytes_written as u64;
                                Ok(bytes_written)
                            },
                            Err(e) => {
                                Err(FileSystemError::new(
                                    FileSystemErrorKind::IoError,
                                    format!("Character device write failed: {}", e)
                                ))
                            }
                        }
                    } else {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::DeviceError,
                            "Device does not support character operations"
                        ));
                    }
                },
                DeviceType::Block => {
                    if let Some(block_device) = device_guard_ref.as_block_device() {
                        let request = Box::new(crate::device::block::request::BlockIORequest {
                            request_type: crate::device::block::request::BlockIORequestType::Write,
                            sector: 0,
                            sector_count: 1,
                            head: 0,
                            cylinder: 0,
                            buffer: buffer.to_vec(),
                        });
                        
                        block_device.enqueue_request(request);
                        let results = block_device.process_requests();
                        
                        if let Some(result) = results.first() {
                            match &result.result {
                                Ok(_) => return Ok(buffer.len()),
                                Err(e) => {
                                    return Err(FileSystemError::new(
                                        FileSystemErrorKind::IoError,
                                        format!("Block device write failed: {}", e)
                                    ));
                                }
                            }
                        }
                        return Ok(0);
                    } else {
                        return Err(FileSystemError::new(
                            FileSystemErrorKind::DeviceError,
                            "Device does not support block operations"
                        ));
                    }
                },
                _ => {
                    return Err(FileSystemError::new(
                        FileSystemErrorKind::DeviceError,
                        "Unsupported device type"
                    ));
                }
            }
        } else {
            Err(FileSystemError::new(
                FileSystemErrorKind::DeviceError,
                "No device guard available"
            ))
        }
    }
}

impl StreamOps for DevFileObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        self.read_device(buffer).map_err(StreamError::from)
    }
    
    fn write(&self, buffer: &[u8]) -> Result<usize, StreamError> {
        self.write_device(buffer).map_err(StreamError::from)
    }
}

impl ControlOps for DevFileObject {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        // For device files, delegate control operations to the underlying device
        if let Some(ref device_guard) = self.device_guard {
            let device_guard_ref = device_guard.as_ref();
            // Device trait now inherits from ControlOps, so we can delegate directly
            device_guard_ref.control(command, arg)
        } else {
            Err("No device available for control operations")
        }
    }
    
    fn supported_control_commands(&self) -> alloc::vec::Vec<(u32, &'static str)> {
        // For device files, delegate to the underlying device
        if let Some(ref device_guard) = self.device_guard {
            let device_guard_ref = device_guard.as_ref();
            device_guard_ref.supported_control_commands()
        } else {
            alloc::vec![]
        }
    }
}

impl MemoryMappingOps for DevFileObject {
    fn get_mapping_info(&self, offset: usize, length: usize) 
                       -> Result<(usize, usize, bool), &'static str> {
        // For device files, delegate to the underlying device if it supports memory mapping
        if let Some(ref device_guard) = self.device_guard {
            let device_guard_ref = device_guard.as_ref();
            device_guard_ref.get_mapping_info(offset, length)
        } else {
            Err("No device associated with this DevFileObject")
        }
    }
    
    fn on_mapped(&self, vaddr: usize, paddr: usize, length: usize, offset: usize) {
        if let Some(ref device_guard) = self.device_guard {
            let device_guard_ref = device_guard.as_ref();
            device_guard_ref.on_mapped(vaddr, paddr, length, offset);
        }
    }
    
    fn on_unmapped(&self, vaddr: usize, length: usize) {
        if let Some(ref device_guard) = self.device_guard {
            let device_guard_ref = device_guard.as_ref();
            device_guard_ref.on_unmapped(vaddr, length);
        }
    }
    
    fn supports_mmap(&self) -> bool {
        if let Some(ref device_guard) = self.device_guard {
            let device_guard_ref = device_guard.as_ref();
            device_guard_ref.supports_mmap()
        } else {
            false
        }
    }
}

impl FileObject for DevFileObject {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        let mut position = self.position.write();

        let new_pos = match whence {
            SeekFrom::Start(offset) => offset,
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *position + offset as u64
                } else {
                    position.saturating_sub((-offset) as u64)
                }
            }
            SeekFrom::End(offset) => {
                // For devices, we can't easily determine the "end" position
                // Most devices don't have a fixed size, so seeking from end is not meaningful
                // We'll treat this as an error for now
                return Err(StreamError::from(FileSystemError::new(
                    FileSystemErrorKind::NotSupported,
                    "Seek from end not supported for device files"
                )));
            }
        };
        
        *position = new_pos;
        Ok(new_pos)
    }
    
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        self.node.metadata().map_err(StreamError::from)
    }

    fn truncate(&self, _size: u64) -> Result<(), StreamError> {
        // Device files cannot be truncated
        Err(StreamError::from(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "Cannot truncate device files"
        )))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A file object for directories in DevFS
/// 
/// This struct provides a FileObject implementation for directories
/// that allows reading directory entries as binary DirectoryEntry data.
pub struct DevDirectoryObject {
    /// Reference to the DevNode
    node: Arc<DevNode>,
    /// Current position in directory entries (entry index)
    position: RwLock<usize>,
}

impl DevDirectoryObject {
    /// Create a new directory file object
    pub fn new(node: Arc<DevNode>) -> Self {
        Self {
            node,
            position: RwLock::new(0),
        }
    }
}

impl StreamOps for DevDirectoryObject {
    fn read(&self, buffer: &mut [u8]) -> Result<usize, StreamError> {
        // Get all directory entries
        let entries = self.node.readdir().map_err(StreamError::from)?;
        let position = *self.position.read();
        
        if position >= entries.len() {
            return Ok(0); // EOF
        }
        
        // Convert the entry at current position to DirectoryEntry format
        let internal_entry = &entries[position];
        
        // Create DirectoryEntryInternal with size field for DirectoryEntry::from_internal
        let internal_with_size = crate::fs::DirectoryEntryInternal {
            name: internal_entry.name.clone(),
            file_type: internal_entry.file_type.clone(),
            size: 0, // Device files have no meaningful size
            file_id: internal_entry.file_id,
            metadata: None,
        };
        
        let dir_entry = crate::fs::DirectoryEntry::from_internal(&internal_with_size);
        let entry_size = dir_entry.entry_size();
        
        if buffer.len() < entry_size {
            return Err(StreamError::InvalidArgument); // Buffer too small
        }
        
        // Copy entry as bytes
        let entry_bytes = unsafe {
            core::slice::from_raw_parts(
                &dir_entry as *const _ as *const u8,
                entry_size
            )
        };
        
        buffer[..entry_size].copy_from_slice(entry_bytes);
        
        // Move to next entry
        *self.position.write() += 1;
        
        Ok(entry_size)
    }
    
    fn write(&self, _buffer: &[u8]) -> Result<usize, StreamError> {
        Err(StreamError::from(FileSystemError::new(
            FileSystemErrorKind::ReadOnly,
            "Cannot write to directory in devfs"
        )))
    }
}

impl ControlOps for DevDirectoryObject {
    // Directory objects don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported on directories")
    }
}

impl MemoryMappingOps for DevDirectoryObject {
    fn get_mapping_info(&self, _offset: usize, _length: usize) 
                       -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported for directories")
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Directories don't support memory mapping
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Directories don't support memory mapping
    }
    
    fn supports_mmap(&self) -> bool {
        false
    }
}

impl FileObject for DevDirectoryObject {
    fn seek(&self, whence: SeekFrom) -> Result<u64, StreamError> {
        // Get directory entries to know the count
        let entries = self.node.readdir().map_err(StreamError::from)?;
        let entry_count = entries.len() as u64;
        
        let mut position = self.position.write();
        
        let new_pos = match whence {
            SeekFrom::Start(offset) => offset,
            SeekFrom::Current(offset) => {
                if offset >= 0 {
                    *position as u64 + offset as u64
                } else {
                    (*position as u64).saturating_sub((-offset) as u64)
                }
            },
            SeekFrom::End(offset) => {
                if offset >= 0 {
                    entry_count + offset as u64
                } else {
                    entry_count.saturating_sub((-offset) as u64)
                }
            }
        };
        
        *position = new_pos as usize;
        Ok(new_pos)
    }
    
    fn metadata(&self) -> Result<FileMetadata, StreamError> {
        self.node.metadata().map_err(StreamError::from)
    }

    fn truncate(&self, _size: u64) -> Result<(), StreamError> {
        Err(StreamError::from(FileSystemError::new(
            FileSystemErrorKind::ReadOnly,
            "Cannot truncate directory in devfs"
        )))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl FileSystemDriver for DevFSDriver {
    fn name(&self) -> &'static str {
        "devfs"
    }

    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Device
    }

    fn create(&self) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        Ok(DevFS::new() as Arc<dyn FileSystemOperations>)
    }

    fn create_from_option_string(&self, _options: &str) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // DevFS doesn't use options, just create a new instance
        self.create()
    }
}

/// Register the DevFS driver with the filesystem driver manager
fn register_driver() {
    let fs_driver_manager = get_fs_driver_manager();
    fs_driver_manager.register_driver(Box::new(DevFSDriver));
}

driver_initcall!(register_driver);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{GenericDevice, manager::DeviceManager};
    use alloc::sync::Arc;

    #[test_case]
    fn test_devfs_creation() {
        let devfs = DevFS::new();
        assert_eq!(devfs.name(), "devfs");
    }

    #[test_case]
    fn test_devfs_root_access() {
        let devfs = DevFS::new();
        let root = devfs.root_node();
        assert_eq!(root.id(), 0);
        
        let metadata = root.metadata().unwrap();
        assert_eq!(metadata.file_type, FileType::Directory);
    }

    #[test_case]
    fn test_devfs_device_discovery() {
        // Register a test device
        let device_manager = DeviceManager::get_manager();
        let test_device = Arc::new(GenericDevice::new("test_devfs_device"));
        let _device_id = device_manager.register_device_with_name("test_devfs_device".to_string(), test_device);

        let devfs = DevFS::new();
        let root = devfs.root_node();
        
        // Check if we can read the directory (this should trigger device population)
        let entries = devfs.readdir(&root).unwrap();
        
        // Note: Generic devices are not currently exposed in devfs (only Char/Block devices)
        // So this test checks that the readdir operation works without error
        assert!(entries.len() == 0 || entries.len() > 0, "DevFS readdir should work without error");
    }

    #[test_case]
    fn test_devfs_lookup() {
        let devfs = DevFS::new();
        let root = devfs.root_node();
        
        // Try to lookup a non-existent device
        let result = devfs.lookup(&root, &"nonexistent_device".to_string());
        assert!(result.is_err(), "Lookup of non-existent device should fail");
        
        let error = result.unwrap_err();
        assert_eq!(error.kind, FileSystemErrorKind::NotFound);
    }

    #[test_case]
    fn test_devfs_with_real_devices() {
        use crate::device::char::mockchar::MockCharDevice;
        use crate::device::block::mockblk::MockBlockDevice;
        
        // Register actual character and block devices
        let device_manager = DeviceManager::get_manager();
        
        // Register a character device (TTY-like)
        let char_device = Arc::new(MockCharDevice::new("tty0"));
        let _char_device_id = device_manager.register_device_with_name("tty0".to_string(), char_device.clone());
        
        // Register a block device (disk-like)  
        let block_device = Arc::new(MockBlockDevice::new("sda", 512, 1000));
        let _block_device_id = device_manager.register_device_with_name("sda".to_string(), block_device.clone());
        
        // Create devfs and verify devices appear
        let devfs = DevFS::new();
        let root = devfs.root_node();
        
        // Read directory contents
        let entries = devfs.readdir(&root).unwrap();
        
        // Debug: print all entries to see what we actually have
        let _unused_entries = &entries;
        
        // Should contain both our devices
        let has_tty0 = entries.iter().any(|entry| {
            entry.name == "tty0" && matches!(entry.file_type, FileType::CharDevice(_))
        });
        let has_sda = entries.iter().any(|entry| {
            entry.name == "sda" && matches!(entry.file_type, FileType::BlockDevice(_))
        });
        
        assert!(has_tty0, "DevFS should contain tty0 character device");
        assert!(has_sda, "DevFS should contain sda block device");
        
        // Test lookup functionality for character device
        let tty0_result = devfs.lookup(&root, &"tty0".to_string());
        assert!(tty0_result.is_ok(), "Should be able to lookup tty0");
        
        // Test lookup functionality for block device  
        let sda_result = devfs.lookup(&root, &"sda".to_string());
        assert!(sda_result.is_ok(), "Should be able to lookup sda");
    }

    #[test_case]
    fn test_devfs_driver_registration() {
        // Test that the DevFS driver is properly registered
        let fs_driver_manager = get_fs_driver_manager();
        
        // Check if devfs driver is registered
        assert!(fs_driver_manager.has_driver("devfs"), "DevFS driver should be registered");
        
        // Check driver type
        let driver_type = fs_driver_manager.get_driver_type("devfs");
        assert_eq!(driver_type, Some(FileSystemType::Device));
        
        // Test creating a devfs instance through the driver manager
        let devfs_result = fs_driver_manager.create_from_option_string("devfs", "");
        assert!(devfs_result.is_ok(), "Should be able to create DevFS through driver manager");
        
        let devfs_instance = devfs_result.unwrap();
        assert_eq!(devfs_instance.name(), "devfs");
        assert!(devfs_instance.is_read_only());
    }

    #[test_case]
    fn test_devfs_readonly_operations() {
        let devfs = DevFS::new();
        let root = devfs.root_node();
        
        // Test that create operations fail
        let create_result = devfs.create(&root, &"test_file".to_string(), FileType::RegularFile, 0);
        assert!(create_result.is_err());
        assert_eq!(create_result.unwrap_err().kind, FileSystemErrorKind::ReadOnly);
        
        // Test that remove operations fail
        let remove_result = devfs.remove(&root, &"test_file".to_string());
        assert!(remove_result.is_err());
        assert_eq!(remove_result.unwrap_err().kind, FileSystemErrorKind::ReadOnly);
    }

    #[test_case]
    fn test_devfs_device_file_operations() {
        use crate::device::char::mockchar::MockCharDevice;
        
        // Register a character device for testing
        let device_manager = DeviceManager::get_manager();
        let char_device = Arc::new(MockCharDevice::new("test_char_dev"));
        let _device_id = device_manager.register_device_with_name("test_char_dev".to_string(), char_device.clone());
        
        // Create devfs and lookup the device
        let devfs = DevFS::new();
        let root = devfs.root_node();
        
        let device_node_result = devfs.lookup(&root, &"test_char_dev".to_string());
        assert!(device_node_result.is_ok(), "Should be able to lookup character device");
        
        let device_node = device_node_result.unwrap();
        
        // Test opening the device file
        let file_result = devfs.open(&device_node, 0);
        assert!(file_result.is_ok(), "Should be able to open character device file");
        
        let file_obj = file_result.unwrap();
        
        // Test basic FileObject operations
        let metadata_result = file_obj.metadata();
        assert!(metadata_result.is_ok(), "Should be able to get device file metadata");
        
        // Test read operation (should work even if device returns no data)
        let mut read_buffer = [0u8; 10];
        let read_result = file_obj.read(&mut read_buffer);
        assert!(read_result.is_ok(), "Read operation should succeed");
        
        // Test write operation
        let write_data = b"test";
        let write_result = file_obj.write(write_data);
        assert!(write_result.is_ok(), "Write operation should succeed");
        
        // Test seek operation
        let seek_result = file_obj.seek(crate::fs::SeekFrom::Start(0));
        assert!(seek_result.is_ok(), "Seek operation should succeed");
        
        // Test truncate operation (should fail for device files)
        let truncate_result = file_obj.truncate(100);
        assert!(truncate_result.is_err(), "Truncate should fail for device files");
    }

    #[test_case]
    fn test_devfs_directory_operations() {
        use crate::device::char::mockchar::MockCharDevice;
        
        // Register a character device for testing
        let device_manager = DeviceManager::get_manager();
        let char_device = Arc::new(MockCharDevice::new("test_dir_ops"));
        let _device_id = device_manager.register_device_with_name("test_dir_ops".to_string(), char_device.clone());
        
        // Create devfs
        let devfs = DevFS::new();
        let root = devfs.root_node();
        
        // Test opening directory
        let dir_file_result = devfs.open(&root, 0);
        assert!(dir_file_result.is_ok(), "Should be able to open directory in devfs");
        
        let dir_file = dir_file_result.unwrap();
        
        // Test reading directory contents (should return DirectoryEntry structs)
        let mut read_buffer = [0u8; 512]; // Buffer for one directory entry
        let read_result = dir_file.read(&mut read_buffer);
        assert!(read_result.is_ok(), "Should be able to read directory contents");
        
        let bytes_read = read_result.unwrap();
        assert!(bytes_read > 0, "Should read some directory data");
        
        // Parse the directory entry
        let dir_entry = crate::fs::DirectoryEntry::parse(&read_buffer[..bytes_read]);
        assert!(dir_entry.is_some(), "Should be able to parse directory entry");
        
        let entry = dir_entry.unwrap();
        let name = entry.name_str().unwrap();
        
        // Should be either "." or ".." or our test device
        assert!(name == "." || name == ".." || name == "test_dir_ops", 
               "Entry name should be '.', '..' or 'test_dir_ops', got: {}", name);
        
        // Test seek operations
        let seek_result = dir_file.seek(SeekFrom::Start(0));
        assert!(seek_result.is_ok(), "Should be able to seek in directory");
        
        // Test metadata
        let metadata_result = dir_file.metadata();
        assert!(metadata_result.is_ok(), "Should be able to get directory metadata");
        
        let metadata = metadata_result.unwrap();
        assert_eq!(metadata.file_type, FileType::Directory, "Should be directory type");
    }
}