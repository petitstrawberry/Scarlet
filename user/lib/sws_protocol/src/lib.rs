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
    pub const SET_WINDOW_PARENT: u32 = 7;
    pub const SET_WINDOW_TRANSIENT_FLAGS: u32 = 8;
    pub const RESIZE_WINDOW: u32 = 9;
    pub const GET_SCREEN_SIZE: u32 = 10;
    pub const SET_WINDOW_SIZE_LIMITS: u32 = 16;
    pub const MINIMIZE_WINDOW: u32 = 17;
    pub const MAXIMIZE_WINDOW: u32 = 18;
    pub const RESTORE_WINDOW: u32 = 19;
    pub const SET_WINDOW_TYPE: u32 = 20;
    pub const SET_WINDOW_OPACITY: u32 = 21;
    pub const SET_WORKAREA: u32 = 22;
    pub const SET_WINDOW_RESIZABLE: u32 = 23;
    pub const GET_WINDOW_LIST: u32 = 24;
    pub const LAUNCH_OR_FOCUS: u32 = 25;
    pub const FOCUS_WINDOW: u32 = 26;
    pub const GET_ACTIVE_APP: u32 = 27; // Get active app info for TaskBar
    pub const SET_WINDOW_HAS_ALPHA_CONTENT: u32 = 28; // Set whether window content has alpha channel
}

/// Message type IDs (server -> client).
pub mod server_msg {
    pub const WINDOW_CREATED: u32 = 10;
    pub const WINDOW_DESTROYED: u32 = 11;
    pub const INPUT_EVENT: u32 = 12;
    pub const ERROR: u32 = 13;
    pub const WINDOW_RESIZED: u32 = 14;
    pub const WINDOW_CONFIGURE: u32 = 15;
    pub const SCREEN_SIZE: u32 = 16;
    pub const WINDOW_LIST: u32 = 17;
    pub const FOCUS_CHANGED: u32 = 18;
    pub const ACTIVE_APP: u32 = 19; // Response to GET_ACTIVE_APP
}

/// Flags for transient (parent/child) window behavior.
///
/// These flags are interpreted by the compositor as *policy hints*.
pub mod transient_flags {
    /// If set, the child moves together when its parent is moved.
    pub const FOLLOW_PARENT_MOVE: u32 = 1 << 0;
    /// If set, raising the parent raises the child group.
    pub const RAISE_WITH_PARENT: u32 = 1 << 1;
}

/// Window type constants for Z-order management
pub mod window_types {
    /// Normal application window (default)
    pub const NORMAL: u32 = 0;
    /// Window that always stays on top
    pub const ALWAYS_ON_TOP: u32 = 1;
    /// Taskbar or panel window
    pub const TASKBAR: u32 = 2;
    /// Desktop background window
    pub const DESKTOP: u32 = 3;
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
        app_id: &'a [u8],
        app_name: &'a [u8],
        menu_titles: &'a [u8], // Format: "menu1|menu2|menu3"
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
    /// Set (or clear) the logical parent of a window.
    ///
    /// `parent_id == 0` means "no parent".
    SetWindowParent {
        window_id: u32,
        parent_id: u32,
    },

    /// Configure transient behavior flags for a window.
    ///
    /// Flags are a bitset from `transient_flags::*`.
    SetWindowTransientFlags {
        window_id: u32,
        flags: u32,
    },

    /// Resize a window buffer.
    ///
    /// This triggers the server to allocate a new shared-memory buffer and
    /// respond with `WINDOW_RESIZED` + a new SHM handle.
    ResizeWindow {
        window_id: u32,
        width: u32,
        height: u32,
    },

    /// Set min/max size constraints for a window.
    ///
    /// Values are in pixels.
    /// - `min_* == 0` means "no minimum".
    /// - `max_* == 0` means "no maximum".
    SetWindowSizeLimits {
        window_id: u32,
        min_width: u32,
        min_height: u32,
        max_width: u32,
        max_height: u32,
    },

    /// Minimize a window (hide but keep in window list)
    MinimizeWindow {
        window_id: u32,
    },

    /// Maximize a window to screen dimensions
    MaximizeWindow {
        window_id: u32,
    },

    /// Restore a window from minimized or maximized state
    RestoreWindow {
        window_id: u32,
    },

    /// Set window type for Z-order management
    /// Type: 0 = Normal, 1 = AlwaysOnTop, 2 = Taskbar, 3 = Desktop
    SetWindowType {
        window_id: u32,
        window_type: u32,
    },

    /// Set window opacity (0-255, where 255 is fully opaque)
    SetWindowOpacity {
        window_id: u32,
        opacity: u8,
    },

    /// Set the workarea (usable screen area) for the window manager
    ///
    /// This is typically sent by the taskbar to inform the window manager
    /// about the area where normal windows should be placed.
    SetWorkarea {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },

    /// Set whether a window can be resized by the user via interactive resize
    SetWindowResizable {
        window_id: u32,
        resizable: bool,
    },

    /// Get the screen size
    GetScreenSize {},

    /// Get list of all windows
    GetWindowList {},

    /// Launch an application or focus an existing window
    ///
    /// If a window with the given app_id already exists, focus it.
    /// Otherwise, launch the specified application.
    LaunchOrFocus {
        app_id: &'a [u8],
        exec_path: &'a [u8],
    },

    /// Focus and raise a specific window.
    FocusWindow {
        window_id: u32,
    },

    /// Get active application information (for TaskBar)
    GetActiveApp {},

    /// Set whether window content contains alpha channel (semi-transparent pixels)
    ///
    /// This is separate from window.opacity - this controls whether pixel alpha
    /// values in the window buffer should be respected during composition.
    SetWindowHasAlphaContent {
        window_id: u32,
        has_alpha: bool,
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
    /// Server acknowledged a resize and provides the new SHM size.
    ///
    /// The server will send the new SHM handle out-of-band immediately after.
    WindowResized {
        window_id: u32,
        shm_size: u64,
        width: u32,
        height: u32,
    },
    /// Compositor requests the client to resize to the given dimensions.
    ///
    /// This does not include a new SHM handle; clients should respond by
    /// issuing a `RESIZE_WINDOW` request.
    WindowConfigure {
        window_id: u32,
        width: u32,
        height: u32,
    },
    InputEvent {
        window_id: u32,
        time: u64,
        type_: u16,
        code: u16,
        value: i32,
    },
    /// Response to GET_SCREEN_SIZE request
    ScreenSize {
        width: u32,
        height: u32,
    },
    /// Response to GET_WINDOW_LIST request
    /// Contains a serialized list of windows
    WindowList,
    /// Focus changed to a different window (includes app info for TaskBar)
    FocusChanged {
        window_id: u32,
        app_id: [u8; 128],
        app_id_len: u32,
        app_name: [u8; 128],
        app_name_len: u32,
        title: [u8; 256],
        title_len: u32,
        menu_titles: [u8; 512], // Format: "menu1|menu2|menu3"
        menu_titles_len: u32,
    },
    /// Active application information (response to GET_ACTIVE_APP)
    ActiveApp {
        app_id: [u8; 128],
        app_id_len: u32,
        app_name: [u8; 128],
        app_name_len: u32,
        menu_titles: [u8; 512], // Format: "menu1|menu2|menu3"
        menu_titles_len: u32,
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
            // Payload: app_id_len (u32) + app_id_bytes + app_name_len (u32) + app_name_bytes
            //          + menu_titles_len (u32) + menu_titles_bytes + width (u32) + height (u32)
            if payload.len() < 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            let app_id_len =
                u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;

            let mut offset = 4 + app_id_len;
            if payload.len() < offset + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let app_name_len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;
            offset += 4 + app_name_len;

            if payload.len() < offset + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let menu_titles_len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;
            offset += 4 + menu_titles_len;

            if payload.len() != offset + 8 {
                return Err(ProtocolError::MalformedPayload);
            }

            let app_id = &payload[4..4 + app_id_len];
            let app_name = &payload[4 + app_id_len + 4..4 + app_id_len + 4 + app_name_len];
            let menu_titles = &payload[4 + app_id_len + 4 + app_name_len + 4
                ..4 + app_id_len + 4 + app_name_len + 4 + menu_titles_len];
            let width = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]);
            let height = u32::from_le_bytes([
                payload[offset + 4],
                payload[offset + 5],
                payload[offset + 6],
                payload[offset + 7],
            ]);
            Ok(ClientMessageRef::CreateWindow {
                app_id,
                app_name,
                menu_titles,
                width,
                height,
            })
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
        client_msg::SET_WINDOW_PARENT => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let parent_id = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Ok(ClientMessageRef::SetWindowParent {
                window_id,
                parent_id,
            })
        }
        client_msg::SET_WINDOW_TRANSIENT_FLAGS => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let flags = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Ok(ClientMessageRef::SetWindowTransientFlags { window_id, flags })
        }
        client_msg::RESIZE_WINDOW => {
            if payload.len() != 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let width = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let height = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            Ok(ClientMessageRef::ResizeWindow {
                window_id,
                width,
                height,
            })
        }
        client_msg::SET_WINDOW_SIZE_LIMITS => {
            if payload.len() != 20 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let min_width = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let min_height = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            let max_width =
                u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
            let max_height =
                u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
            Ok(ClientMessageRef::SetWindowSizeLimits {
                window_id,
                min_width,
                min_height,
                max_width,
                max_height,
            })
        }
        client_msg::MINIMIZE_WINDOW => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::MinimizeWindow { window_id })
        }
        client_msg::MAXIMIZE_WINDOW => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::MaximizeWindow { window_id })
        }
        client_msg::RESTORE_WINDOW => {
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::RestoreWindow { window_id })
        }
        client_msg::SET_WINDOW_TYPE => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let window_type = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Ok(ClientMessageRef::SetWindowType {
                window_id,
                window_type,
            })
        }
        client_msg::SET_WINDOW_OPACITY => {
            if payload.len() != 5 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let opacity = payload[4];
            Ok(ClientMessageRef::SetWindowOpacity { window_id, opacity })
        }
        client_msg::SET_WORKAREA => {
            if payload.len() != 16 {
                return Err(ProtocolError::MalformedPayload);
            }
            let x = i32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let y = i32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let width = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            let height = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
            Ok(ClientMessageRef::SetWorkarea {
                x,
                y,
                width,
                height,
            })
        }
        client_msg::SET_WINDOW_RESIZABLE => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let resizable = payload[4] != 0;
            Ok(ClientMessageRef::SetWindowResizable {
                window_id,
                resizable,
            })
        }
        client_msg::GET_SCREEN_SIZE => {
            if !payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::GetScreenSize {})
        }
        client_msg::GET_WINDOW_LIST => {
            if !payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::GetWindowList {})
        }
        client_msg::LAUNCH_OR_FOCUS => {
            // Payload: app_id_len (u32) + app_id_bytes + exec_path_len (u32) + exec_path_bytes
            if payload.len() < 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let app_id_len =
                u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
            let exec_path_len =
                u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;

            if payload.len() != 8 + app_id_len + exec_path_len {
                return Err(ProtocolError::MalformedPayload);
            }

            let app_id = &payload[8..8 + app_id_len];
            let exec_path = &payload[8 + app_id_len..8 + app_id_len + exec_path_len];

            Ok(ClientMessageRef::LaunchOrFocus { app_id, exec_path })
        }
        client_msg::FOCUS_WINDOW => {
            // Payload: window_id (u32)
            if payload.len() != 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ClientMessageRef::FocusWindow { window_id })
        }
        client_msg::GET_ACTIVE_APP => {
            // No payload
            if !payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ClientMessageRef::GetActiveApp {})
        }
        client_msg::SET_WINDOW_HAS_ALPHA_CONTENT => {
            // Payload: window_id (u32) + has_alpha (u8, 0 = false, 1 = true)
            if payload.len() != 5 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let has_alpha = payload[4] != 0;
            Ok(ClientMessageRef::SetWindowHasAlphaContent {
                window_id,
                has_alpha,
            })
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
        server_msg::WINDOW_RESIZED => {
            if payload.len() != 20 {
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
            let width = u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]);
            let height = u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
            Ok(ServerMessage::WindowResized {
                window_id,
                shm_size,
                width,
                height,
            })
        }
        server_msg::WINDOW_CONFIGURE => {
            if payload.len() != 12 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let width = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            let height = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
            Ok(ServerMessage::WindowConfigure {
                window_id,
                width,
                height,
            })
        }
        server_msg::INPUT_EVENT => {
            if payload.len() != 20 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let time = u64::from_le_bytes([
                payload[4],
                payload[5],
                payload[6],
                payload[7],
                payload[8],
                payload[9],
                payload[10],
                payload[11],
            ]);
            let type_ = u16::from_le_bytes([payload[12], payload[13]]);
            let code = u16::from_le_bytes([payload[14], payload[15]]);
            let value = i32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]);
            Ok(ServerMessage::InputEvent {
                window_id,
                time,
                type_,
                code,
                value,
            })
        }
        server_msg::SCREEN_SIZE => {
            if payload.len() != 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let width = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let height = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
            Ok(ServerMessage::ScreenSize { width, height })
        }
        server_msg::WINDOW_LIST => {
            // Window list payload is variable length, just validate it's not empty
            if payload.is_empty() {
                return Err(ProtocolError::MalformedPayload);
            }
            Ok(ServerMessage::WindowList)
        }
        server_msg::FOCUS_CHANGED => {
            // Payload: window_id (u32) + app_id_len (u32) + app_id (variable, max 128)
            //          + app_name_len (u32) + app_name (variable, max 128)
            //          + title_len (u32) + title (variable, max 256)
            //          + menu_titles_len (u32) + menu_titles (variable, max 512)
            if payload.len() < 8 {
                return Err(ProtocolError::MalformedPayload);
            }
            let window_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            let app_id_len =
                u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;

            let mut offset = 8 + app_id_len;
            if payload.len() < offset + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut app_id = [0u8; 128];
            if app_id_len > 0 {
                app_id[..app_id_len].copy_from_slice(&payload[8..8 + app_id_len]);
            }

            let app_name_len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;
            offset += 4 + app_name_len;

            if payload.len() < offset + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut app_name = [0u8; 128];
            if app_name_len > 0 {
                app_name[..app_name_len].copy_from_slice(&payload[offset - app_name_len..offset]);
            }

            let title_len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;
            offset += 4 + title_len;

            if payload.len() < offset + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut title = [0u8; 256];
            if title_len > 0 {
                title[..title_len].copy_from_slice(&payload[offset - title_len..offset]);
            }

            let menu_titles_len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;
            offset += 4 + menu_titles_len;

            if payload.len() < offset {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut menu_titles = [0u8; 512];
            if menu_titles_len > 0 {
                menu_titles[..menu_titles_len]
                    .copy_from_slice(&payload[offset - menu_titles_len..offset]);
            }

            Ok(ServerMessage::FocusChanged {
                window_id,
                app_id,
                app_id_len: app_id_len as u32,
                app_name,
                app_name_len: app_name_len as u32,
                title,
                title_len: title_len as u32,
                menu_titles,
                menu_titles_len: menu_titles_len as u32,
            })
        }
        server_msg::ERROR => {
            if payload.len() < 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let code = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ServerMessage::Error { code })
        }
        server_msg::ACTIVE_APP => {
            // Payload: app_id_len (u32) + app_id (variable, max 128)
            //          + app_name_len (u32) + app_name (variable, max 128)
            //          + menu_titles_len (u32) + menu_titles (variable, max 512)
            if payload.len() < 4 {
                return Err(ProtocolError::MalformedPayload);
            }
            let app_id_len =
                u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;

            if payload.len() < 4 + app_id_len + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut app_id = [0u8; 128];
            if app_id_len > 0 {
                app_id[..app_id_len].copy_from_slice(&payload[4..4 + app_id_len]);
            }

            let offset = 4 + app_id_len;
            let app_name_len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;

            if payload.len() < offset + 4 + app_name_len + 4 {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut app_name = [0u8; 128];
            if app_name_len > 0 {
                app_name[..app_name_len]
                    .copy_from_slice(&payload[offset + 4..offset + 4 + app_name_len]);
            }

            let offset2 = offset + 4 + app_name_len;
            let menu_titles_len = u32::from_le_bytes([
                payload[offset2],
                payload[offset2 + 1],
                payload[offset2 + 2],
                payload[offset2 + 3],
            ]) as usize;

            if payload.len() < offset2 + 4 + menu_titles_len {
                return Err(ProtocolError::MalformedPayload);
            }

            let mut menu_titles = [0u8; 512];
            if menu_titles_len > 0 {
                menu_titles[..menu_titles_len]
                    .copy_from_slice(&payload[offset2 + 4..offset2 + 4 + menu_titles_len]);
            }

            Ok(ServerMessage::ActiveApp {
                app_id,
                app_id_len: app_id_len as u32,
                app_name,
                app_name_len: app_name_len as u32,
                menu_titles,
                menu_titles_len: menu_titles_len as u32,
            })
        }
        _ => Err(ProtocolError::UnknownMessageType),
    }
}

/// Build payload for client->server `CREATE_WINDOW`.
///
/// Payload format:
/// - app_id_len (u32)
/// - app_id_bytes (variable)
/// - app_name_len (u32)
/// - app_name_bytes (variable)
/// - menu_titles_len (u32)
/// - menu_titles_bytes (variable, format: "menu1|menu2|menu3")
/// - width (u32)
/// - height (u32)
pub fn payload_create_window(
    app_id: &[u8],
    app_name: &[u8],
    menu_titles: &[u8],
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(app_id.len() as u32).to_le_bytes());
    payload.extend_from_slice(app_id);
    payload.extend_from_slice(&(app_name.len() as u32).to_le_bytes());
    payload.extend_from_slice(app_name);
    payload.extend_from_slice(&(menu_titles.len() as u32).to_le_bytes());
    payload.extend_from_slice(menu_titles);
    payload.extend_from_slice(&width.to_le_bytes());
    payload.extend_from_slice(&height.to_le_bytes());
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

/// Build payload for client->server `SET_WINDOW_PARENT`.
///
/// `parent_id == 0` means "no parent".
pub fn payload_set_window_parent(window_id: u32, parent_id: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&parent_id.to_le_bytes());
    payload
}

/// Build payload for client->server `SET_WINDOW_TRANSIENT_FLAGS`.
pub fn payload_set_window_transient_flags(window_id: u32, flags: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&flags.to_le_bytes());
    payload
}

/// Build payload for client->server `RESIZE_WINDOW`.
pub fn payload_resize_window(window_id: u32, width: u32, height: u32) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&width.to_le_bytes());
    payload[8..12].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for client->server `SET_WINDOW_SIZE_LIMITS`.
pub fn payload_set_window_size_limits(
    window_id: u32,
    min_width: u32,
    min_height: u32,
    max_width: u32,
    max_height: u32,
) -> [u8; 20] {
    let mut payload = [0u8; 20];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&min_width.to_le_bytes());
    payload[8..12].copy_from_slice(&min_height.to_le_bytes());
    payload[12..16].copy_from_slice(&max_width.to_le_bytes());
    payload[16..20].copy_from_slice(&max_height.to_le_bytes());
    payload
}

/// Build payload for server->client `WINDOW_CREATED`.
pub fn payload_window_created(window_id: u32, shm_size: u64) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..12].copy_from_slice(&shm_size.to_le_bytes());
    payload
}

/// Build payload for server->client `WINDOW_RESIZED`.
pub fn payload_window_resized(window_id: u32, shm_size: u64, width: u32, height: u32) -> [u8; 20] {
    let mut payload = [0u8; 20];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..12].copy_from_slice(&shm_size.to_le_bytes());
    payload[12..16].copy_from_slice(&width.to_le_bytes());
    payload[16..20].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for server->client `WINDOW_CONFIGURE`.
pub fn payload_window_configure(window_id: u32, width: u32, height: u32) -> [u8; 12] {
    let mut payload = [0u8; 12];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&width.to_le_bytes());
    payload[8..12].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for server->client `WINDOW_DESTROYED`.
pub fn payload_window_destroyed(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for server->client `INPUT_EVENT`.
pub fn payload_input_event(
    window_id: u32,
    time: u64,
    type_: u16,
    code: u16,
    value: i32,
) -> [u8; 20] {
    let mut payload = [0u8; 20];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..12].copy_from_slice(&time.to_le_bytes());
    payload[12..14].copy_from_slice(&type_.to_le_bytes());
    payload[14..16].copy_from_slice(&code.to_le_bytes());
    payload[16..20].copy_from_slice(&value.to_le_bytes());
    payload
}

/// Build payload for server->client `ERROR`.
pub fn payload_error(code: u32) -> [u8; 4] {
    code.to_le_bytes()
}

/// Build payload for client->server `MINIMIZE_WINDOW`.
pub fn payload_minimize_window(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for client->server `MAXIMIZE_WINDOW`.
pub fn payload_maximize_window(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for client->server `RESTORE_WINDOW`.
pub fn payload_restore_window(window_id: u32) -> [u8; 4] {
    window_id.to_le_bytes()
}

/// Build payload for client->server `SET_WINDOW_TYPE`.
pub fn payload_set_window_type(window_id: u32, window_type: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4..8].copy_from_slice(&window_type.to_le_bytes());
    payload
}

/// Build payload for client->server `SET_WINDOW_OPACITY`.
pub fn payload_set_window_opacity(window_id: u32, opacity: u8) -> [u8; 5] {
    let mut payload = [0u8; 5];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4] = opacity;
    payload
}

/// Build payload for client->server `SET_WORKAREA`.
pub fn payload_set_workarea(x: i32, y: i32, width: u32, height: u32) -> [u8; 16] {
    let mut payload = [0u8; 16];
    payload[0..4].copy_from_slice(&x.to_le_bytes());
    payload[4..8].copy_from_slice(&y.to_le_bytes());
    payload[8..12].copy_from_slice(&width.to_le_bytes());
    payload[12..16].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Build payload for client->server `SET_WINDOW_RESIZABLE`.
pub fn payload_set_window_resizable(window_id: u32, resizable: bool) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4] = if resizable { 1 } else { 0 };
    payload
}

/// Build payload for server->client `SCREEN_SIZE`.
pub fn payload_screen_size(width: u32, height: u32) -> [u8; 8] {
    let mut payload = [0u8; 8];
    payload[0..4].copy_from_slice(&width.to_le_bytes());
    payload[4..8].copy_from_slice(&height.to_le_bytes());
    payload
}

/// Window list entry for WINDOW_LIST message
#[derive(Debug, Clone)]
pub struct WindowListEntry {
    pub window_id: u32,
    pub app_id: std::string::String,
    pub title: std::string::String,
    pub window_type: u32,
    pub visible: bool,
    pub focused: bool,
    pub minimized: bool,
}

/// Build payload for server->client `WINDOW_LIST`.
///
/// Serializes a list of windows into the wire format:
/// - count (u32)
/// - For each window:
///   - window_id (u32)
///   - app_id_length (u32)
///   - app_id_bytes (variable)
///   - title_length (u32)
///   - title_bytes (variable)
///   - window_type (u32)
///   - flags (3 bytes: visible, focused, minimized) + 1 byte padding
pub fn payload_window_list(windows: &[WindowListEntry]) -> Vec<u8> {
    let mut payload = Vec::new();

    // Window count
    payload.extend_from_slice(&(windows.len() as u32).to_le_bytes());

    for entry in windows {
        // Window ID
        payload.extend_from_slice(&entry.window_id.to_le_bytes());

        // App ID length and app_id
        let app_id_bytes = entry.app_id.as_bytes();
        payload.extend_from_slice(&(app_id_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(app_id_bytes);

        // Title length and title
        let title_bytes = entry.title.as_bytes();
        payload.extend_from_slice(&(title_bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(title_bytes);

        // Window type
        payload.extend_from_slice(&entry.window_type.to_le_bytes());

        // Flags
        payload.push(if entry.visible { 1 } else { 0 });
        payload.push(if entry.focused { 1 } else { 0 });
        payload.push(if entry.minimized { 1 } else { 0 });
        payload.push(0); // padding
    }

    payload
}

/// Parse WINDOW_LIST payload into a list of window entries.
///
/// See `payload_window_list` for the wire format.
pub fn parse_window_list_payload(payload: &[u8]) -> Result<Vec<WindowListEntry>, ProtocolError> {
    if payload.len() < 4 {
        return Err(ProtocolError::MalformedPayload);
    }

    let count = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    let mut windows = Vec::new();
    let mut offset = 4;

    for _ in 0..count {
        // Window ID (4 bytes)
        if offset + 4 > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let window_id = u32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]);
        offset += 4;

        // App ID length (4 bytes)
        if offset + 4 > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let app_id_len = u32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]) as usize;
        offset += 4;

        // App ID bytes
        if offset + app_id_len > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let app_id = std::string::String::from_utf8_lossy(&payload[offset..offset + app_id_len])
            .into_owned();
        offset += app_id_len;

        // Title length (4 bytes)
        if offset + 4 > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let title_len = u32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]) as usize;
        offset += 4;

        // Title bytes
        if offset + title_len > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let title =
            std::string::String::from_utf8_lossy(&payload[offset..offset + title_len]).into_owned();
        offset += title_len;

        // Window type (4 bytes)
        if offset + 4 > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let window_type = u32::from_le_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]);
        offset += 4;

        // Flags (4 bytes: visible, focused, minimized, padding)
        if offset + 4 > payload.len() {
            return Err(ProtocolError::MalformedPayload);
        }
        let visible = payload[offset] != 0;
        let focused = payload[offset + 1] != 0;
        let minimized = payload[offset + 2] != 0;
        offset += 4;

        windows.push(WindowListEntry {
            window_id,
            app_id,
            title,
            window_type,
            visible,
            focused,
            minimized,
        });
    }

    Ok(windows)
}

/// Build payload for client->server `LAUNCH_OR_FOCUS`.
///
/// Payload format:
/// - app_id_len (u32)
/// - app_id_bytes (variable)
/// - exec_path_len (u32)
/// - exec_path_bytes (variable)
pub fn payload_launch_or_focus(app_id: &[u8], exec_path: &[u8]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(app_id.len() as u32).to_le_bytes());
    payload.extend_from_slice(app_id);
    payload.extend_from_slice(&(exec_path.len() as u32).to_le_bytes());
    payload.extend_from_slice(exec_path);
    payload
}

/// Build payload for client->server `FOCUS_WINDOW`.
pub fn payload_focus_window(window_id: u32) -> Vec<u8> {
    window_id.to_le_bytes().to_vec()
}

/// Build payload for server->client `FOCUS_CHANGED`.
///
/// Payload format:
/// - window_id (u32)
/// - app_id_len (u32)
/// - app_id_bytes (variable, max 128)
/// - app_name_len (u32)
/// - app_name_bytes (variable, max 128)
/// - title_len (u32)
/// - title_bytes (variable, max 256)
/// - menu_titles_len (u32)
/// - menu_titles_bytes (variable, max 512, format: "menu1|menu2|menu3")
pub fn payload_focus_changed(
    window_id: u32,
    app_id: &[u8],
    app_name: &[u8],
    title: &[u8],
    menu_titles: &[u8],
) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&window_id.to_le_bytes());

    let app_id_len = app_id.len().min(128);
    payload.extend_from_slice(&(app_id_len as u32).to_le_bytes());
    payload.extend_from_slice(&app_id[..app_id_len]);

    let app_name_len = app_name.len().min(128);
    payload.extend_from_slice(&(app_name_len as u32).to_le_bytes());
    payload.extend_from_slice(&app_name[..app_name_len]);

    let title_len = title.len().min(256);
    payload.extend_from_slice(&(title_len as u32).to_le_bytes());
    payload.extend_from_slice(&title[..title_len]);

    let menu_titles_len = menu_titles.len().min(512);
    payload.extend_from_slice(&(menu_titles_len as u32).to_le_bytes());
    payload.extend_from_slice(&menu_titles[..menu_titles_len]);

    payload
}

/// Build payload for client->server `SET_WINDOW_HAS_ALPHA_CONTENT`.
pub fn payload_set_window_has_alpha_content(window_id: u32, has_alpha: bool) -> [u8; 5] {
    let mut payload = [0u8; 5];
    payload[0..4].copy_from_slice(&window_id.to_le_bytes());
    payload[4] = if has_alpha { 1 } else { 0 };
    payload
}
