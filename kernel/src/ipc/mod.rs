//! Inter-Process Communication (IPC) module
//! 
//! This module provides various IPC mechanisms for Scarlet OS:
//! - Pipes: Unidirectional and bidirectional data streams
//! - Message Queues: Structured message passing
//! - Shared Memory: Memory-based communication
//! - Sockets: Network and local communication endpoints
//! - Events: Synchronization and notification primitives
//! - Semaphores: Resource counting and synchronization

use crate::object::capability::{StreamOps, StreamError};
use alloc::string::String;

pub mod pipe;
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

/// Common trait for all IPC objects
/// 
/// This trait provides common functionality that all IPC mechanisms share,
/// such as connection state management and peer information.
pub trait IpcObject: StreamOps {
    /// Check if the IPC object is still connected/valid
    fn is_connected(&self) -> bool;
    
    /// Get the number of active peers (readers/writers/endpoints)
    fn peer_count(&self) -> usize;
    
    /// Get a human-readable description of this IPC object
    fn description(&self) -> String;
}

// Future IPC trait definitions:

/// Message queue operations (future implementation)
pub trait MessageQueueObject: IpcObject {
    // Message-based communication methods will be defined here
}

/// Shared memory operations (future implementation)
pub trait SharedMemoryObject: IpcObject {
    // Shared memory methods will be defined here
}

/// Socket operations (future implementation)
pub trait SocketObject: IpcObject {
    // Socket-specific methods will be defined here
}

// Re-export commonly used types
pub use pipe::{UnidirectionalPipe, PipeError, PipeObject};
