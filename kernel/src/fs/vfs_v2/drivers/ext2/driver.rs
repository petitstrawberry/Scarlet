//! ext2 Filesystem Driver Implementation
//!
//! This module implements the FileSystemDriver trait for ext2,
//! enabling the filesystem to be registered with the VFS manager
//! and created from block devices.

use alloc::sync::Arc;

use crate::{
    device::block::BlockDevice,
    fs::{
        FileSystemDriver, FileSystemError, FileSystemErrorKind, FileSystemType,
        params::FileSystemParams
    }
};

use super::{Ext2FileSystem, Ext2Params};
use super::super::super::core::FileSystemOperations;

/// ext2 filesystem driver
/// 
/// This driver implements the FileSystemDriver trait and is responsible
/// for creating ext2 filesystem instances from block devices.
pub struct Ext2Driver;

impl FileSystemDriver for Ext2Driver {
    fn name(&self) -> &'static str {
        "ext2"
    }
    
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Block
    }
    
    fn create(&self) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // ext2 requires a block device, cannot create without one
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "ext2 filesystem requires a block device"
        ))
    }
    
    fn create_from_block(
        &self, 
        block_device: Arc<dyn BlockDevice>, 
        _block_size: usize
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // Create ext2 filesystem from the block device
        let fs = Ext2FileSystem::new(block_device)?;
        Ok(fs as Arc<dyn FileSystemOperations>)
    }
    
    fn create_from_memory(
        &self, 
        _memory_area: &crate::vm::vmem::MemoryArea
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // ext2 is a block-based filesystem, not memory-based
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "ext2 filesystem does not support memory-based creation"
        ))
    }
    
    fn create_from_option_string(
        &self, 
        options: &str
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // Parse options into Ext2Params
        let mut params = Ext2Params::from_option_string(options)?;
        
        // Create filesystem using params
        let fs = params.create_filesystem()?;
        Ok(fs as Arc<dyn FileSystemOperations>)
    }
    
    fn create_from_params(
        &self, 
        params: &dyn FileSystemParams
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // Downcast to Ext2Params
        let ext2_params = params.as_any()
            .downcast_ref::<Ext2Params>()
            .ok_or_else(|| FileSystemError::new(
                FileSystemErrorKind::InvalidData,
                "Invalid parameter type for ext2 filesystem"
            ))?;
        
        // Clone params to make them mutable for device resolution
        let mut params = ext2_params.clone();
        
        // Create filesystem using params
        let fs = params.create_filesystem()?;
        Ok(fs as Arc<dyn FileSystemOperations>)
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
        
        // Create a mock block device with a basic ext2 superblock
        let mock_device = MockBlockDevice::new("mock_ext2", 512, 65536);
        
        // Note: This test might fail due to the mock implementation not having
        // a proper ext2 superblock, but it tests the interface
        let result = driver.create_from_block(Arc::new(mock_device), 512);
        
        match result {
            Ok(_fs) => {
                // Successfully created filesystem (unlikely with empty mock device)
            },
            Err(e) => {
                // Expected to fail with mock device, but error should be about
                // filesystem format, not interface issues
                assert!(
                    e.kind == FileSystemErrorKind::IoError || 
                    e.kind == FileSystemErrorKind::InvalidData
                );
            }
        }
    }
}