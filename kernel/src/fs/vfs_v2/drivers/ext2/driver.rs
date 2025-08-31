//! EXT2 Filesystem Driver Implementation
//!
//! This module implements the FileSystemDriver trait for EXT2,
//! enabling the filesystem to be registered with the VFS manager
//! and created from block devices.

use alloc::{boxed::Box, sync::Arc, vec};

use crate::{
    device::block::BlockDevice,
    fs::{
        FileSystemDriver, FileSystemError, FileSystemErrorKind, FileSystemType,
        params::FileSystemParams
    }
};

use super::{Ext2FileSystem};
use crate::fs::vfs_v2::core::FileSystemOperations;

/// EXT2 filesystem driver
/// 
/// This driver implements the FileSystemDriver trait and is responsible
/// for creating EXT2 filesystem instances from block devices.
pub struct Ext2Driver;

impl FileSystemDriver for Ext2Driver {
    fn name(&self) -> &'static str {
        "ext2"
    }
    
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Block
    }
    
    fn create(&self) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // EXT2 requires a block device, cannot create without one
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "EXT2 filesystem requires a block device"
        ))
    }
    
    fn create_from_block(
        &self, 
        block_device: Arc<dyn BlockDevice>, 
        _block_size: usize
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // Create EXT2 filesystem from the block device
        let fs = Ext2FileSystem::new(block_device)?;
        Ok(fs as Arc<dyn FileSystemOperations>)
    }
    
    fn create_from_memory(
        &self, 
        _memory_area: &crate::vm::vmem::MemoryArea
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // EXT2 is a block-based filesystem, not memory-based
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "EXT2 filesystem does not support memory-based creation"
        ))
    }
    
    fn create_from_option_string(
        &self, 
        _options: &str
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // EXT2 requires a block device, cannot create from options alone
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "EXT2 filesystem requires a block device, not options"
        ))
    }
    
    fn create_from_params(
        &self, 
        _params: &dyn FileSystemParams
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // For now, EXT2 doesn't support parameter-based creation
        // This could be extended in the future to support formatting options
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "EXT2 filesystem parameter-based creation not implemented"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::block::mockblk::MockBlockDevice;
    
    #[test_case]
    fn test_ext2_driver_type() {
        let driver = Ext2Driver;
        assert_eq!(driver.name(), "ext2");
        assert_eq!(driver.filesystem_type(), FileSystemType::Block);
    }
    
    #[test_case]
    fn test_ext2_create_without_block_device_fails() {
        let driver = Ext2Driver;
        let result = driver.create();
        assert!(result.is_err());
        
        let result = driver.create_from_option_string("");
        assert!(result.is_err());
    }
    
    #[test_case]
    fn test_ext2_create_from_mock_block_device() {
        let driver = Ext2Driver;
        
        // Create a mock block device
        let mock_device = MockBlockDevice::new("mock_ext2", 1024, 8192);
        let result = driver.create_from_block(Arc::new(mock_device), 1024);
        
        // Should succeed in creating filesystem with mock device
        match result {
            Ok(_fs) => {
                // Successfully created filesystem
            },
            Err(e) => {
                // Should not fail with mock device for basic creation
                panic!("EXT2 filesystem creation failed: {:?}", e);
            }
        }
    }
    
    #[test_case]
    fn test_ext2_unsupported_creation_methods() {
        use crate::vm::vmem::MemoryArea;
        
        let driver = Ext2Driver;
        
        // Test memory-based creation
        let memory_area = MemoryArea::new(0x1000, 0x2000, 0x755, false);
        let result = driver.create_from_memory(&memory_area);
        assert!(result.is_err());
        
        // Test params-based creation
        struct MockParams;
        impl FileSystemParams for MockParams {
            fn as_any(&self) -> &dyn core::any::Any { self }
        }
        let params = MockParams;
        let result = driver.create_from_params(&params);
        assert!(result.is_err());
    }
}