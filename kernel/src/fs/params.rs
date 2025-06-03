//! Filesystem Parameter System
//! 
//! This module provides a type-safe parameter system for filesystem creation,
//! replacing the legacy BTreeMap<String, String> approach with structured
//! configuration types that provide compile-time validation and better ergonomics.
//! 
//! # Overview
//! 
//! The parameter system enables:
//! - **Type Safety**: Compile-time validation of filesystem parameters
//! - **Backward Compatibility**: Conversion to/from string maps for legacy code
//! - **Extensibility**: Easy addition of new parameter types for different filesystems
//! - **Dynamic Dispatch**: Support for future dynamic filesystem module loading
//! 
//! # Architecture
//! 
//! All filesystem parameter types implement the `FileSystemParams` trait, which
//! provides standardized interfaces for:
//! - String map conversion for backward compatibility
//! - Dynamic type identification for runtime dispatch
//! - Structured access to typed configuration data
//! 
//! # Usage
//! 
//! ```rust
//! use crate::fs::params::{TmpFSParams, BasicFSParams};
//! 
//! // Create TmpFS with 1MB memory limit
//! let tmpfs_params = TmpFSParams::with_memory_limit(1048576);
//! let fs_id = vfs_manager.create_and_register_fs_with_params("tmpfs", &tmpfs_params)?;
//! 
//! // Create basic filesystem
//! let basic_params = BasicFSParams::with_block_size(4096);
//! let fs_id = vfs_manager.create_and_register_fs_with_params("ext4", &basic_params)?;
//! ```

use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use core::any::Any;

/// Core trait for filesystem parameter types
/// 
/// This trait enables type-safe filesystem configuration while maintaining
/// backward compatibility with string-based parameter systems. All filesystem
/// parameter structures must implement this trait to be usable with the
/// VfsManager's structured parameter creation methods.
/// 
/// # Dynamic Dispatch Support
/// 
/// The trait includes `as_any()` to enable dynamic downcasting, which supports
/// future dynamic filesystem module loading scenarios where parameter types
/// may not be known at compile time.
pub trait FileSystemParams {
    /// Convert parameters to string map for backward compatibility
    /// 
    /// This method serializes the structured parameters into a key-value
    /// string map that can be consumed by legacy filesystem drivers that
    /// haven't been updated to use structured parameters.
    /// 
    /// # Returns
    /// 
    /// BTreeMap containing string representations of all parameters
    fn to_string_map(&self) -> BTreeMap<String, String>;
    
    /// Create parameters from string map for backward compatibility
    /// 
    /// This method deserializes parameters from a string map, enabling
    /// legacy code to continue working while gradually migrating to
    /// structured parameter usage.
    /// 
    /// # Arguments
    /// 
    /// * `map` - String map containing parameter key-value pairs
    /// 
    /// # Returns
    /// 
    /// * `Ok(Self)` - Successfully parsed parameters
    /// * `Err(String)` - Parse error with description
    fn from_string_map(map: &BTreeMap<String, String>) -> Result<Self, String>
    where
        Self: Sized;
        
    /// Enable dynamic downcasting for runtime type identification
    /// 
    /// This method supports dynamic dispatch scenarios where the exact
    /// parameter type is not known at compile time, such as when loading
    /// filesystem modules dynamically.
    /// 
    /// # Returns
    /// 
    /// Reference to self as Any trait object for downcasting
    fn as_any(&self) -> &dyn Any;
}

/// TmpFS filesystem configuration parameters
/// 
/// Configuration structure for creating TmpFS (temporary filesystem) instances.
/// TmpFS is a RAM-based filesystem that stores all data in memory, making it
/// very fast but volatile (data is lost on reboot).
/// 
/// # Features
/// 
/// - **Memory Limiting**: Configurable maximum memory usage to prevent OOM
/// - **Performance**: All operations occur in RAM for maximum speed
/// - **Volatility**: Data exists only while mounted and system is running
/// 
/// # Memory Management
/// 
/// The memory limit prevents runaway processes from consuming all available
/// RAM through filesystem operations. A limit of 0 means unlimited memory usage.
#[derive(Debug, Clone, PartialEq)]
pub struct TmpFSParams {
    /// Maximum memory usage in bytes (0 = unlimited)
    /// 
    /// This limit applies to the total size of all files and directories
    /// stored in the TmpFS instance. When the limit is reached, write
    /// operations will fail with ENOSPC (No space left on device).
    pub memory_limit: usize,
}

impl Default for TmpFSParams {
    /// Create TmpFS parameters with unlimited memory
    /// 
    /// The default configuration allows unlimited memory usage, which
    /// provides maximum flexibility but requires careful monitoring in
    /// production environments.
    fn default() -> Self {
        Self {
            memory_limit: 0, // Unlimited by default
        }
    }
}

impl TmpFSParams {
    /// Create TmpFS parameters with specified memory limit
    /// 
    /// # Arguments
    /// 
    /// * `memory_limit` - Maximum memory usage in bytes (0 for unlimited)
    /// 
    /// # Returns
    /// 
    /// TmpFSParams instance with the specified memory limit
    /// 
    /// # Example
    /// 
    /// ```rust
    /// // Create TmpFS with 10MB limit
    /// let params = TmpFSParams::with_memory_limit(10 * 1024 * 1024);
    /// ```
    pub fn with_memory_limit(memory_limit: usize) -> Self {
        Self {
            memory_limit,
        }
    }
}

impl FileSystemParams for TmpFSParams {
    fn to_string_map(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert("memory_limit".to_string(), self.memory_limit.to_string());
        map
    }
    
    fn from_string_map(map: &BTreeMap<String, String>) -> Result<Self, String> {
        let memory_limit = if let Some(limit_str) = map.get("memory_limit") {
            limit_str.parse::<usize>()
                .map_err(|_| format!("Invalid memory_limit value: {}", limit_str))?
        } else {
            0 // Default to unlimited memory
        };

        Ok(Self { memory_limit })
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Parameters for CPIO filesystem creation
#[derive(Debug, Clone, PartialEq)]
pub struct CpioFSParams {
}

impl Default for CpioFSParams {
    fn default() -> Self {
        Self {
        }
    }
}

impl CpioFSParams {
    /// Create CPIO parameters
    pub fn new() -> Self {
        Self { }
    }
}

impl FileSystemParams for CpioFSParams {
    fn to_string_map(&self) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
    
    fn from_string_map(_map: &BTreeMap<String, String>) -> Result<Self, String> {
        Ok(Self { })
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Generic parameters for basic filesystem creation
#[derive(Debug, Clone, PartialEq)]
pub struct BasicFSParams {
    /// Block size (for block-based filesystems)
    pub block_size: Option<usize>,
    /// Read-only flag
    pub read_only: bool,
}

impl Default for BasicFSParams {
    fn default() -> Self {
        Self {
            block_size: None,
            read_only: false,
        }
    }
}

impl BasicFSParams {
    /// Create basic parameters with default values
    pub fn new() -> Self {
        Self {
            block_size: None,
            read_only: false,
        }
    }
    
    /// Set block size
    pub fn with_block_size(mut self, block_size: usize) -> Self {
        self.block_size = Some(block_size);
        self
    }
    
    /// Set read-only flag
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }
}

impl FileSystemParams for BasicFSParams {
    fn to_string_map(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        
        if let Some(block_size) = self.block_size {
            map.insert("block_size".to_string(), block_size.to_string());
        }
        
        map.insert("read_only".to_string(), self.read_only.to_string());
        map
    }
    
    fn from_string_map(map: &BTreeMap<String, String>) -> Result<Self, String> {
        let block_size = if let Some(size_str) = map.get("block_size") {
            Some(size_str.parse::<usize>()
                .map_err(|_| format!("Invalid block_size value: {}", size_str))?)
        } else {
            None
        };

        let read_only = if let Some(ro_str) = map.get("read_only") {
            ro_str.parse::<bool>()
                .map_err(|_| format!("Invalid read_only value: {}", ro_str))?
        } else {
            false // Default to read-write
        };

        Ok(Self { block_size, read_only })
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}
