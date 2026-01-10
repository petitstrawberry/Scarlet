//! Scarlet Window Server (SWS) IPC protocol.
//!
//! This crate is the single source of truth for both the SWS server (`sws`)
//! and clients (`sws_client`) for message IDs, framing, and parsing.
//!
//! Wire format
//! -----------
//! Each message is framed as:
//! - Header (8 bytes, little-endian)
//!   - `msg_type: u32`
//!   - `payload_size: u32`
//! - Payload (`payload_size` bytes)
//!
//! See `docs/sws_ipc_protocol.md` for the detailed specification.

#![no_std]

extern crate scarlet_std as std;

use std::vec::Vec;

/// Maximum payload we accept from the socket.
///
/// This prevents unbounded allocations on malformed frames.
pub const MAX_PAYLOAD_SIZE: usize = 1024 * 1024; // 1 MiB

/// Message type IDs (client -> server).
pub mod client_msg {
    pub const CREATE_WINDOW: u32 = 1;
    pub const DESTROY_WINDOW: u32 = 2;
    pub const SET_WINDOW_TITLE: u32 = 3;
    pub const UPDATE_BUFFER: u32 = 4;
    pub const REQUEST_MOVE_WINDOW: u32 = 5;
    pub const MOVE_WINDOW: u32 = 6;
}

/// Message type IDs (server -> client).
pub mod server_msg {
    pub const WINDOW_CREATED: u32 = 10;
    pub const WINDOW_DESTROYED: u32 = 11;
    pub const INPUT_EVENT: u32 = 12;
    pub const ERROR: u32 = 13;
}

/// Message header.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageHeader {
    pub msg_type: u32,
    pub payload_size: u32,
}

impl MessageHeader {
    pub const SIZE: usize = 8;

    pub fn to_le_bytes(self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        out[0..4].copy_from_slice(&self.msg_type.to_le_bytes());
        out[4..8].copy_from_slice(&self.payload_size.to_le_bytes());
        out
    }

    pub fn from_le_bytes(bytes: [u8; Self::SIZE]) -> Self {
        let msg_type = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let payload_size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        Self {
            msg_type,
            payload_size,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    /// Frame payload is too large.
    PayloadTooLarge,
    /// Malformed payload for the given message type.
    MalformedPayload,
    /// Unknown message type.
    UnknownMessageType,
}

/// Encode a full framed message (header + payload) into a single buffer.
///
/// This is a protocol-only helper; actual I/O is implemented by server/client code.
pub fn encode_frame(msg_type: u32, payload: &[u8]) -> Vec<u8> {
    let header = MessageHeader {
        msg_type,
        payload_size: payload.len() as u32,
    };
    let mut out = Vec::new();
    out.extend_from_slice(&header.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// Borrowed client->server messages (payload may be borrowed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientMessageRef<'a> {
    CreateWindow {
        width: u32,
        height: u32,
    },
    DestroyWindow {
        window_id: u32,
    },
    SetWindowTitle {
        window_id: u32,
        title: &'a [u8],
    },
    UpdateBuffer {
        window_id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    RequestMoveWindow {
        window_id: u32,
    },
    MoveWindow {
        window_id: u32,
        x: i32,
        y: i32,
    },
}

/// Server->client messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMessage {
    WindowCreated {
        window_id: u32,
        shm_size: u64,
    },
    WindowDestroyed {
        window_id: u32,
    },
    InputEvent {
        time: u64,
        type_: u16,
        code: u16,
        value: i32,
    },
    Error {
        code: u32,
    },
}

/// Parse a client->server message from `(msg_type, payload)`.
pub fn parse_client_message<'a>(
    msg_type: u32,
    payload: &'a [u8],
) -> Result<ClientMessageRef<'a>, ProtocolError> {
    match msg_type {
        client_msg::CREATE_WINDOW => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let width = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let height = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Ok(ClientMessageRef::CreateWindow { width, height })
        }
        client_msg::DESTROY_WINDOW => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::DestroyWindow { window_id })
        }
        client_msg::SET_WINDOW_TITLE => {
            if payload.len() < 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let title_len =
                u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
            if payload.len() != 8 + title_len {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::SetWindowTitle {
                window_id,
                title: &payload[8..],
            })
        }
        client_msg::UPDATE_BUFFER => {
            if payload.len() != 20 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let x = i32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let y = i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            let width = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
            let height = u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
            Ok(ClientMessageRef::UpdateBuffer {
                window_id,
                x,
                y,
                width,
                height,
            })
        }
        client_msg::REQUEST_MOVE_WINDOW => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::RequestMoveWindow { window_id })
        }
        client_msg::MOVE_WINDOW => {
            if payload.len() != 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let x = i32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let y = i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            Ok(ClientMessageRef::MoveWindow { window_id, x, y })
        }
        _ => Err(ProtocolError::UnknownMessageType),
    }
}

/// Parse a server->client message from `(msg_type, payload)`.
pub fn parse_server_message(msg_type: u32, payload: &[u8]) -> Result<ServerMessage, ProtocolError> {
    match msg_type {
        server_msg::WINDOW_CREATED => {
            if payload.len() != 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let shm_size = u64::from_le_bytes([
                payload[4],
                payload[5],
                payload[6],
                payload[7],
                payload[8],
                payload[9],
                payload[10],
                payload[11],
            ]);
            Ok(ServerMessage::WindowCreated {
                window_id,
                shm_size,
            })
        }
        server_msg::WINDOW_DESTROYED => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ServerMessage::WindowDestroyed { window_id })
        }
        server_msg::INPUT_EVENT => {
            if payload.len() != 16 {
                return Err(ProtocolError::MalformedPayload);
            }
            let time = u64::from_le_bytes([
                payload[0], payload[1], payload[2], payload[3], payload[4], payload[5], payload[6],
                payload[7],
            ]);
            let type_ = u16::from_le_bytes([payload[8], payload[9]]);
            let code = u16::from_le_bytes([payload[10], payload[11]]);
            let value = i32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
            Ok(ServerMessage::InputEvent {
                time,
                type_,
                code,
                value,
            })
        }
        server_msg::ERROR => {
            if payload.len() < 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let code = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ServerMessage::Error { code })
        }
        _ => Err(ProtocolError::UnknownMessageType),
    }
}

/// Build payload for client->server `CREATE_WINDOW`.
pub fn payload_create_window(width: u32, height: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&width.to_le_bytes());
    payload[4..8].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for client->server `DESTROY_WINDOW`.
pub fn payload_destroy_window(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for client->server `SET_WINDOW_TITLE`.
pub fn payload_set_window_title(window_id: u32, title: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&window_id.to_le_bytes());
    out.extend_from_slice(&(title.len() as u32).to_le_bytes());
    out.extend_from_slice(title);
    out
}

/// Build payload for client->server `UPDATE_BUFFER` (damage notification).
pub fn payload_update_buffer(window_id: u32, x: i32, y: i32, width: u32, height: u32) -> [u8; 20] {
    let mut payload = [0u8; 20];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&x.to_le_bytes());
    payload[8..12].copy_from_slice(&y.to_le_bytes());
    payload[12..16].copy_from_slice(&width.to_le_bytes());
    payload[16..20].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for client->server `REQUEST_MOVE_WINDOW`.
pub fn payload_request_move_window(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for client->server `MOVE_WINDOW`.
pub fn payload_move_window(window_id: u32, x: i32, y: i32) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&x.to_le_bytes());
    payload[8..12].copy_from_slice(&y.to_le_bytes());
    payload
}

/// Build payload for server->client `WINDOW_CREATED`.
pub fn payload_window_created(window_id: u32, shm_size: u64) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..12].copy_from_slice(&shm_size.to_le_bytes());
    payload
}

/// Build payload for server->client `WINDOW_DESTROYED`.
pub fn payload_window_destroyed(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for server->client `INPUT_EVENT`.
pub fn payload_input_event(time: u64, type_: u16, code: u16, value: i32) -> [u8; 16] {
    let mut payload = [0u8; 16];
    payload[0..8].copy_from_slice(&time.to_le_bytes());
    payload[8..10].copy_from_slice(&type_.to_le_bytes());
    payload[10..12].copy_from_slice(&code.to_le_bytes());
    payload[12..16].copy_from_slice(&value.to_le_bytes());
    payload
}

/// Build payload for server->client `ERROR`.
pub fn payload_error(code: u32) -> [u8; 4] {
    code.to_le_bytes()
}
