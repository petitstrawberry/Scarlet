//! VFS v2 Driver Manager - Filesystem Driver Registration and Management
//!
//! This module provides the driver management infrastructure for VFS v2,
//! enabling dynamic registration and instantiation of filesystem drivers.
//!
//! ## Features
//!
//! - **Dynamic Driver Registration**: Runtime registration of filesystem drivers
//! - **Multiple Creation APIs**: Support for various filesystem creation methods
//! - **Type-Safe Driver IDs**: Enum-based filesystem identification
//! - **Flexible Instantiation**: Support for different parameter types
//!
//! ## Usage
//!
//! ```rust,no_run
//! // Register a filesystem driver
//! let driver_manager = FileSystemDriverManagerV2::new();
//! driver_manager.register_driver(Arc::new(MyFSDriver));
//!
//! // Create filesystem instance
//! let fs = driver_manager.create_from_option_string(
//!     FileSystemId::TmpFS,
//!     Some("size=1M")
//! )?;
//! ```
//!
//! ## Driver Implementation
//!
//! Filesystem drivers must implement the `FileSystemDriverV2` trait:
//!
//! ```rust,no_run
//! impl FileSystemDriverV2 for MyFSDriver {
//!     fn id(&self) -> FileSystemId { FileSystemId::MyFS }
//!     fn name(&self) -> &'static str { "myfs" }
//!     fn create_from_option_string(&self, option: Option<&str>) -> Arc<dyn FileSystemOperations> {
//!         // Implementation
//!     }
//!     // ... other methods
//! }
//! ```

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use spin::RwLock;

use super::core::FileSystemOperations;
use crate::fs::params::FileSystemParams;

/// Filesystem identifiers for VFS v2
///
/// This enum provides type-safe identification of filesystem types.
/// Each filesystem driver maps to a unique identifier that is used
/// for registration and instantiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileSystemId {
    /// In-memory temporary filesystem
    TmpFS,
    /// CPIO archive filesystem (for initramfs)
    CpioFS,
    /// Overlay/Union filesystem
    OverlayFS,
    // Add more filesystem types as needed
}

/// Filesystem driver trait for VFS v2
///
/// This trait defines the interface that all VFS v2 filesystem drivers
/// must implement. It provides multiple creation methods to support
/// different configuration approaches.
///
/// ## Implementation Requirements
///
/// - Must be Send + Sync for multi-threaded access
/// - Should handle creation errors gracefully
/// - May support multiple parameter formats for flexibility
pub trait FileSystemDriverV2: Send + Sync {
    /// Get the unique identifier for this filesystem type
    fn id(&self) -> FileSystemId;
    
    /// Get the human-readable name of this filesystem
    fn name(&self) -> &'static str;
    
    /// Create filesystem instance from option string
    ///
    /// # Arguments
    /// * `option` - Optional configuration string (format is driver-specific)
    fn create_from_option_string(&self, option: Option<&str>) -> Arc<dyn FileSystemOperations>;
    
    /// Create filesystem instance from structured parameters
    ///
    /// # Arguments  
    /// * `params` - Structured parameter object implementing FileSystemParams
    fn create_from_params(&self, params: &dyn FileSystemParams) -> Arc<dyn FileSystemOperations>;
    // Additional creation methods can be added as needed
}

/// Filesystem driver manager for VFS v2
///
/// This manager maintains a registry of available filesystem drivers
/// and provides APIs for creating filesystem instances. It supports
/// multiple creation methods and handles driver lifecycle management.
///
/// ## Thread Safety
///
/// The manager is thread-safe and can be accessed concurrently from
/// multiple threads or system call contexts.
pub struct FileSystemDriverManagerV2 {
    drivers: RwLock<BTreeMap<FileSystemId, Arc<dyn FileSystemDriverV2>>>,
}

impl FileSystemDriverManagerV2 {
    /// Create a new driver manager instance
    pub fn new() -> Self {
        Self { 
            drivers: RwLock::new(BTreeMap::new()) 
        }
    }

    /// Register a filesystem driver
    ///
    /// # Arguments
    /// * `driver` - The filesystem driver to register
    ///
    /// # Note
    /// If a driver with the same ID is already registered, it will be replaced.
    pub fn register_driver(&self, driver: Arc<dyn FileSystemDriverV2>) {
        self.drivers.write().insert(driver.id(), driver);
    }

    /// Get a registered driver by ID
    ///
    /// # Arguments
    /// * `id` - The filesystem ID to look up
    ///
    /// # Returns
    /// Returns Some(driver) if found, None if not registered
    pub fn get_driver(&self, id: FileSystemId) -> Option<Arc<dyn FileSystemDriverV2>> {
        self.drivers.read().get(&id).cloned()
    }

    /// Create filesystem instance from option string
    ///
    /// # Arguments
    /// * `id` - The filesystem type to create
    /// * `option` - Optional configuration string
    ///
    /// # Returns
    /// Returns Some(filesystem) if driver found and creation successful, None otherwise
    pub fn create_from_option_string(&self, id: FileSystemId, option: Option<&str>) -> Option<Arc<dyn FileSystemOperations>> {
        self.get_driver(id).map(|drv| drv.create_from_option_string(option))
    }

    /// Create filesystem instance from structured parameters
    ///
    /// # Arguments
    /// * `id` - The filesystem type to create
    /// * `params` - Structured parameter object
    ///
    /// # Returns
    /// Returns Some(filesystem) if driver found and creation successful, None otherwise
    pub fn create_from_params(&self, id: FileSystemId, params: &dyn FileSystemParams) -> Option<Arc<dyn FileSystemOperations>> {
        self.get_driver(id).map(|drv| drv.create_from_params(params))
    }
}

// Global v2 driver manager (can also be static with unsafe)
use core::sync::atomic::{AtomicPtr, Ordering};
static mut V2_DRIVER_MANAGER: Option<FileSystemDriverManagerV2> = None;

pub fn get_v2_driver_manager() -> &'static FileSystemDriverManagerV2 {
    unsafe {
        if V2_DRIVER_MANAGER.is_none() {
            V2_DRIVER_MANAGER = Some(FileSystemDriverManagerV2::new());
        }
        V2_DRIVER_MANAGER.as_ref().unwrap()
    }
}
