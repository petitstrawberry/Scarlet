//! Control operations capability module
//! 
//! This module provides system calls and traits for ControlOps capability,
//! which enables device control operations (ioctl-equivalent) on KernelObjects.

use alloc::vec::Vec;

/// Control operations capability
/// 
/// This trait represents the ability to perform control operations
/// on a resource, similar to ioctl operations in POSIX systems.
/// Control operations allow querying and modifying device-specific
/// settings and configurations.
pub trait ControlOps: Send + Sync {
    /// Perform a control operation
    /// 
    /// # Arguments
    /// 
    /// * `command` - The control command identifier
    /// * `arg` - Command-specific argument (often a pointer to data)
    /// 
    /// # Returns
    /// 
    /// * `Result<i32, &'static str>` - Command-specific return value or error
    /// 
    /// # Default Implementation
    /// 
    /// The default implementation returns an error indicating that control
    /// operations are not supported by this object.
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        let _ = (command, arg);
        Err("Control operation not supported")
    }
    
    /// Get a list of supported control commands
    /// 
    /// # Returns
    /// 
    /// A vector of tuples containing (command_id, description) for each
    /// supported control command. This can be used for introspection
    /// and debugging purposes.
    /// 
    /// # Default Implementation
    /// 
    /// The default implementation returns an empty vector, indicating
    /// no control commands are supported.
    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        Vec::new()
    }
}