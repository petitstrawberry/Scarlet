//! KernelObject introspection and capability discovery
//! 
//! This module provides types and functions for discovering KernelObject
//! types and capabilities at runtime, enabling type-safe user-space wrappers.

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

/// Types of KernelObject that can be distinguished by user space
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelObjectType {
    /// Regular file object
    File = 1,
    /// Pipe object for IPC
    Pipe = 2,
    /// Character device (future)
    CharDevice = 3,
    /// Block device (future)
    BlockDevice = 4,
    /// Socket (future)
    Socket = 5,
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
    /// Supports CloneOps (custom cloning)
    pub clone_ops: bool,
    /// Reserved for future capabilities
    pub reserved: [bool; 4],
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

impl KernelObjectInfo {
    /// Create info for a File KernelObject
    pub fn for_file(handle_role: HandleRole, readable: bool, writable: bool) -> Self {
        Self {
            object_type: KernelObjectType::File,
            capabilities: ObjectCapabilities {
                stream_ops: true,
                file_ops: true,
                pipe_ops: false,
                clone_ops: false,
                reserved: [false; 4],
            },
            handle_role,
            access_mode: Self::encode_access_mode(readable, writable),
        }
    }
    
    /// Create info for a Pipe KernelObject
    pub fn for_pipe(handle_role: HandleRole, readable: bool, writable: bool) -> Self {
        Self {
            object_type: KernelObjectType::Pipe,
            capabilities: ObjectCapabilities {
                stream_ops: true,
                file_ops: false,
                pipe_ops: true,
                clone_ops: true,
                reserved: [false; 4],
            },
            handle_role,
            access_mode: Self::encode_access_mode(readable, writable),
        }
    }
    
    /// Create info for unknown KernelObject
    pub fn unknown() -> Self {
        Self {
            object_type: KernelObjectType::Unknown,
            capabilities: ObjectCapabilities {
                stream_ops: false,
                file_ops: false,
                pipe_ops: false,
                clone_ops: false,
                reserved: [false; 4],
            },
            handle_role: HandleRole::Regular,
            access_mode: 0,
        }
    }
    
    fn encode_access_mode(readable: bool, writable: bool) -> u32 {
        let mut mode = 0;
        if readable { mode |= 0x1; }
        if writable { mode |= 0x2; }
        mode
    }
}

/// Convert from HandleType to HandleRole for user space
impl From<crate::object::handle::HandleType> for HandleRole {
    fn from(handle_type: crate::object::handle::HandleType) -> Self {
        match handle_type {
            crate::object::handle::HandleType::StandardInputOutput(_) => HandleRole::StandardInputOutput,
            crate::object::handle::HandleType::IpcChannel => HandleRole::IpcChannel,
            crate::object::handle::HandleType::Regular => HandleRole::Regular,
        }
    }
}

/// Convert from AccessMode to boolean flags
impl From<crate::object::handle::AccessMode> for (bool, bool) {
    fn from(access_mode: crate::object::handle::AccessMode) -> Self {
        match access_mode {
            crate::object::handle::AccessMode::ReadOnly => (true, false),
            crate::object::handle::AccessMode::WriteOnly => (false, true),
            crate::object::handle::AccessMode::ReadWrite => (true, true),
        }
    }
}
