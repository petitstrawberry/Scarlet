//! KernelObject introspection types (user-space mirror)
//!
//! These definitions must match the kernel-side layout in
//! `kernel/src/object/introspection.rs`.

/// Information about a KernelObject that can be queried by user space
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelObjectInfo {
    /// The type of the underlying KernelObject
    pub object_type: KernelObjectType,
    /// Available capabilities for this object
    pub capabilities: ObjectCapabilities,
    /// Current handle metadata
    pub handle_role: HandleRole,
    /// Access permissions
    pub access_mode: u32,
}

impl KernelObjectInfo {
    pub const fn unknown() -> Self {
        Self {
            object_type: KernelObjectType::Unknown,
            capabilities: ObjectCapabilities::none(),
            handle_role: HandleRole::Regular,
            access_mode: 0,
        }
    }
}

/// Types of KernelObject that can be distinguished by user space
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelObjectType {
    /// Regular file object
    File = 1,
    /// Pipe object for IPC
    Pipe = 2,
    /// Event channel for pub/sub IPC
    EventChannel = 3,
    /// Event subscription for receiving events
    EventSubscription = 4,
    /// Character device (future)
    CharDevice = 5,
    /// Block device (future)
    BlockDevice = 6,
    /// Socket
    Socket = 7,
    /// Shared memory for IPC
    SharedMemory = 8,
    /// Unknown or unsupported type
    Unknown = 0,
}

/// Capabilities available for a KernelObject
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectCapabilities {
    /// Supports StreamOps (read/write)
    pub stream_ops: bool,
    /// Supports FileOps (seek, truncate, etc.)
    pub file_ops: bool,
    /// Supports PipeOps (pipe-specific operations)
    pub pipe_ops: bool,
    /// Supports Event capabilities
    pub event_ops: bool,
    /// Supports CloneOps (custom cloning)
    pub clone_ops: bool,
    /// Reserved for future capabilities
    pub reserved: [bool; 3],
}

impl ObjectCapabilities {
    pub const fn none() -> Self {
        Self {
            stream_ops: false,
            file_ops: false,
            pipe_ops: false,
            event_ops: false,
            clone_ops: false,
            reserved: [false; 3],
        }
    }
}

/// Handle role information (simplified from HandleType)
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleRole {
    /// Standard input/output stream
    StandardInputOutput = 1,
    /// Inter-process communication
    IpcChannel = 2,
    /// Regular usage
    Regular = 3,
}
