//! Capability traits for KernelObject resources
//! 
//! This module defines capability traits that represent the operations
//! that can be performed on different types of kernel objects.

pub mod stream;
pub mod file;
pub mod control;
pub mod memory_mapping;
pub mod ipc;

#[cfg(test)]
mod control_tests;

use crate::object::KernelObject;

// Re-export stream types for backward compatibility
pub use stream::{StreamError, StreamOps};

// Re-export file types for backward compatibility
pub use file::{FileObject, SeekFrom};

// Re-export control types
pub use control::ControlOps;

// Re-export memory mapping types
pub use memory_mapping::MemoryMappingOps;

// Re-export IPC types
pub use ipc::{EventSender, EventReceiver, EventSubscriber};

/// Clone operations capability
/// 
/// This trait represents the ability to properly clone an object
/// with custom semantics. Objects that need special cloning behavior
/// (like pipes that need to update reader/writer counts) should implement this.
/// 
/// The presence of this capability indicates that the object needs custom
/// clone semantics beyond simple Arc::clone.
pub trait CloneOps: Send + Sync {
    /// Perform a custom clone operation and return the cloned object
    /// 
    /// This method should handle any object-specific cloning logic,
    /// such as incrementing reference counts for pipes or other shared resources.
    /// Returns the cloned object as a KernelObject.
    fn custom_clone(&self) -> KernelObject;
}