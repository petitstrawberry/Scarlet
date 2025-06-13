//! Kernel object management system
//! 
//! This module provides a unified abstraction for all kernel-managed resources
//! including files, pipes, devices, and other IPC mechanisms.

pub mod capability;

use alloc::sync::Arc;
use crate::fs::{FileObject, FileSystemError};
use capability::{StreamOps, FileStreamOps};

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
            KernelObject::File(file_handle) => {
                // FileObject automatically implements StreamOps
                let stream_ops: &dyn StreamOps = file_handle.as_ref();
                Some(stream_ops)
            }
        }
    }
    
    /// Try to get FileStreamOps capability (seekable streams)
    pub fn as_file_stream(&self) -> Option<&dyn FileStreamOps> {
        match self {
            KernelObject::File(file_handle) => {
                // FileObject automatically implements FileStreamOps
                let file_stream_ops: &dyn FileStreamOps = file_handle.as_ref();
                Some(file_stream_ops)
            }
        }
    }
}

impl Drop for KernelObject {
    fn drop(&mut self) {
        // Release resources when KernelObject is dropped
        match self {
            KernelObject::File(file_handle) => {
                let stream: &dyn StreamOps = file_handle.as_ref();
                stream.release();
            }
        }
    }
}
