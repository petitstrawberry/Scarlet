//! Wayland Wire Protocol Implementation
//!
//! This module implements the Wayland wire protocol for parsing and
//! encoding Wayland messages. The wire protocol is documented at:
//! https://wayland.freedesktop.org/docs/html/ch04.html
//!
//! Wire Format:
//! - All integers are in host byte order (native endianness)
//! - Message header: [object_id: u32, size_and_opcode: u32]
//!   - size_and_opcode = (size << 16) | opcode
//!   - size includes the header (8 bytes minimum)
//! - Arguments follow the header based on message signature

use std::vec::Vec;

/// Wayland message header (8 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MessageHeader {
    /// Object ID this message is for
    pub object_id: u32,
    /// Combined size (upper 16 bits) and opcode (lower 16 bits)
    pub size_and_opcode: u32,
}

impl MessageHeader {
    pub const SIZE: usize = 8;

    /// Get the message size in bytes (includes header)
    pub fn size(&self) -> u32 {
        self.size_and_opcode >> 16
    }

    /// Get the message opcode
    pub fn opcode(&self) -> u16 {
        (self.size_and_opcode & 0xFFFF) as u16
    }

    /// Create a new message header
    pub fn new(object_id: u32, size: u32, opcode: u16) -> Self {
        Self {
            object_id,
            size_and_opcode: (size << 16) | (opcode as u32),
        }
    }

    /// Parse header from bytes
    pub fn from_bytes(bytes: &[u8; 8]) -> Self {
        let object_id = u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let size_and_opcode = u32::from_ne_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        Self {
            object_id,
            size_and_opcode,
        }
    }

    /// Convert header to bytes
    pub fn to_bytes(&self) -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes[0..4].copy_from_slice(&self.object_id.to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.size_and_opcode.to_ne_bytes());
        bytes
    }
}

/// Wayland argument types
#[derive(Debug, Clone)]
pub enum WaylandArg {
    /// Integer (i32)
    Int(i32),
    /// Unsigned integer (u32)
    Uint(u32),
    /// Fixed point number (24.8 format)
    Fixed(i32),
    /// String (null-terminated, padded to 4-byte boundary)
    String(Vec<u8>),
    /// Object ID
    Object(u32),
    /// New object ID (with interface name and version for bind operations)
    NewId(u32),
    /// Array of bytes
    Array(Vec<u8>),
    /// File descriptor (index in ancillary data)
    Fd(i32),
}

/// Wayland message
#[derive(Debug, Clone)]
pub struct WaylandMessage {
    pub header: MessageHeader,
    pub args: Vec<WaylandArg>,
}

impl WaylandMessage {
    /// Create a new Wayland message
    pub fn new(object_id: u32, opcode: u16) -> Self {
        Self {
            header: MessageHeader::new(object_id, 8, opcode),
            args: Vec::new(),
        }
    }

    /// Add an argument to the message
    pub fn add_arg(&mut self, arg: WaylandArg) {
        self.args.push(arg);
    }

    /// Encode message to bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        
        // Calculate total size
        let mut size = 8u32; // Header size
        for arg in &self.args {
            size += arg.encoded_size();
        }

        // Write header
        let header = MessageHeader::new(self.header.object_id, size, self.header.opcode());
        bytes.extend_from_slice(&header.to_bytes());

        // Write arguments
        for arg in &self.args {
            arg.encode_into(&mut bytes);
        }

        bytes
    }
}

impl WaylandArg {
    /// Get the encoded size of this argument (with padding)
    fn encoded_size(&self) -> u32 {
        match self {
            WaylandArg::Int(_) | WaylandArg::Uint(_) | WaylandArg::Fixed(_) 
            | WaylandArg::Object(_) | WaylandArg::NewId(_) => 4,
            WaylandArg::String(s) => {
                // size (4 bytes) + string (including null) + padding
                let len = s.len() + 1; // +1 for null terminator
                4 + ((len + 3) & !3) as u32 // Round up to 4-byte boundary
            }
            WaylandArg::Array(a) => {
                // size (4 bytes) + data + padding
                4 + ((a.len() + 3) & !3) as u32
            }
            WaylandArg::Fd(_) => 0, // FDs are passed via handle transfer (Socket::recv_handle/send_handle)
        }
    }

    /// Encode this argument into a byte vector
    fn encode_into(&self, bytes: &mut Vec<u8>) {
        match self {
            WaylandArg::Int(v) => bytes.extend_from_slice(&v.to_ne_bytes()),
            WaylandArg::Uint(v) => bytes.extend_from_slice(&v.to_ne_bytes()),
            WaylandArg::Fixed(v) => bytes.extend_from_slice(&v.to_ne_bytes()),
            WaylandArg::Object(v) => bytes.extend_from_slice(&v.to_ne_bytes()),
            WaylandArg::NewId(v) => bytes.extend_from_slice(&v.to_ne_bytes()),
            WaylandArg::String(s) => {
                let len = (s.len() + 1) as u32; // +1 for null terminator
                bytes.extend_from_slice(&len.to_ne_bytes());
                bytes.extend_from_slice(s);
                bytes.push(0); // Null terminator
                // Pad to 4-byte boundary
                while bytes.len() % 4 != 0 {
                    bytes.push(0);
                }
            }
            WaylandArg::Array(a) => {
                let len = a.len() as u32;
                bytes.extend_from_slice(&len.to_ne_bytes());
                bytes.extend_from_slice(a);
                // Pad to 4-byte boundary
                while bytes.len() % 4 != 0 {
                    bytes.push(0);
                }
            }
            WaylandArg::Fd(_) => {
                // FDs are passed via handle transfer (Socket::recv_handle/send_handle)
                // The Linux compatibility layer converts SCM_RIGHTS to handle transfer
                // Nothing to encode in the message body
            }
        }
    }
}

/// Parse a Wayland message from bytes
pub fn parse_message(data: &[u8]) -> Option<WaylandMessage> {
    if data.len() < MessageHeader::SIZE {
        return None;
    }

    let mut header_bytes = [0u8; 8];
    header_bytes.copy_from_slice(&data[0..8]);
    let header = MessageHeader::from_bytes(&header_bytes);

    let msg_size = header.size() as usize;
    if data.len() < msg_size {
        return None;
    }

    // For now, we don't parse arguments - that requires knowing the message signature
    // which depends on the interface and opcode. We'll add that incrementally.
    
    Some(WaylandMessage {
        header,
        args: Vec::new(),
    })
}

/// Wayland global interface IDs
pub mod interfaces {
    /// wl_display interface ID (always 1)
    pub const WL_DISPLAY: u32 = 1;
}

/// wl_display opcodes (requests from client)
pub mod display_request {
    pub const SYNC: u16 = 0;
    pub const GET_REGISTRY: u16 = 1;
}

/// wl_display opcodes (events from server)
pub mod display_event {
    pub const ERROR: u16 = 0;
    pub const DELETE_ID: u16 = 1;
}

/// wl_registry opcodes (requests from client)
pub mod registry_request {
    pub const BIND: u16 = 0;
}

/// wl_registry opcodes (events from server)
pub mod registry_event {
    pub const GLOBAL: u16 = 0;
    pub const GLOBAL_REMOVE: u16 = 1;
}

/// wl_compositor opcodes (requests from client)
pub mod compositor_request {
    pub const CREATE_SURFACE: u16 = 0;
    pub const CREATE_REGION: u16 = 1;
}

/// wl_surface opcodes (requests from client)
pub mod surface_request {
    pub const DESTROY: u16 = 0;
    pub const ATTACH: u16 = 1;
    pub const DAMAGE: u16 = 2;
    pub const FRAME: u16 = 3;
    pub const SET_OPAQUE_REGION: u16 = 4;
    pub const SET_INPUT_REGION: u16 = 5;
    pub const COMMIT: u16 = 6;
}

/// wl_surface opcodes (events from server)
pub mod surface_event {
    pub const ENTER: u16 = 0;
    pub const LEAVE: u16 = 1;
}
