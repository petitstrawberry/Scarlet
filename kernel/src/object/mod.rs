//! Kernel object management system
//! 
//! This module provides a unified abstraction for all kernel-managed resources
//! including files, pipes, devices, and other IPC mechanisms.

pub mod capability;

use alloc::{sync::Arc, vec::Vec};
use crate::fs::FileObject;
use crate::ipc::pipe::PipeObject;
use capability::{StreamOps, CloneOps};

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

#[derive(Clone)]
pub struct HandleTable {
    /// Fixed-size handle table
    handles: [Option<KernelObject>; Self::MAX_HANDLES],
    /// Metadata for each handle
    metadata: [Option<HandleMetadata>; Self::MAX_HANDLES],
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
            metadata: [const { None }; Self::MAX_HANDLES],
            free_handles,
        }
    }
    
    /// O(1) allocation with automatic metadata inference
    pub fn insert(&mut self, obj: KernelObject) -> Result<Handle, &'static str> {
        let metadata = Self::infer_metadata_from_object(&obj);
        self.insert_with_metadata(obj, metadata)
    }
    
    /// O(1) allocation with explicit metadata
    pub fn insert_with_metadata(&mut self, obj: KernelObject, metadata: HandleMetadata) -> Result<Handle, &'static str> {
        if let Some(handle) = self.free_handles.pop() {
            self.handles[handle as usize] = Some(obj);
            self.metadata[handle as usize] = Some(metadata);
            Ok(handle)
        } else {
            Err("Too many open KernelObjects, limit reached")
        }
    }
    
    /// Infer metadata from KernelObject type
    fn infer_metadata_from_object(object: &KernelObject) -> HandleMetadata {
        let handle_type = match object {
            KernelObject::Pipe(_) => HandleType::IpcChannel,  // Pipes are clearly IPC
            _ => HandleType::Regular,  // Everything else defaults to Regular
        };

        HandleMetadata {
            handle_type,
            access_mode: AccessMode::ReadWrite,  // Default value
            special_semantics: None,             // Normal behavior (inherit on exec, etc.)
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
            self.metadata[handle as usize] = None; // Clear metadata too
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
                self.metadata[i] = None; // Clear metadata too
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
    
    /// Get metadata for a handle
    pub fn get_metadata(&self, handle: Handle) -> Option<&HandleMetadata> {
        if handle as usize >= Self::MAX_HANDLES {
            return None;
        }
        self.metadata[handle as usize].as_ref()
    }
    
    /// Iterator over handles with their objects and metadata
    pub fn iter_with_metadata(&self) -> impl Iterator<Item = (Handle, &KernelObject, &HandleMetadata)> {
        self.handles.iter().enumerate()
            .filter_map(|(i, obj)| {
                obj.as_ref().and_then(|o| {
                    self.metadata[i].as_ref().map(|m| (i as Handle, o, m))
                })
            })
    }
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle metadata for managing special semantics and ABI conversion
#[derive(Clone, Debug)]
pub struct HandleMetadata {
    pub handle_type: HandleType,
    pub access_mode: AccessMode,
    pub special_semantics: Option<SpecialSemantics>,
}

/// Role-based handle classification
#[derive(Clone, Debug, PartialEq)]
pub enum HandleType {
    StandardStream(StandardStreamType),  // stdin/stdout/stderr role
    IpcChannel,                         // Inter-process communication
    Regular,                            // Default for other handles
}

#[derive(Clone, Debug, PartialEq)]
pub enum StandardStreamType {
    Stdin,
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AccessMode {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

/// Special behaviors that differ from default Unix semantics
#[derive(Clone, Debug, PartialEq)]
pub enum SpecialSemantics {
    CloseOnExec,        // Close on exec (O_CLOEXEC)
    NonBlocking,        // Non-blocking mode (O_NONBLOCK)
    Append,             // Append mode (O_APPEND)
    Sync,               // Synchronous writes (O_SYNC)
}

impl Default for HandleMetadata {
    fn default() -> Self {
        Self {
            handle_type: HandleType::Regular,
            access_mode: AccessMode::ReadWrite,
            special_semantics: None,
        }
    }
}

#[cfg(test)]
mod tests;