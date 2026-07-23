use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};
use crate::sync::RwLock;

use crate::object::{KernelObject, KernelObjectRef, introspection};

pub mod syscall;

#[cfg(test)]
mod tests;

/// Handle type for referencing kernel objects
pub type Handle = u32;

/// Internal structure containing the actual handle table data.
/// This is wrapped in Arc<RwLock<...>> to enable sharing between tasks.
struct HandleTableInner {
    /// Fixed-size handle table allocated on heap to avoid stack overflow
    handles: Box<[Option<KernelObject>; HandleTable::MAX_HANDLES]>,
    /// Metadata for each handle allocated on heap to avoid stack overflow
    metadata: Box<[Option<HandleMetadata>; HandleTable::MAX_HANDLES]>,
    /// Stack of available handle numbers for O(1) allocation
    free_handles: Vec<Handle>,
}

/// Handle table for managing kernel objects with support for sharing between tasks.
///
/// This structure uses interior mutability via `Arc<RwLock<...>>` to enable
/// sharing between parent and child tasks when using CLONE_FILES flag.
/// The `Clone` implementation creates a shallow copy (Arc clone) that shares
/// the same underlying data. Use `deep_clone()` for an independent copy.
pub struct HandleTable {
    inner: Arc<RwLock<HandleTableInner>>,
}

impl HandleTable {
    /// Maximum number of handles per table (POSIX standard limit for fd)
    pub const MAX_HANDLES: usize = 1024;

    /// Create a new empty handle table.
    pub fn new() -> Self {
        // Initialize free handle stack in forward order (0 will be allocated first)
        let mut free_handles = Vec::new();
        for handle in (0..Self::MAX_HANDLES as Handle).rev() {
            free_handles.push(handle);
        }

        // Allocate handles and metadata as boxed slices to avoid stack overflow
        let handles = vec![None; Self::MAX_HANDLES]
            .try_into()
            .unwrap_or_else(|_| panic!("Failed to create boxed slice for handles"));

        let metadata = vec![None; Self::MAX_HANDLES]
            .try_into()
            .unwrap_or_else(|_| panic!("Failed to create boxed slice for metadata"));

        Self {
            inner: Arc::new(RwLock::new(HandleTableInner {
                handles,
                metadata,
                free_handles,
            })),
        }
    }

    /// Create a deep clone of this handle table (independent copy).
    ///
    /// This method creates a completely independent copy of the handle table,
    /// including all handles and metadata. Use this when you need separate
    /// handle tables for parent and child tasks (non-CLONE_FILES behavior).
    pub fn deep_clone(&self) -> Self {
        let inner = self.inner.read();

        let handles_clone = {
            let vec: Vec<Option<KernelObject>> = inner.handles.to_vec();
            vec.try_into()
                .unwrap_or_else(|_| panic!("slice with incorrect length"))
        };

        let metadata_clone = {
            let vec: Vec<Option<HandleMetadata>> = inner.metadata.to_vec();
            vec.try_into()
                .unwrap_or_else(|_| panic!("slice with incorrect length"))
        };

        Self {
            inner: Arc::new(RwLock::new(HandleTableInner {
                handles: handles_clone,
                metadata: metadata_clone,
                free_handles: inner.free_handles.clone(),
            })),
        }
    }

    /// O(1) allocation with automatic metadata inference
    pub fn insert(&self, obj: KernelObject) -> Result<Handle, &'static str> {
        let metadata = Self::infer_metadata_from_object(&obj);
        self.insert_with_metadata(obj, metadata)
    }

    /// O(1) allocation with explicit metadata
    pub fn insert_with_metadata(
        &self,
        obj: KernelObject,
        metadata: HandleMetadata,
    ) -> Result<Handle, &'static str> {
        let mut inner = self.inner.write();
        if let Some(handle) = inner.free_handles.pop() {
            inner.handles[handle as usize] = Some(obj);
            inner.metadata[handle as usize] = Some(metadata);
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
            #[cfg(feature = "network")]
            KernelObject::Socket(_) => {
                // Sockets are used for network and IPC communication
                HandleType::IpcChannel
            }
            KernelObject::EventChannel(_) => {
                // Event channels are used for pub/sub communication
                HandleType::EventChannel
            }
            KernelObject::EventSubscription(_) => {
                // Event subscriptions are used for receiving events
                HandleType::EventSubscription
            }
            KernelObject::SharedMemory(_) => {
                // Shared memory is used for IPC and data sharing
                HandleType::IpcChannel
            }
            KernelObject::Counter(_) => {
                // Counter is used for event notification (IPC)
                HandleType::IpcChannel
            }
            KernelObject::Timer(_) => HandleType::IpcChannel,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => HandleType::Regular,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => HandleType::Regular,
        };

        HandleMetadata {
            handle_type,
            access_mode: AccessMode::ReadWrite, // Default value
            special_semantics: None,            // Normal behavior (inherit on exec, etc.)
        }
    }

    /// O(1) access - executes a closure with a reference to the KernelObject
    ///
    /// Since the internal data is protected by RwLock, we cannot return a direct
    /// reference. Instead, use this method to access the object within a closure.
    pub fn with_object<F, R>(&self, handle: Handle, f: F) -> Option<R>
    where
        F: FnOnce(&KernelObject) -> R,
    {
        if handle as usize >= Self::MAX_HANDLES {
            return None;
        }
        let inner = self.inner.read();
        inner.handles[handle as usize].as_ref().map(f)
    }

    /// O(1) borrowed access that hides `Arc` ownership from the caller.
    ///
    /// This method returns a borrowed view of the handle-table object without
    /// incrementing the object's `Arc` reference count. The returned view does
    /// not expose the underlying `Arc`, so ordinary handle lookup cannot extend
    /// object lifetime beyond the owning handle table.
    ///
    /// # Arguments
    ///
    /// * `handle` - Handle number to access
    ///
    /// # Returns
    ///
    /// Borrowed object view if the handle exists, otherwise `None`.
    pub fn get(&self, handle: Handle) -> Option<KernelObjectRef<'_>> {
        if handle as usize >= Self::MAX_HANDLES {
            return None;
        }
        let inner = self.inner.read();
        let object = inner.handles[handle as usize].as_ref()? as *const KernelObject;
        // SAFETY: The pointer refers to a handle-table slot in `self`. The view
        // is lifetime-bound to `&self` and does not own or clone the object.
        Some(unsafe { KernelObjectRef::from_ptr(object) })
    }

    /// O(1) borrowed access that hides `Arc` ownership from the caller.
    ///
    /// Since the internal data is protected by RwLock, the borrowed view is
    /// available only for the duration of the closure. Use this for ordinary
    /// handle operations that should not extend object lifetime beyond the
    /// owning handle table.
    ///
    /// # Arguments
    ///
    /// * `handle` - Handle number to access
    /// * `f` - Closure executed with a borrowed kernel object view
    ///
    /// # Returns
    ///
    /// The closure result if the handle exists, otherwise `None`.
    pub fn with_object_ref<F, R>(&self, handle: Handle, f: F) -> Option<R>
    where
        F: for<'a> FnOnce(KernelObjectRef<'a>) -> R,
    {
        if handle as usize >= Self::MAX_HANDLES {
            return None;
        }
        let inner = self.inner.read();
        inner.handles[handle as usize]
            .as_ref()
            .map(|object| f(KernelObjectRef::new(object)))
    }

    /// O(1) access - returns an Arc-level clone of the KernelObject if it exists.
    ///
    /// This method returns an Arc-level clone of the KernelObject. Unlike the Clone
    /// trait which may have side effects (e.g., incrementing Pipe reader/writer counts),
    /// this performs a simple Arc reference count increment without modifying object state.
    ///
    /// Prefer [`HandleTable::get`] for ordinary handle lookup. Use this method only
    /// when an operation must intentionally keep the object alive independently of
    /// the owning handle table.
    pub fn get_arc_clone(&self, handle: Handle) -> Option<KernelObject> {
        if handle as usize >= Self::MAX_HANDLES {
            return None;
        }
        let inner = self.inner.read();
        inner.handles[handle as usize]
            .as_ref()
            .map(|obj| obj.arc_clone())
    }

    /// O(1) access - returns a full clone with dup() semantics
    ///
    /// This method uses the KernelObject's Clone trait which may invoke custom_clone()
    /// for objects like Pipes. This is appropriate when duplicating a file descriptor
    /// (dup/dup2 syscalls) where the new descriptor should be tracked separately.
    pub fn clone_for_dup(&self, handle: Handle) -> Option<KernelObject> {
        if handle as usize >= Self::MAX_HANDLES {
            return None;
        }
        let inner = self.inner.read();
        inner.handles[handle as usize].clone()
    }

    /// O(1) removal
    pub fn remove(&self, handle: Handle) -> Option<KernelObject> {
        if handle as usize >= Self::MAX_HANDLES {
            return None;
        }

        let mut inner = self.inner.write();
        if let Some(obj) = inner.handles[handle as usize].take() {
            inner.metadata[handle as usize] = None; // Clear metadata too
            inner.free_handles.push(handle); // Return to free pool
            Some(obj)
        } else {
            None
        }
    }

    /// Update metadata for an existing handle
    pub fn update_metadata(
        &self,
        handle: Handle,
        new_metadata: HandleMetadata,
    ) -> Result<(), &'static str> {
        if handle as usize >= Self::MAX_HANDLES {
            return Err("Invalid handle");
        }

        let mut inner = self.inner.write();
        if inner.handles[handle as usize].is_some() {
            inner.metadata[handle as usize] = Some(new_metadata);
            Ok(())
        } else {
            Err("Handle does not exist")
        }
    }

    /// Get the number of open handles
    pub fn open_count(&self) -> usize {
        let inner = self.inner.read();
        Self::MAX_HANDLES - inner.free_handles.len()
    }

    /// Get all active handles
    pub fn active_handles(&self) -> Vec<Handle> {
        let inner = self.inner.read();
        inner
            .handles
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

    /// Check if this is the sole owner of the underlying handle table.
    ///
    /// Returns `true` if no other `HandleTable` shares the same inner data
    /// (i.e., the `Arc` strong reference count is 1).
    /// This is used to decide whether `close_all` should run during task exit:
    /// when the handle table is shared via `CLONE_FILES` (threads), only the
    /// last task holding the table should close all handles.
    pub fn is_sole_owner(&self) -> bool {
        Arc::strong_count(&self.inner) == 1
    }

    /// Close all handles (for process termination)
    pub fn close_all(&self) {
        let mut inner = self.inner.write();
        for i in 0..Self::MAX_HANDLES {
            if let Some(_obj) = inner.handles[i].take() {
                // obj is automatically dropped, calling its Drop implementation
                inner.metadata[i] = None; // Clear metadata too
                inner.free_handles.push(i as Handle);
            }
        }
    }

    /// Check if a handle is valid
    pub fn is_valid_handle(&self, handle: Handle) -> bool {
        if handle as usize >= Self::MAX_HANDLES {
            return false;
        }
        let inner = self.inner.read();
        inner.handles[handle as usize].is_some()
    }

    /// Get metadata for a handle - returns a clone since we can't return a reference
    pub fn get_metadata(&self, handle: Handle) -> Option<HandleMetadata> {
        if handle as usize >= Self::MAX_HANDLES {
            return None;
        }
        let inner = self.inner.read();
        inner.metadata[handle as usize].clone()
    }

    /// Execute a closure with access to metadata
    pub fn with_metadata<F, R>(&self, handle: Handle, f: F) -> Option<R>
    where
        F: FnOnce(&HandleMetadata) -> R,
    {
        if handle as usize >= Self::MAX_HANDLES {
            return None;
        }
        let inner = self.inner.read();
        inner.metadata[handle as usize].as_ref().map(f)
    }

    /// Iterate over handles with their objects and metadata, executing a closure for each
    pub fn for_each_with_metadata<F>(&self, mut f: F)
    where
        F: FnMut(Handle, &KernelObject, &HandleMetadata),
    {
        let inner = self.inner.read();
        for (i, obj) in inner.handles.iter().enumerate() {
            if let Some(o) = obj.as_ref() {
                if let Some(m) = inner.metadata[i].as_ref() {
                    f(i as Handle, o, m);
                }
            }
        }
    }

    /// Get detailed information about a KernelObject for user space introspection
    pub fn get_object_info(&self, handle: Handle) -> Option<introspection::KernelObjectInfo> {
        let inner = self.inner.read();

        if handle as usize >= Self::MAX_HANDLES {
            return None;
        }

        let kernel_obj = inner.handles[handle as usize].as_ref()?;
        let metadata = inner.metadata[handle as usize].as_ref()?;
        let handle_role = introspection::HandleRole::from(metadata.handle_type.clone());
        let (readable, writable) = metadata.access_mode.into();

        match kernel_obj {
            KernelObject::File(_) => Some(introspection::KernelObjectInfo::for_file(
                handle_role,
                readable,
                writable,
            )),
            KernelObject::Pipe(_) => Some(introspection::KernelObjectInfo::for_pipe(
                handle_role,
                readable,
                writable,
            )),
            #[cfg(feature = "network")]
            KernelObject::Socket(_) => Some(introspection::KernelObjectInfo::for_socket(
                handle_role,
                readable,
                writable,
            )),
            KernelObject::EventChannel(_) => Some(
                introspection::KernelObjectInfo::for_event_channel(handle_role),
            ),
            KernelObject::EventSubscription(_) => Some(
                introspection::KernelObjectInfo::for_event_subscription(handle_role),
            ),
            KernelObject::SharedMemory(_) => Some(
                introspection::KernelObjectInfo::for_shared_memory(handle_role, readable, writable),
            ),
            KernelObject::Counter(_) => Some(introspection::KernelObjectInfo::for_counter(
                handle_role,
                readable,
                writable,
            )),
            KernelObject::Timer(_) => Some(introspection::KernelObjectInfo::for_timer(
                handle_role,
                readable,
                writable,
            )),
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVm(_) => None,
            #[cfg(feature = "hypervisor")]
            KernelObject::HypervisorVcpu(_) => None,
        }
    }

    /// Get access to the free_handles vector length for testing purposes
    #[cfg(test)]
    pub fn free_handles_len(&self) -> usize {
        self.inner.read().free_handles.len()
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
///
/// Clone implementation creates a shallow copy (Arc clone).
/// This means the cloned HandleTable shares the same underlying data.
/// Use `deep_clone()` to create an independent copy.
impl Clone for HandleTable {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

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
    /// Event channel for pub/sub communication
    EventChannel,
    /// Event subscription for receiving events
    EventSubscription,
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
    CloseOnExec, // Close on exec (O_CLOEXEC)
    NonBlocking, // Non-blocking mode (O_NONBLOCK)
    Append,      // Append mode (O_APPEND)
    Sync,        // Synchronous writes (O_SYNC)
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
