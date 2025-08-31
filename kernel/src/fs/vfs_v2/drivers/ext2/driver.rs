//! Ext2 Filesystem Driver Registration
//!
//! This module implements the filesystem driver interface for ext2,
//! allowing it to be registered with the VFS driver manager.

use alloc::sync::Arc;
use core::fmt::Debug;

use crate::{
    device::block::BlockDevice,
    fs::{
        FileSystemDriver, FileSystemError, FileSystemErrorKind, 
        FileSystemParams, FileSystemType
    },
    vm::vmem::MemoryArea
};

use crate::fs::vfs_v2::core::FileSystemOperations;
use super::{Ext2FileSystem};

/// Ext2 Filesystem Driver
/// 
/// This driver can create ext2 filesystem instances from block devices.
pub struct Ext2Driver;

impl Debug for Ext2Driver {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Ext2Driver").finish()
    }
}

impl FileSystemDriver for Ext2Driver {
    fn name(&self) -> &'static str {
        "ext2"
    }
    
    fn filesystem_type(&self) -> FileSystemType {
        FileSystemType::Block
    }
    
    fn create(&self) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // Ext2 requires a block device, cannot create without one
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "Ext2 filesystem requires a block device"
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
        _memory_area: &MemoryArea
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // Ext2 doesn't support memory-based creation
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "Ext2 filesystem does not support memory-based creation"
        ))
    }
    
    fn create_from_option_string(
        &self, 
        _options: &str
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // Ext2 doesn't support option string creation without a block device
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "Ext2 filesystem requires a block device, cannot create from options alone"
        ))
    }
    
    fn create_from_params(
        &self, 
        _params: &dyn FileSystemParams
    ) -> Result<Arc<dyn FileSystemOperations>, FileSystemError> {
        // For now, ext2 doesn't support parameter-based creation
        // This could be extended in the future to support formatting options
        Err(FileSystemError::new(
            FileSystemErrorKind::NotSupported,
            "Ext2 filesystem parameter-based creation not implemented"
        ))
    }
}