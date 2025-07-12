//! Capability traits for KernelObject resources
//! 
//! This module defines capability traits that represent the operations
//! that can be performed on different types of kernel objects.

pub mod stream;

use crate::object::KernelObject;

// Re-export stream types for backward compatibility
pub use stream::{StreamError, StreamOps};

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