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
//! - Shared Memory: Memory-based communication (future)
//! - Sockets: Network and local communication endpoints (future)

use crate::object::capability::{StreamOps, StreamError};
use alloc::string::String;

pub mod pipe;
pub mod event;
pub mod event_objects;
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
}

// Future IPC trait definitions:

/// Event channel operations (implements EventIpcOps capability)
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

/// Shared memory operations (future implementation)
pub trait SharedMemoryObject: StreamIpcOps {
    // Shared memory methods will be defined here
}

/// Socket operations (future implementation)
pub trait SocketObject: StreamIpcOps {
    // Socket-specific methods will be defined here
}

// Re-export commonly used types
pub use pipe::{PipeEndpoint, UnidirectionalPipe, PipeError, PipeObject};
pub use event::{EventManager, EventOps, Event, EventType, EventError, EventPayload, GroupTarget};
