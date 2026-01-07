//! Inter-Process Communication (IPC) module
//!
//! This module provides various IPC mechanisms for Scarlet OS:
//! - Stream IPC: Pipes and data streams (StreamIpcOps-based)
//! - Event IPC: Event distribution with 4 delivery modes (EventOps-based)
//!   - Immediate: Force delivery (Signal-like)
//!   - Notification: Best-effort delivery
//!   - Subscription: Channel-based pub/sub
//!   - Group: Broadcast delivery
//! - Message Queues: Structured message passing (future)
//! - Shared Memory: Memory-based communication
//! - Sockets: Network and local communication endpoints (future)

use crate::object::capability::{StreamError, StreamOps};
use alloc::string::String;

pub mod event;
pub mod pipe;
pub mod shared_memory;
pub mod syscall;

/// Represents errors specific to IPC operations
#[derive(Debug, Clone)]
pub enum IpcError {
    /// The other end of the communication channel has been closed
    PeerClosed,
    /// The IPC channel is full (for bounded channels)
    ChannelFull,
    /// The IPC channel is empty (for non-blocking reads)
    ChannelEmpty,
    /// Invalid IPC object state
    InvalidState,
    /// Operation not supported by this IPC type
    NotSupported,
    /// General stream error
    StreamError(StreamError),
    /// Custom error message
    Other(String),
}

impl From<StreamError> for IpcError {
    fn from(stream_err: StreamError) -> Self {
        IpcError::StreamError(stream_err)
    }
}

/// Common trait for stream-based IPC objects
///
/// This trait provides common functionality for stream-based IPC mechanisms
/// that operate as continuous data flows, such as pipes and sockets.
/// It extends StreamOps with stream-specific IPC state management.
pub trait StreamIpcOps: StreamOps {
    /// Check if the stream IPC object is still connected/valid
    fn is_connected(&self) -> bool;

    /// Get the number of active peers (readers/writers/endpoints)
    fn peer_count(&self) -> usize;

    /// Get a human-readable description of this IPC object
    fn description(&self) -> String;

    /// Send a KernelObject handle through this IPC channel
    ///
    /// This enables passing kernel objects (like SharedMemoryObject) between tasks
    /// through a socket or pipe connection, similar to Unix SCM_RIGHTS functionality.
    /// Uses dup() semantics - the object is duplicated with proper reference counting,
    /// ensuring that objects like Pipes correctly track reader/writer counts.
    ///
    /// # Arguments
    /// * `object` - The KernelObject to transfer to the peer (already duplicated)
    ///
    /// # Returns
    /// * `Ok(())` if the handle was queued successfully
    /// * `Err(IpcError)` if the operation failed
    fn send_handle(&self, object: crate::object::KernelObject) -> Result<(), IpcError> {
        let _ = object;
        Err(IpcError::NotSupported)
    }

    /// Receive a KernelObject handle from this IPC channel
    ///
    /// Retrieves a KernelObject that was sent by the peer via send_handle().
    ///
    /// # Returns
    /// * `Ok(KernelObject)` if a handle was available
    /// * `Err(IpcError::ChannelEmpty)` if no handles are available
    /// * `Err(IpcError)` for other errors
    fn recv_handle(&self) -> Result<crate::object::KernelObject, IpcError> {
        Err(IpcError::NotSupported)
    }
}

// Future IPC trait definitions:

/// Event channel operations (implements EventSender + EventReceiver capabilities)
///
/// This trait defines objects that provide event-based communication
/// channels with pub/sub semantics, different from stream-based pipes.
pub trait EventIpcChannelObject: Send + Sync {
    /// Get channel identifier/name
    fn channel_id(&self) -> String;

    /// Check if channel is active
    fn is_active(&self) -> bool;

    /// Get number of subscribers
    fn subscriber_count(&self) -> usize;
}

/// Message queue operations (future implementation)
pub trait MessageQueueObject: StreamIpcOps {
    // Message-based communication methods will be defined here
}

/// Socket operations (future implementation)
pub trait SocketObject: StreamIpcOps {
    // Socket-specific methods will be defined here
}

// Re-export commonly used types
pub use event::{
    Event, EventContent, EventDelivery, EventError, EventManager, EventPayload, GroupTarget,
};
pub use pipe::{PipeEndpoint, PipeError, PipeObject, UnidirectionalPipe};
pub use shared_memory::{SharedMemory, SharedMemoryObject};
