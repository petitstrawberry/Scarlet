//! Kernel object management system
//! 
//! This module provides a unified abstraction for all kernel-managed resources
//! including files, pipes, devices, and other IPC mechanisms.
//!
//! ## Design Notes
//!
//! ### Clone Semantics (`dup` operation)
//! 
//! Different kernel objects have different requirements for duplication:
//! 
//! - **Files**: Share state (file position) between duplicated handles. 
//!   `Arc::clone` is sufficient as it shares the same underlying file object.
//! 
//! - **Pipes**: Require custom clone logic to properly increment reader/writer counts.
//!   `Arc::clone` alone would bypass the custom `Clone` implementation, breaking
//!   pipe protocol semantics (broken pipe detection, EOF handling).
//! 
//! To solve this, `KernelObject::clone()` uses the `clone_pipe()` method on `PipeObject`
//! to ensure proper duplication semantics for pipes while maintaining efficient
//! `Arc::clone` behavior for files.
//!
//! This approach provides correct `dup` semantics for both file and pipe objects
//! while maintaining performance and avoiding the complexity of a complete redesign.

pub mod capability;

use alloc::{sync::Arc, vec::Vec};
use crate::{fs::FileObject, object::capability::CloneOps};
use crate::ipc::pipe::PipeObject;
use capability::StreamOps;

/// Handle type for referencing kernel objects
pub type Handle = u32;

/// Unified representation of all kernel-managed resources
pub enum KernelObject {
    File(Arc<dyn FileObject>),
    Pipe(Arc<dyn PipeObject>),
    // Future variants will be added here:
    // CharDevice(Arc<dyn CharDevice>),
    // Socket(Arc<dyn SocketObject>),
}

impl KernelObject {
    /// Create a KernelObject from a FileObject
    pub fn from_file_object(file_object: Arc<dyn FileObject>) -> Self {
        KernelObject::File(file_object)
    }
    
    /// Create a KernelObject from a PipeObject
    pub fn from_pipe_object(pipe_object: Arc<dyn PipeObject>) -> Self {
        KernelObject::Pipe(pipe_object)
    }
    
    /// Try to get StreamOps capability
    pub fn as_stream(&self) -> Option<&dyn StreamOps> {
        match self {
            KernelObject::File(file_object) => {
                // FileObject automatically implements StreamOps
                let stream_ops: &dyn StreamOps = file_object.as_ref();
                Some(stream_ops)
            }
            KernelObject::Pipe(pipe_object) => {
                // PipeObject automatically implements StreamOps
                let stream_ops: &dyn StreamOps = pipe_object.as_ref();
                Some(stream_ops)
            }
        }
    }
    
    /// Try to get FileObject that provides file-like operations and stream capabilities
    pub fn as_file(&self) -> Option<&dyn FileObject> {
        match self {
            KernelObject::File(file_object) => {
                // FileObject automatically implements StreamOps
                let file_ops: &dyn FileObject = file_object.as_ref();
                Some(file_ops)
            }
            KernelObject::Pipe(_) => {
                // Pipes don't provide file operations
                None
            }
        }
    }
    
    /// Try to get PipeObject that provides pipe-specific operations
    pub fn as_pipe(&self) -> Option<&dyn PipeObject> {
        match self {
            KernelObject::File(_) => {
                // Files don't provide pipe operations
                None
            }
            KernelObject::Pipe(pipe_object) => {
                let pipe_ops: &dyn PipeObject = pipe_object.as_ref();
                Some(pipe_ops)
            }
        }
    }
    
    /// Try to get CloneOps capability
    pub fn as_cloneable(&self) -> Option<&dyn CloneOps> {
        match self {
            KernelObject::File(_) => {
                None // Files do not implement CloneOps, use Arc::clone directly
            }
            KernelObject::Pipe(pipe_object) => {
                // Check if PipeObject implements CloneOps
                let cloneable: &dyn CloneOps = pipe_object.as_ref();
                Some(cloneable)
            }
        }
    }
}

impl Drop for KernelObject {
    fn drop(&mut self) {
        // When a KernelObject is dropped, it will automatically drop the underlying
        // Arc reference, which will call the Drop implementation of FileObject or PipeObject.
        // No additional cleanup is needed here.
    }
}

impl Clone for KernelObject {
    fn clone(&self) -> Self {
        // Try to use CloneOps capability first
        if let Some(cloneable) = self.as_cloneable() {
            cloneable.custom_clone()
        } else {
            // Fallback to Arc::clone
            match self {
                KernelObject::File(file_object) => KernelObject::File(Arc::clone(file_object)),
                KernelObject::Pipe(pipe_object) => KernelObject::Pipe(Arc::clone(pipe_object)),
            }
        }
    }
}



#[derive(Clone)]
pub struct HandleTable {
    /// Fixed-size handle table
    handles: [Option<KernelObject>; Self::MAX_HANDLES],
    /// Stack of available handle numbers for O(1) allocation
    free_handles: Vec<Handle>,
}

impl HandleTable {
    const MAX_HANDLES: usize = 1024; // POSIX standard limit (fd)
    
    pub fn new() -> Self {
        // Initialize free handle stack in forward order (0 will be allocated first)
        let mut free_handles = Vec::with_capacity(Self::MAX_HANDLES);
        for handle in (0..Self::MAX_HANDLES as Handle).rev() {
            free_handles.push(handle);
        }
        
        Self {
            handles: [const { None }; Self::MAX_HANDLES],
            free_handles,
        }
    }
    
    /// O(1) allocation
    pub fn insert(&mut self, obj: KernelObject) -> Result<Handle, &'static str> {
        if let Some(handle) = self.free_handles.pop() {
            self.handles[handle as usize] = Some(obj);
            Ok(handle)
        } else {
            Err("Too many open KernelObjects, limit reached")
        }
    }
    
    /// O(1) access
    pub fn get(&self, handle: Handle) -> Option<&KernelObject> {
        if handle as usize >= Self::MAX_HANDLES {
            return None;
        }
        self.handles[handle as usize].as_ref()
    }
    
    /// O(1) removal
    pub fn remove(&mut self, handle: Handle) -> Option<KernelObject> {
        if handle as usize >= Self::MAX_HANDLES {
            return None;
        }

        if let Some(obj) = self.handles[handle as usize].take() {
            self.free_handles.push(handle); // Return to free pool
            Some(obj)
        } else {
            None
        }
    }
    
    /// Get the number of open handles
    pub fn open_count(&self) -> usize {
        Self::MAX_HANDLES - self.free_handles.len()
    }
    
    /// Get all active handles
    pub fn active_handles(&self) -> Vec<Handle> {
        self.handles
            .iter()
            .enumerate()
            .filter_map(|(i, handle)| {
                if handle.is_some() {
                    Some(i as Handle)
                } else {
                    None
                }
            })
            .collect()
    }
    
    /// Close all handles (for process termination)
    pub fn close_all(&mut self) {
        for (i, handle) in self.handles.iter_mut().enumerate() {
            if let Some(_obj) = handle.take() {
                // obj is automatically dropped, calling its Drop implementation
                self.free_handles.push(i as Handle);
            }
        }
    }
    
    /// Check if a handle is valid
    pub fn is_valid_handle(&self, handle: Handle) -> bool {
        if handle as usize >= Self::MAX_HANDLES {
            return false;
        }
        self.handles[handle as usize].is_some()
    }
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;