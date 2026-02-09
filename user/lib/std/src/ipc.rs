//! IPC (Inter-Process Communication) Module
//!
//! This module provides user-space interfaces for IPC mechanisms including
//! pipes, shared memory, and event channels.

use crate::handle::Handle;
use crate::handle::RawHandle;
use crate::syscall::{Syscall, syscall2};

/// Shared memory error type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedMemoryError {
    /// System call failed
    SyscallFailed,
    /// Invalid handle type
    InvalidHandle,
}

pub type SharedMemoryResult<T> = core::result::Result<T, SharedMemoryError>;

/// High-level SharedMemory wrapper with automatic resource management
///
/// Owns a kernel shared memory object handle. The handle is automatically
/// closed when the SharedMemory instance is dropped.
#[derive(Debug)]
pub struct SharedMemory {
    handle: Handle,
}

impl SharedMemory {
    /// Create a shared memory region
    pub fn create(size: usize, permissions: usize) -> SharedMemoryResult<Self> {
        let result = syscall2(Syscall::SharedMemoryCreate, size, permissions);
        if result == usize::MAX {
            return Err(SharedMemoryError::SyscallFailed);
        }
        let handle = unsafe { Handle::from_raw(result as i32) }
            .map_err(|_| SharedMemoryError::SyscallFailed)?;
        Ok(Self { handle })
    }

    /// Create a `SharedMemory` from an existing [`Handle`].
    ///
    /// This performs a type check using the handle's cached kernel object info.
    /// If the handle does not represent a shared memory object, this returns
    /// [`SharedMemoryError::InvalidHandle`] and does **not** consume the handle.
    pub fn from_handle(handle: Handle) -> SharedMemoryResult<Self> {
        handle
            .as_shared_memory()
            .map_err(|_| SharedMemoryError::InvalidHandle)?;
        Ok(Self { handle })
    }

    /// Get the underlying handle (for advanced usage)
    pub fn as_handle(&self) -> &Handle {
        &self.handle
    }

    /// Get the raw handle value
    pub fn as_raw(&self) -> RawHandle {
        self.handle.as_raw()
    }

    /// Get a `SharedMemoryObject` capability for this shared memory.
    ///
    /// This is fallible to avoid panicking when a `SharedMemory` wrapper was
    /// constructed from an unexpected handle type.
    pub fn as_object(
        &self,
    ) -> core::result::Result<crate::handle::capability::SharedMemoryObject<'_>, SharedMemoryError>
    {
        self.handle
            .as_shared_memory()
            .map_err(|_| SharedMemoryError::InvalidHandle)
    }

    /// Convert the SharedMemory into a Handle
    pub fn into_handle(self) -> Handle {
        self.handle
    }
}

/// Permissions for shared memory
pub mod permissions {
    /// Read permission
    pub const READ: usize = 0x1;
    /// Write permission
    pub const WRITE: usize = 0x2;
    /// Execute permission
    pub const EXECUTE: usize = 0x4;
    /// Read and write permissions
    pub const READ_WRITE: usize = READ | WRITE;
}

// === Scarlet Native Event System ===

/// Event handler function type
pub type EventHandler = extern "C" fn(event_info: &EventInfo);

/// Event information structure
#[repr(C)]
pub struct EventInfo {
    pub content_type: u8,
    pub content_data: [u64; 4],
}

/// Error type for event operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventError {
    SyscallFailed,
    InvalidHandler,
    InvalidEventType,
}

pub type EventResult<T> = core::result::Result<T, EventError>;

/// Event content types
pub mod event_types {
    pub const PROCESS_CONTROL: u8 = 0;
    pub const MESSAGE: u8 = 1;
    pub const NOTIFICATION: u8 = 2;
    pub const CUSTOM: u8 = 3;
}

/// Process control types
pub mod process_control {
    pub const TERMINATE: u32 = 0;
    pub const KILL: u32 = 1;
    pub const STOP: u32 = 2;
    pub const CONTINUE: u32 = 3;
    pub const INTERRUPT: u32 = 4;
    pub const QUIT: u32 = 5;
    pub const HANGUP: u32 = 6;
    pub const CHILD_EXIT: u32 = 7;
    pub const PIPE_BROKEN: u32 = 8;
    pub const ALARM: u32 = 9;
    pub const IO_READY: u32 = 10;
    pub const USER_START: u32 = 11;
}

/// Event mask operations
pub mod mask_ops {
    pub const BLOCK: u32 = 0;
    pub const UNBLOCK: u32 = 1;
    pub const BLOCK_ALL: u32 = 2;
    pub const CLEAR_ALL: u32 = 3;
}

/// Event mask kinds
pub mod mask_kinds {
    pub const PROCESS_CONTROL: u32 = 0;
    pub const NOTIFICATION: u32 = 1;
    pub const ALL: u32 = 2;
}

/// Register an event handler for a specific event content type
///
/// # Arguments
/// * `content_type` - Event content type (0=ProcessControl, 1=Message, 2=Notification, 3=Custom)
/// * `handler` - Handler function address
/// * `synchronous` - If true, handler is called synchronously
pub fn register_event_handler(
    content_type: u8,
    handler: EventHandler,
    synchronous: bool,
) -> EventResult<()> {
    use crate::syscall::{Syscall, syscall4};

    let result = syscall4(
        Syscall::EventHandlerRegister,
        content_type as usize,
        handler as usize,
        synchronous as usize,
        0, // is_default = false
    );

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Register a default event handler for unhandled events
pub fn register_default_handler(handler: EventHandler, synchronous: bool) -> EventResult<()> {
    use crate::syscall::{Syscall, syscall4};

    let result = syscall4(
        Syscall::EventHandlerRegister,
        0, // content_type doesn't matter for default
        handler as usize,
        synchronous as usize,
        1, // is_default = true
    );

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Unregister an event handler for a specific event content type
pub fn unregister_event_handler(content_type: u8) -> EventResult<()> {
    use crate::syscall::{Syscall, syscall1};

    let result = syscall1(Syscall::EventHandlerUnregister, content_type as usize);

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Set event mask to block specific events
pub fn event_mask_block(kind: u32, subtype: u32) -> EventResult<()> {
    use crate::syscall::{Syscall, syscall3};

    let result = syscall3(
        Syscall::EventMask,
        mask_ops::BLOCK as usize,
        kind as usize,
        subtype as usize,
    );

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Set event mask to unblock specific events
pub fn event_mask_unblock(kind: u32, subtype: u32) -> EventResult<()> {
    use crate::syscall::{Syscall, syscall3};

    let result = syscall3(
        Syscall::EventMask,
        mask_ops::UNBLOCK as usize,
        kind as usize,
        subtype as usize,
    );

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Block all events
pub fn event_mask_block_all() -> EventResult<()> {
    use crate::syscall::{Syscall, syscall1};

    let result = syscall1(Syscall::EventMask, mask_ops::BLOCK_ALL as usize);

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Unblock all events
pub fn event_mask_clear_all() -> EventResult<()> {
    use crate::syscall::{Syscall, syscall1};

    let result = syscall1(Syscall::EventMask, mask_ops::CLEAR_ALL as usize);

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Return from event handler (should be called by handler trampoline)
pub fn event_return() {
    use crate::syscall::Syscall;
    crate::arch::arch_syscall0(Syscall::EventReturn);
}
