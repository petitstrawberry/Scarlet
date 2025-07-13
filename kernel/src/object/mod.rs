//! Kernel object management system
//! 
//! This module provides a unified abstraction for all kernel-managed resources
//! including files, pipes, devices, and other IPC mechanisms.

pub mod capability;
pub mod introspection;
pub mod handle;

use alloc::{sync::Arc, vec::Vec};
use crate::fs::FileObject;
use crate::ipc::pipe::PipeObject;
use capability::{StreamOps, CloneOps};

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

impl Clone for KernelObject {
    fn clone(&self) -> Self {
        // Try to use CloneOps capability first
        if let Some(cloneable) = self.as_cloneable() {
            cloneable.custom_clone()
        } else {
            // Default: Use Arc::clone for direct cloning
            match self {
                KernelObject::File(file_object) => {
                    KernelObject::File(Arc::clone(file_object))
                }
                KernelObject::Pipe(pipe_object) => {
                    KernelObject::Pipe(Arc::clone(pipe_object))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;