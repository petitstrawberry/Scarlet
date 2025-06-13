//! Kernel object management system
//! 
//! This module provides a unified abstraction for all kernel-managed resources
//! including files, pipes, devices, and other IPC mechanisms.

pub mod capability;

use alloc::{sync::Arc, vec::Vec};
use crate::fs::FileObject;
use capability::StreamOps;

/// Handle type for referencing kernel objects
pub type Handle = u32;

/// Unified representation of all kernel-managed resources
#[derive(Clone)]
pub enum KernelObject {
    File(Arc<dyn FileObject>),
    // Future variants will be added here:
    // Pipe(Arc<dyn PipeObject>),
    // CharDevice(Arc<dyn CharDevice>),
    // Socket(Arc<dyn SocketObject>),
}

impl KernelObject {
    /// Try to get StreamOps capability
    pub fn as_stream(&self) -> Option<&dyn StreamOps> {
        match self {
            KernelObject::File(file_object) => {
                // FileObject automatically implements StreamOps
                let stream_ops: &dyn StreamOps = file_object.as_ref();
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
        }
    }
}

impl Drop for KernelObject {
    fn drop(&mut self) {
        // Release resources when KernelObject is dropped
        match self {
            KernelObject::File(file_object) => {
                let stream: &dyn StreamOps = file_object.as_ref();
                let _ = stream.release();
            }
        }
    }
}

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