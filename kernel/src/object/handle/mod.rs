use alloc::vec::Vec;

use crate::object::{introspection, KernelObject};

pub mod syscall;

#[cfg(test)]
mod tests;

/// Handle type for referencing kernel objects
pub type Handle = u32;

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
    
    /// Infer metadata from KernelObject type and usage context
    /// 
    /// This function provides reasonable defaults for handle roles based on the KernelObject type.
    /// Applications can override this by using insert_with_metadata() to specify exact roles.
    fn infer_metadata_from_object(object: &KernelObject) -> HandleMetadata {
        let handle_type = match object {
            KernelObject::Pipe(_) => {
                // Pipes are typically used for IPC, but could also be used for
                // logging, temp storage, etc. We default to IPC as the most common case.
                HandleType::IpcChannel
            }
            KernelObject::File(_file_obj) => {
                // Files can serve many roles. Without additional context,
                // we default to Regular usage. Applications should use
                // insert_with_metadata() to specify specific roles like
                // ConfigFile, LogOutput, etc.
                HandleType::Regular
            }
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
    
    /// Update metadata for an existing handle
    pub fn update_metadata(&mut self, handle: Handle, new_metadata: HandleMetadata) -> Result<(), &'static str> {
        if handle as usize >= Self::MAX_HANDLES {
            return Err("Invalid handle");
        }
        
        if self.handles[handle as usize].is_some() {
            self.metadata[handle as usize] = Some(new_metadata);
            Ok(())
        } else {
            Err("Handle does not exist")
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
    
    /// Get detailed information about a KernelObject for user space introspection
    pub fn get_object_info(&self, handle: Handle) -> Option<introspection::KernelObjectInfo> {
        if let Some(kernel_obj) = self.get(handle) {
            let metadata = self.get_metadata(handle)?;
            let handle_role = introspection::HandleRole::from(metadata.handle_type.clone());
            let (readable, writable) = metadata.access_mode.into();
            
            match kernel_obj {
                KernelObject::File(_) => {
                    Some(introspection::KernelObjectInfo::for_file(handle_role, readable, writable))
                }
                KernelObject::Pipe(_) => {
                    Some(introspection::KernelObjectInfo::for_pipe(handle_role, readable, writable))
                }
            }
        } else {
            None
        }
    }
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle metadata for managing special semantics and ABI conversion
/// 
/// This metadata describes HOW a handle is being used, not WHAT the underlying KernelObject is.
/// This enables proper ABI conversion, security policies, and resource management.
/// 
/// ## Examples of Role-based Usage
/// 
/// ```rust
/// // Same file object used in different roles
/// let config_file = file_obj.clone();
/// let log_file = file_obj.clone();
/// 
/// // Handle for reading configuration
/// let config_handle = task.handle_table.insert_with_metadata(
///     KernelObject::File(config_file),
///     HandleMetadata {
///         handle_type: HandleType::ConfigFile,
///         access_mode: AccessMode::ReadOnly,
///         special_semantics: Some(SpecialSemantics::CloseOnExec),
///     }
/// )?;
/// 
/// // Handle for writing logs
/// let log_handle = task.handle_table.insert_with_metadata(
///     KernelObject::File(log_file),
///     HandleMetadata {
///         handle_type: HandleType::LogOutput,
///         access_mode: AccessMode::WriteOnly,
///         special_semantics: Some(SpecialSemantics::Append),
///     }
/// )?;
/// ```
#[derive(Clone, Debug)]
pub struct HandleMetadata {
    pub handle_type: HandleType,
    pub access_mode: AccessMode,
    pub special_semantics: Option<SpecialSemantics>,
}

/// Role-based handle classification
/// 
/// This enum describes HOW a handle is being used, not WHAT the underlying KernelObject is.
/// The same KernelObject (e.g., a File) could be used in different roles by different handles.
#[derive(Clone, Debug, PartialEq)]
pub enum HandleType {
    /// Standard input/output/error streams
    StandardInputOutput(StandardInputOutput),
    /// Inter-process communication channel
    IpcChannel,
    /// Default/generic usage
    Regular,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StandardInputOutput {
    Stdin,
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, PartialEq, Copy)]
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
