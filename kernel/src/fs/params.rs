//! File System Parameter Types
//! 
//! This module provides type-safe parameter structures for filesystem creation,
//! replacing the raw BTreeMap<String, String> approach with proper structured
//! configuration types.

use alloc::string::{String, ToString};
use alloc::collections::BTreeMap;
use alloc::format;
use core::any::Any;

/// Trait for filesystem-specific parameter types
pub trait FileSystemParams {
    /// Convert the parameters to a string map for backward compatibility
    fn to_string_map(&self) -> BTreeMap<String, String>;
    
    /// Create parameters from a string map for backward compatibility
    fn from_string_map(map: &BTreeMap<String, String>) -> Result<Self, String>
    where
        Self: Sized;
        
    /// Enable dynamic downcasting for parameter types
    fn as_any(&self) -> &dyn Any;
}

/// Parameters for TmpFS filesystem creation
#[derive(Debug, Clone, PartialEq)]
pub struct TmpFSParams {
    /// Maximum memory usage in bytes (0 = unlimited)
    pub memory_limit: usize,
    /// Filesystem identifier
    pub fs_id: usize,
}

impl Default for TmpFSParams {
    fn default() -> Self {
        Self {
            memory_limit: 0, // Unlimited by default
            fs_id: 0,
        }
    }
}

impl TmpFSParams {
    /// Create TmpFS parameters with specified memory limit
    pub fn with_memory_limit(memory_limit: usize) -> Self {
        Self {
            memory_limit,
            fs_id: 0,
        }
    }
    
    /// Create TmpFS parameters with specified filesystem ID
    pub fn with_fs_id(fs_id: usize) -> Self {
        Self {
            memory_limit: 0,
            fs_id,
        }
    }
    
    /// Create TmpFS parameters with both memory limit and filesystem ID
    pub fn new(memory_limit: usize, fs_id: usize) -> Self {
        Self {
            memory_limit,
            fs_id,
        }
    }
}

impl FileSystemParams for TmpFSParams {
    fn to_string_map(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert("memory_limit".to_string(), self.memory_limit.to_string());
        map.insert("fs_id".to_string(), self.fs_id.to_string());
        map
    }
    
    fn from_string_map(map: &BTreeMap<String, String>) -> Result<Self, String> {
        let memory_limit = if let Some(limit_str) = map.get("memory_limit") {
            limit_str.parse::<usize>()
                .map_err(|_| format!("Invalid memory_limit value: {}", limit_str))?
        } else {
            0 // Default to unlimited memory
        };

        let fs_id = if let Some(id_str) = map.get("fs_id") {
            id_str.parse::<usize>()
                .map_err(|_| format!("Invalid fs_id value: {}", id_str))?
        } else {
            0 // Default filesystem ID
        };

        Ok(Self { memory_limit, fs_id })
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Parameters for CPIO filesystem creation
#[derive(Debug, Clone, PartialEq)]
pub struct CpioFSParams {
    /// Filesystem identifier
    pub fs_id: usize,
}

impl Default for CpioFSParams {
    fn default() -> Self {
        Self {
            fs_id: 0,
        }
    }
}

impl CpioFSParams {
    /// Create CPIO parameters with specified filesystem ID
    pub fn new(fs_id: usize) -> Self {
        Self { fs_id }
    }
}

impl FileSystemParams for CpioFSParams {
    fn to_string_map(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert("fs_id".to_string(), self.fs_id.to_string());
        map
    }
    
    fn from_string_map(map: &BTreeMap<String, String>) -> Result<Self, String> {
        let fs_id = if let Some(id_str) = map.get("fs_id") {
            id_str.parse::<usize>()
                .map_err(|_| format!("Invalid fs_id value: {}", id_str))?
        } else {
            0 // Default filesystem ID
        };

        Ok(Self { fs_id })
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Generic parameters for basic filesystem creation
#[derive(Debug, Clone, PartialEq)]
pub struct BasicFSParams {
    /// Filesystem identifier
    pub fs_id: usize,
    /// Block size (for block-based filesystems)
    pub block_size: Option<usize>,
    /// Read-only flag
    pub read_only: bool,
}

impl Default for BasicFSParams {
    fn default() -> Self {
        Self {
            fs_id: 0,
            block_size: None,
            read_only: false,
        }
    }
}

impl BasicFSParams {
    /// Create basic parameters with specified filesystem ID
    pub fn new(fs_id: usize) -> Self {
        Self {
            fs_id,
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
        map.insert("fs_id".to_string(), self.fs_id.to_string());
        
        if let Some(block_size) = self.block_size {
            map.insert("block_size".to_string(), block_size.to_string());
        }
        
        map.insert("read_only".to_string(), self.read_only.to_string());
        map
    }
    
    fn from_string_map(map: &BTreeMap<String, String>) -> Result<Self, String> {
        let fs_id = if let Some(id_str) = map.get("fs_id") {
            id_str.parse::<usize>()
                .map_err(|_| format!("Invalid fs_id value: {}", id_str))?
        } else {
            0 // Default filesystem ID
        };

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

        Ok(Self { fs_id, block_size, read_only })
    }
    
    fn as_any(&self) -> &dyn Any {
        self
    }
}
