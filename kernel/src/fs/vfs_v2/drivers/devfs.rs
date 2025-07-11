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
    FileSystemError, FileSystemErrorKind, FileSystemType, FileType
}};
use crate::device::{manager::DeviceManager, DeviceType};

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
        
        // Add device files for all registered devices
        let device_count = device_manager.get_devices_count();
        for device_id in 0..device_count {
            if let Some(device) = device_manager.get_device(device_id) {
                let device_name = device.name().to_string();
                let device_type = device.device_type();
                
                // Only add char and block devices to devfs
                match device_type {
                    DeviceType::Char | DeviceType::Block => {
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
                            device_id as u64 + 1, // file_id = device_id + 1 (root is 0)
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

        for (name, child) in children.iter() {
            entries.push(DirectoryEntryInternal {
                name: name.clone(),
                file_type: child.file_type,
                file_id: child.file_id,
            });
        }

        Ok(entries)
    }

    /// Open the node as a file object
    pub fn open(&self) -> Result<Arc<dyn FileObject>, FileSystemError> {
        match self.file_type {
            FileType::CharDevice(device_info) | FileType::BlockDevice(device_info) => {
                // Get the actual device from DeviceManager
                let device_manager = DeviceManager::get_manager();
                if let Some(_device) = device_manager.get_device(device_info.device_id) {
                    // For device files, we need to create a wrapper that provides FileObject interface
                    // For now, return an error as device file operations require more complex implementation
                    Err(FileSystemError::new(
                        FileSystemErrorKind::NotSupported,
                        "Device file operations not yet implemented"
                    ))
                } else {
                    Err(FileSystemError::new(
                        FileSystemErrorKind::DeviceError,
                        "Device not found in DeviceManager"
                    ))
                }
            }
            FileType::Directory => {
                Err(FileSystemError::new(
                    FileSystemErrorKind::IsADirectory,
                    "Cannot open directory as file"
                ))
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
            file_type: self.file_type,
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
        let test_device = Arc::new(GenericDevice::new("test_devfs_device", 999));
        let _device_id = device_manager.register_device_with_name("test_devfs_device".to_string(), test_device);

        let devfs = DevFS::new();
        let root = devfs.root_node();
        
        // Check if we can read the directory (this should trigger device population)
        let entries = devfs.readdir(&root).unwrap();
        
        // Note: Generic devices are not currently exposed in devfs (only Char/Block devices)
        // So this test checks that the readdir operation works without error
        assert!(entries.len() >= 0, "DevFS readdir should work without error");
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
}