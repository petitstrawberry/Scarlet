//! IPC Protocol definitions for Scarlet Window Server

use std::vec::Vec;

/// Client-to-Server messages
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum ClientMessage {
    /// Create a new window
    CreateWindow {
        width: u32,
        height: u32,
    },
    /// Destroy a window
    DestroyWindow {
        window_id: u32,
    },
    /// Set window title
    SetWindowTitle {
        window_id: u32,
        title_len: u32,
        // title data follows
    },
    /// Update window buffer (client has drawn new content)
    UpdateBuffer {
        window_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    /// Request window move (for drag operation)
    RequestMoveWindow {
        window_id: u32,
    },
    /// Move window to specific position
    MoveWindow {
        window_id: u32,
        x: i32,
        y: i32,
    },
}

/// Server-to-Client messages
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum ServerMessage {
    /// Window created successfully
    WindowCreated {
        window_id: u32,
        shm_size: usize,
        // Shared memory key follows
    },
    /// Window destroyed
    WindowDestroyed {
        window_id: u32,
    },
    /// Input event for focused window
    InputEvent {
        time: u64,
        type_: u16,
        code: u16,
        value: i32,
    },
    /// Error occurred
    Error {
        code: u32,
        // error message follows
    },
}

/// Message header for variable-length messages
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MessageHeader {
    pub msg_type: u32,
    pub payload_size: u32,
}

impl ClientMessage {
    /// Get message type ID
    pub fn type_id(&self) -> u32 {
        match self {
            ClientMessage::CreateWindow { .. } => 1,
            ClientMessage::DestroyWindow { .. } => 2,
            ClientMessage::SetWindowTitle { .. } => 3,
            ClientMessage::UpdateBuffer { .. } => 4,
            ClientMessage::RequestMoveWindow { .. } => 5,
            ClientMessage::MoveWindow { .. } => 6,
        }
    }
}

impl ServerMessage {
    /// Get message type ID
    pub fn type_id(&self) -> u32 {
        match self {
            ServerMessage::WindowCreated { .. } => 1,
            ServerMessage::WindowDestroyed { .. } => 2,
            ServerMessage::InputEvent { .. } => 3,
            ServerMessage::Error { .. } => 4,
        }
    }
}
