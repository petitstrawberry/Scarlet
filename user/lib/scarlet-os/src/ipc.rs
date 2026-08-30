//! IPC (Inter-Process Communication) Module
//!
//! This module provides user-space interfaces for IPC mechanisms including
//! pipes, shared memory, and event channels.

use crate::handle::Handle;
use crate::handle::RawHandle;
use scarlet_sys::{Syscall, syscall1, syscall2};

/// Errors returned while creating a native pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeError {
    /// The kernel rejected pipe creation.
    SyscallFailed,
    /// A returned endpoint could not be adopted as a valid handle.
    InvalidHandle,
}

/// Result type for native pipe creation.
pub type PipeResult<T> = core::result::Result<T, PipeError>;

/// Create a unidirectional native pipe.
///
/// # Returns
///
/// An owning `(read_end, write_end)` handle pair, or a pipe creation error.
pub fn pipe() -> PipeResult<(Handle, Handle)> {
    let mut pipe_handles = [0u32; 2];
    let result = syscall2(Syscall::Pipe, pipe_handles.as_mut_ptr() as usize, 0);
    if result == usize::MAX {
        return Err(PipeError::SyscallFailed);
    }

    // SAFETY: a successful `Pipe` syscall returns two newly owned endpoints;
    // this call transfers exclusive ownership of the read endpoint.
    let read_handle = match unsafe { Handle::from_raw(pipe_handles[0] as RawHandle) } {
        Ok(handle) => handle,
        Err(_) => {
            // `from_raw` consumed the read endpoint. The write endpoint has not
            // been adopted yet, so close it explicitly before returning.
            let _ = syscall1(Syscall::HandleClose, pipe_handles[1] as usize);
            return Err(PipeError::InvalidHandle);
        }
    };
    // SAFETY: the write endpoint is still exclusively owned here and has not
    // previously been adopted or closed.
    let write_handle = match unsafe { Handle::from_raw(pipe_handles[1] as RawHandle) } {
        Ok(handle) => handle,
        Err(_) => {
            // `from_raw` consumed the write endpoint; dropping `read_handle`
            // closes the remaining endpoint exactly once.
            return Err(PipeError::InvalidHandle);
        }
    };

    Ok((read_handle, write_handle))
}

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

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    ".section .text.scarlet_event_return,\"ax\",@progbits",
    ".global __scarlet_event_return_trampoline",
    ".type __scarlet_event_return_trampoline,@function",
    "__scarlet_event_return_trampoline:",
    "    addi a7, x0, 643",
    "    ecall",
    "    ebreak",
    ".size __scarlet_event_return_trampoline, .-__scarlet_event_return_trampoline",
);

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    ".section .text.scarlet_event_return,\"ax\"",
    ".global __scarlet_event_return_trampoline",
    ".type __scarlet_event_return_trampoline,%function",
    "__scarlet_event_return_trampoline:",
    "    mov x8, #643",
    "    svc #0",
    "    brk #0",
    ".size __scarlet_event_return_trampoline, .-__scarlet_event_return_trampoline",
);

#[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
unsafe extern "C" {
    fn __scarlet_event_return_trampoline();
}

#[cfg(any(target_arch = "riscv64", target_arch = "aarch64"))]
fn event_return_trampoline() -> usize {
    __scarlet_event_return_trampoline as *const () as usize
}

#[cfg(not(any(target_arch = "riscv64", target_arch = "aarch64")))]
fn event_return_trampoline() -> usize {
    0
}

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
    pub const TERMINAL_STOP: u32 = 11;
    pub const TERMINAL_INPUT: u32 = 12;
    pub const TERMINAL_OUTPUT: u32 = 13;
    pub const WINDOW_CHANGE: u32 = 14;
    pub const USER_START: u32 = 256;
}

/// Native process-control events.
///
/// These values are Scarlet Event subtypes, not POSIX signal numbers.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessControl {
    Terminate = 0,
    Kill = 1,
    Stop = 2,
    Continue = 3,
    Interrupt = 4,
    Quit = 5,
    Hangup = 6,
    ChildExit = 7,
    PipeBroken = 8,
    Alarm = 9,
    IoReady = 10,
    TerminalStop = 11,
    TerminalInput = 12,
    TerminalOutput = 13,
    WindowChange = 14,
}

/// Event delivery priority.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPriority {
    Low = 1,
    Normal = 2,
    High = 3,
    Critical = 4,
}

/// Send a process-control event directly to a task.
///
/// # Arguments
///
/// * `task_id` - Namespace-local task ID.
/// * `control` - Process-control event to deliver.
pub fn send_process_control(task_id: u32, control: ProcessControl) -> EventResult<()> {
    send_process_control_with_priority(task_id, control, EventPriority::High, true)
}

/// Send a process-control event directly to a task with delivery options.
///
/// # Arguments
///
/// * `task_id` - Namespace-local task ID.
/// * `control` - Process-control event to deliver.
/// * `priority` - Event delivery priority.
/// * `reliable` - Whether the kernel should use reliable delivery.
pub fn send_process_control_with_priority(
    task_id: u32,
    control: ProcessControl,
    priority: EventPriority,
    reliable: bool,
) -> EventResult<()> {
    use scarlet_sys::{Syscall, syscall4};

    let result = syscall4(
        Syscall::EventSendDirect,
        task_id as usize,
        control as usize,
        reliable as usize,
        priority as usize,
    );

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Send a process-control event to a process group.
///
/// # Arguments
///
/// * `process_group_id` - Namespace-local process group ID. `None` targets the
///   caller's process group.
/// * `control` - Process-control event to deliver.
pub fn send_process_control_to_group(
    process_group_id: Option<u32>,
    control: ProcessControl,
) -> EventResult<()> {
    send_process_control_to_group_with_priority(
        process_group_id,
        control,
        EventPriority::High,
        true,
    )
}

/// Send a process-control event to a process group with delivery options.
///
/// # Arguments
///
/// * `process_group_id` - Namespace-local process group ID. `None` targets the
///   caller's process group.
/// * `control` - Process-control event to deliver.
/// * `priority` - Event delivery priority.
/// * `reliable` - Whether the kernel should use reliable delivery.
pub fn send_process_control_to_group_with_priority(
    process_group_id: Option<u32>,
    control: ProcessControl,
    priority: EventPriority,
    reliable: bool,
) -> EventResult<()> {
    use scarlet_sys::{Syscall, syscall4};

    let result = syscall4(
        Syscall::EventSendGroup,
        process_group_id.unwrap_or(0) as usize,
        control as usize,
        reliable as usize,
        priority as usize,
    );

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
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
    use scarlet_sys::{Syscall, syscall5};

    // Validate content_type range (0-3)
    if content_type > 3 {
        return Err(EventError::InvalidEventType);
    }

    let restorer = event_return_trampoline();
    if restorer == 0 {
        return Err(EventError::SyscallFailed);
    }

    let result = syscall5(
        Syscall::EventHandlerRegisterWithRestorer,
        content_type as usize,
        handler as usize,
        synchronous as usize,
        0, // is_default = false
        restorer,
    );

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Register a default event handler for unhandled events
pub fn register_default_handler(handler: EventHandler, synchronous: bool) -> EventResult<()> {
    use scarlet_sys::{Syscall, syscall5};

    let restorer = event_return_trampoline();
    if restorer == 0 {
        return Err(EventError::SyscallFailed);
    }

    let result = syscall5(
        Syscall::EventHandlerRegisterWithRestorer,
        0, // content_type doesn't matter for default
        handler as usize,
        synchronous as usize,
        1, // is_default = true
        restorer,
    );

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Unregister an event handler for a specific event content type
pub fn unregister_event_handler(content_type: u8) -> EventResult<()> {
    use scarlet_sys::{Syscall, syscall1};

    let result = syscall1(Syscall::EventHandlerUnregister, content_type as usize);

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Set event mask to block specific events
pub fn event_mask_block(kind: u32, subtype: u32) -> EventResult<()> {
    use scarlet_sys::{Syscall, syscall3};

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
    use scarlet_sys::{Syscall, syscall3};

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
    use scarlet_sys::{Syscall, syscall1};

    let result = syscall1(Syscall::EventMask, mask_ops::BLOCK_ALL as usize);

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Unblock all events
pub fn event_mask_clear_all() -> EventResult<()> {
    use scarlet_sys::{Syscall, syscall1};

    let result = syscall1(Syscall::EventMask, mask_ops::CLEAR_ALL as usize);

    if result == usize::MAX {
        Err(EventError::SyscallFailed)
    } else {
        Ok(())
    }
}

/// Return from event handler (should be called by handler trampoline)
pub fn event_return() {
    use scarlet_sys::Syscall;
    scarlet_sys::syscall0(Syscall::EventReturn);
}
