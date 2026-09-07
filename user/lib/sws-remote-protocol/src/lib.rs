//! Protocol shared by SWS and privileged remote-display services.
//!
//! The protocol deliberately exposes only completed-output capture and virtual
//! input. Remote-display transports, key-symbol conversion, authentication,
//! networking, and client management remain outside SWS.
//!
//! Every stream frame starts with an eight-byte little-endian header:
//! `message_type: u16`, `flags: u16`, and `payload_size: u32`. A
//! [`ClientMessage::RegisterBuffer`] frame is sent atomically with the shared
//! memory handle described by that frame.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate scarlet_std as std;

use std::vec::Vec;

/// Current version of the SWS remote protocol.
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum payload accepted from one protocol frame.
pub const MAX_PAYLOAD_SIZE: usize = 64 * 1024;

/// Maximum damage rectangles returned for one captured frame.
pub const MAX_DAMAGE_RECTS: usize = 256;

/// Header flag indicating that the frame carries one transferred handle.
pub const FLAG_HAS_HANDLE: u16 = 1 << 0;

/// Capture pixel formats with stable wire values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CaptureFormat {
    /// Eight-bit blue, green, red, and alpha channels in memory order.
    Bgra8888 = 1,
}

impl CaptureFormat {
    /// Decode a stable wire value.
    ///
    /// # Arguments
    ///
    /// * `value` - Raw format value from a protocol payload.
    ///
    /// # Returns
    ///
    /// The corresponding capture format, or `None` for an unknown value.
    pub const fn from_raw(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Bgra8888),
            _ => None,
        }
    }

    /// Return the stable wire value.
    ///
    /// # Returns
    ///
    /// The `u32` value encoded in a buffer-registration payload.
    pub const fn as_raw(self) -> u32 {
        self as u32
    }

    /// Return the bytes occupied by one pixel.
    ///
    /// # Returns
    ///
    /// The tightly packed byte size of one pixel.
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Bgra8888 => 4,
        }
    }
}

/// Output-space damage rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge in physical output pixels.
    pub x: u32,
    /// Top edge in physical output pixels.
    pub y: u32,
    /// Rectangle width in physical output pixels.
    pub width: u32,
    /// Rectangle height in physical output pixels.
    pub height: u32,
}

impl Rect {
    /// Construct an output-space rectangle.
    ///
    /// # Arguments
    ///
    /// * `x` - Left edge in pixels.
    /// * `y` - Top edge in pixels.
    /// * `width` - Width in pixels.
    /// * `height` - Height in pixels.
    ///
    /// # Returns
    ///
    /// A rectangle carrying the supplied coordinates.
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Fixed stream-frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageHeader {
    /// Stable message type.
    pub message_type: u16,
    /// Per-frame flags such as [`FLAG_HAS_HANDLE`].
    pub flags: u16,
    /// Number of payload bytes following the header.
    pub payload_size: u32,
}

impl MessageHeader {
    /// Encoded header size in bytes.
    pub const SIZE: usize = 8;

    /// Construct a frame header.
    ///
    /// # Arguments
    ///
    /// * `message_type` - Stable message type.
    /// * `flags` - Frame flags.
    /// * `payload_size` - Encoded payload size.
    ///
    /// # Returns
    ///
    /// A header containing the supplied fields.
    pub const fn new(message_type: u16, flags: u16, payload_size: u32) -> Self {
        Self {
            message_type,
            flags,
            payload_size,
        }
    }

    /// Encode the header in little-endian wire order.
    ///
    /// # Returns
    ///
    /// The fixed-size wire representation.
    pub fn to_le_bytes(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        bytes[0..2].copy_from_slice(&self.message_type.to_le_bytes());
        bytes[2..4].copy_from_slice(&self.flags.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.payload_size.to_le_bytes());
        bytes
    }

    /// Decode a little-endian wire header.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Complete eight-byte header.
    ///
    /// # Returns
    ///
    /// The decoded header.
    pub fn from_le_bytes(bytes: [u8; Self::SIZE]) -> Self {
        Self {
            message_type: u16::from_le_bytes([bytes[0], bytes[1]]),
            flags: u16::from_le_bytes([bytes[2], bytes[3]]),
            payload_size: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        }
    }
}

/// Stable client-to-SWS message type identifiers.
pub mod client_message_type {
    /// Create the single capture session for an output.
    pub const CREATE_CAPTURE: u16 = 1;
    /// Register one client-owned shared-memory capture buffer.
    pub const REGISTER_BUFFER: u16 = 2;
    /// Request copying the current completed output into a registered buffer.
    pub const REQUEST_FRAME: u16 = 3;
    /// Inject one Scarlet keycode transition.
    pub const KEY: u16 = 4;
    /// Inject an absolute pointer position.
    pub const POINTER_ABSOLUTE: u16 = 5;
    /// Inject one pointer-button transition.
    pub const POINTER_BUTTON: u16 = 6;
    /// Inject discrete horizontal and vertical scroll deltas.
    pub const POINTER_SCROLL: u16 = 7;
}

/// Stable SWS-to-client message type identifiers.
pub mod server_message_type {
    /// Report that a newer completed output frame exists.
    pub const FRAME_AVAILABLE: u16 = 0x8001;
    /// Report completion of one requested capture copy.
    pub const FRAME_READY: u16 = 0x8002;
    /// Report the current physical output dimensions.
    pub const OUTPUT_CHANGED: u16 = 0x8003;
}

/// Messages sent by a remote-display service to SWS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMessage {
    /// Select an output and create the connection's capture session.
    CreateCapture {
        /// SWS output identifier. The initial implementation exposes output zero.
        output_id: u32,
    },
    /// Register a client-owned shared-memory buffer.
    RegisterBuffer {
        /// Connection-local buffer identifier.
        buffer_id: u32,
        /// Buffer width in physical pixels.
        width: u32,
        /// Buffer height in physical pixels.
        height: u32,
        /// Bytes between adjacent rows.
        stride: u32,
        /// Buffer pixel format.
        format: CaptureFormat,
    },
    /// Copy the latest completed frame into a registered buffer.
    RequestFrame {
        /// Destination buffer identifier.
        buffer_id: u32,
    },
    /// Inject one Scarlet keycode transition.
    Key {
        /// Scarlet/Linux input keycode.
        code: u16,
        /// `true` for a press and `false` for a release.
        pressed: bool,
    },
    /// Inject an absolute pointer position.
    PointerAbsolute {
        /// Horizontal physical-output coordinate.
        x: i32,
        /// Vertical physical-output coordinate.
        y: i32,
    },
    /// Inject one pointer-button transition.
    PointerButton {
        /// Scarlet/Linux input button code.
        button: u16,
        /// `true` for a press and `false` for a release.
        pressed: bool,
    },
    /// Inject discrete pointer-wheel movement.
    PointerScroll {
        /// Horizontal wheel notches; positive values scroll right.
        dx: i32,
        /// Vertical wheel notches; positive values scroll up.
        dy: i32,
    },
}

impl ClientMessage {
    /// Return whether this message must carry one transferred handle.
    ///
    /// # Returns
    ///
    /// `true` only for [`Self::RegisterBuffer`].
    pub const fn requires_handle(&self) -> bool {
        matches!(self, Self::RegisterBuffer { .. })
    }
}

/// Messages sent by SWS to a remote-display service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMessage {
    /// A newer completed output frame is available on demand.
    FrameAvailable {
        /// Monotonically increasing presentation sequence.
        sequence: u64,
    },
    /// A requested capture buffer now contains the reported sequence.
    FrameReady {
        /// Destination buffer identifier.
        buffer_id: u32,
        /// Captured presentation sequence.
        sequence: u64,
        /// Output-space regions copied since the preceding capture.
        damage: Vec<Rect>,
    },
    /// Physical output dimensions changed or were initially announced.
    OutputChanged {
        /// New width in physical pixels.
        width: u32,
        /// New height in physical pixels.
        height: u32,
    },
}

/// Protocol decoding failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    /// The message type is unknown in this direction.
    UnknownMessage,
    /// The frame flags do not match the message contract.
    InvalidFlags,
    /// The payload length or field encoding is invalid.
    InvalidPayload,
    /// The advertised payload exceeds [`MAX_PAYLOAD_SIZE`].
    PayloadTooLarge,
}

/// Encode a complete client-to-SWS stream frame.
///
/// The caller must send a [`ClientMessage::RegisterBuffer`] result atomically
/// with its shared-memory handle.
///
/// # Arguments
///
/// * `message` - Client message to serialize.
///
/// # Returns
///
/// Header and payload bytes in wire order.
pub fn encode_client_message(message: &ClientMessage) -> Vec<u8> {
    let mut payload = Vec::new();
    let (message_type, flags) = match message {
        ClientMessage::CreateCapture { output_id } => {
            push_u32(&mut payload, *output_id);
            (client_message_type::CREATE_CAPTURE, 0)
        }
        ClientMessage::RegisterBuffer {
            buffer_id,
            width,
            height,
            stride,
            format,
        } => {
            push_u32(&mut payload, *buffer_id);
            push_u32(&mut payload, *width);
            push_u32(&mut payload, *height);
            push_u32(&mut payload, *stride);
            push_u32(&mut payload, format.as_raw());
            (client_message_type::REGISTER_BUFFER, FLAG_HAS_HANDLE)
        }
        ClientMessage::RequestFrame { buffer_id } => {
            push_u32(&mut payload, *buffer_id);
            (client_message_type::REQUEST_FRAME, 0)
        }
        ClientMessage::Key { code, pressed } => {
            push_u16(&mut payload, *code);
            payload.push(u8::from(*pressed));
            payload.push(0);
            (client_message_type::KEY, 0)
        }
        ClientMessage::PointerAbsolute { x, y } => {
            push_i32(&mut payload, *x);
            push_i32(&mut payload, *y);
            (client_message_type::POINTER_ABSOLUTE, 0)
        }
        ClientMessage::PointerButton { button, pressed } => {
            push_u16(&mut payload, *button);
            payload.push(u8::from(*pressed));
            payload.push(0);
            (client_message_type::POINTER_BUTTON, 0)
        }
        ClientMessage::PointerScroll { dx, dy } => {
            push_i32(&mut payload, *dx);
            push_i32(&mut payload, *dy);
            (client_message_type::POINTER_SCROLL, 0)
        }
    };
    encode_frame(message_type, flags, &payload)
}

/// Decode one client-to-SWS payload.
///
/// # Arguments
///
/// * `header` - Previously decoded frame header.
/// * `payload` - Complete payload named by `header`.
///
/// # Returns
///
/// The decoded client message or a protocol error.
pub fn decode_client_message(
    header: MessageHeader,
    payload: &[u8],
) -> Result<ClientMessage, ProtocolError> {
    validate_payload_size(header, payload)?;
    let (expected_flags, message) = match header.message_type {
        client_message_type::CREATE_CAPTURE => {
            require_len(payload, 4)?;
            (
                0,
                ClientMessage::CreateCapture {
                    output_id: read_u32(payload, 0),
                },
            )
        }
        client_message_type::REGISTER_BUFFER => {
            require_len(payload, 20)?;
            let format = CaptureFormat::from_raw(read_u32(payload, 16))
                .ok_or(ProtocolError::InvalidPayload)?;
            (
                FLAG_HAS_HANDLE,
                ClientMessage::RegisterBuffer {
                    buffer_id: read_u32(payload, 0),
                    width: read_u32(payload, 4),
                    height: read_u32(payload, 8),
                    stride: read_u32(payload, 12),
                    format,
                },
            )
        }
        client_message_type::REQUEST_FRAME => {
            require_len(payload, 4)?;
            (
                0,
                ClientMessage::RequestFrame {
                    buffer_id: read_u32(payload, 0),
                },
            )
        }
        client_message_type::KEY => {
            require_len(payload, 4)?;
            (
                0,
                ClientMessage::Key {
                    code: read_u16(payload, 0),
                    pressed: read_bool(payload[2])?,
                },
            )
        }
        client_message_type::POINTER_ABSOLUTE => {
            require_len(payload, 8)?;
            (
                0,
                ClientMessage::PointerAbsolute {
                    x: read_i32(payload, 0),
                    y: read_i32(payload, 4),
                },
            )
        }
        client_message_type::POINTER_BUTTON => {
            require_len(payload, 4)?;
            (
                0,
                ClientMessage::PointerButton {
                    button: read_u16(payload, 0),
                    pressed: read_bool(payload[2])?,
                },
            )
        }
        client_message_type::POINTER_SCROLL => {
            require_len(payload, 8)?;
            (
                0,
                ClientMessage::PointerScroll {
                    dx: read_i32(payload, 0),
                    dy: read_i32(payload, 4),
                },
            )
        }
        _ => return Err(ProtocolError::UnknownMessage),
    };
    if header.flags != expected_flags {
        return Err(ProtocolError::InvalidFlags);
    }
    Ok(message)
}

/// Encode a complete SWS-to-client stream frame.
///
/// # Arguments
///
/// * `message` - Server message to serialize.
///
/// # Returns
///
/// Header and payload bytes in wire order.
pub fn encode_server_message(message: &ServerMessage) -> Vec<u8> {
    let mut payload = Vec::new();
    let message_type = match message {
        ServerMessage::FrameAvailable { sequence } => {
            push_u64(&mut payload, *sequence);
            server_message_type::FRAME_AVAILABLE
        }
        ServerMessage::FrameReady {
            buffer_id,
            sequence,
            damage,
        } => {
            let damage_count = damage.len().min(MAX_DAMAGE_RECTS);
            push_u32(&mut payload, *buffer_id);
            push_u32(&mut payload, damage_count as u32);
            push_u64(&mut payload, *sequence);
            for rect in damage.iter().take(damage_count) {
                push_u32(&mut payload, rect.x);
                push_u32(&mut payload, rect.y);
                push_u32(&mut payload, rect.width);
                push_u32(&mut payload, rect.height);
            }
            server_message_type::FRAME_READY
        }
        ServerMessage::OutputChanged { width, height } => {
            push_u32(&mut payload, *width);
            push_u32(&mut payload, *height);
            server_message_type::OUTPUT_CHANGED
        }
    };
    encode_frame(message_type, 0, &payload)
}

/// Decode one SWS-to-client payload.
///
/// # Arguments
///
/// * `header` - Previously decoded frame header.
/// * `payload` - Complete payload named by `header`.
///
/// # Returns
///
/// The decoded server message or a protocol error.
pub fn decode_server_message(
    header: MessageHeader,
    payload: &[u8],
) -> Result<ServerMessage, ProtocolError> {
    validate_payload_size(header, payload)?;
    if header.flags != 0 {
        return Err(ProtocolError::InvalidFlags);
    }
    match header.message_type {
        server_message_type::FRAME_AVAILABLE => {
            require_len(payload, 8)?;
            Ok(ServerMessage::FrameAvailable {
                sequence: read_u64(payload, 0),
            })
        }
        server_message_type::FRAME_READY => {
            if payload.len() < 16 {
                return Err(ProtocolError::InvalidPayload);
            }
            let count = read_u32(payload, 4) as usize;
            if count > MAX_DAMAGE_RECTS
                || 16usize.checked_add(count.saturating_mul(16)) != Some(payload.len())
            {
                return Err(ProtocolError::InvalidPayload);
            }
            let mut damage = Vec::new();
            damage
                .try_reserve_exact(count)
                .map_err(|_| ProtocolError::InvalidPayload)?;
            for index in 0..count {
                let offset = 16 + index * 16;
                damage.push(Rect::new(
                    read_u32(payload, offset),
                    read_u32(payload, offset + 4),
                    read_u32(payload, offset + 8),
                    read_u32(payload, offset + 12),
                ));
            }
            Ok(ServerMessage::FrameReady {
                buffer_id: read_u32(payload, 0),
                sequence: read_u64(payload, 8),
                damage,
            })
        }
        server_message_type::OUTPUT_CHANGED => {
            require_len(payload, 8)?;
            Ok(ServerMessage::OutputChanged {
                width: read_u32(payload, 0),
                height: read_u32(payload, 4),
            })
        }
        _ => Err(ProtocolError::UnknownMessage),
    }
}

/// Decode the header of a complete or partial frame.
///
/// # Arguments
///
/// * `bytes` - Buffer beginning with a complete frame header.
///
/// # Returns
///
/// The decoded header, or an error when the buffer is short or the payload is
/// larger than the protocol limit.
pub fn decode_header(bytes: &[u8]) -> Result<MessageHeader, ProtocolError> {
    if bytes.len() < MessageHeader::SIZE {
        return Err(ProtocolError::InvalidPayload);
    }
    let mut encoded = [0; MessageHeader::SIZE];
    encoded.copy_from_slice(&bytes[..MessageHeader::SIZE]);
    let header = MessageHeader::from_le_bytes(encoded);
    if header.payload_size as usize > MAX_PAYLOAD_SIZE {
        return Err(ProtocolError::PayloadTooLarge);
    }
    Ok(header)
}

fn encode_frame(message_type: u16, flags: u16, payload: &[u8]) -> Vec<u8> {
    let payload_size = u32::try_from(payload.len()).expect("remote protocol payload exceeds u32");
    let header = MessageHeader::new(message_type, flags, payload_size);
    let mut frame = Vec::with_capacity(MessageHeader::SIZE + payload.len());
    frame.extend_from_slice(&header.to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn validate_payload_size(header: MessageHeader, payload: &[u8]) -> Result<(), ProtocolError> {
    if header.payload_size as usize > MAX_PAYLOAD_SIZE {
        return Err(ProtocolError::PayloadTooLarge);
    }
    if header.payload_size as usize != payload.len() {
        return Err(ProtocolError::InvalidPayload);
    }
    Ok(())
}

fn require_len(payload: &[u8], expected: usize) -> Result<(), ProtocolError> {
    if payload.len() == expected {
        Ok(())
    } else {
        Err(ProtocolError::InvalidPayload)
    }
}

fn read_bool(value: u8) -> Result<bool, ProtocolError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ProtocolError::InvalidPayload),
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
        bytes[offset + 4],
        bytes[offset + 5],
        bytes[offset + 6],
        bytes[offset + 7],
    ])
}

fn push_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_i32(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::{
        CaptureFormat, ClientMessage, FLAG_HAS_HANDLE, MAX_DAMAGE_RECTS, MessageHeader,
        ProtocolError, Rect, ServerMessage, decode_client_message, decode_header,
        decode_server_message, encode_client_message, encode_server_message,
    };

    fn split_frame(frame: &[u8]) -> (MessageHeader, &[u8]) {
        let header = decode_header(frame).unwrap();
        (header, &frame[MessageHeader::SIZE..])
    }

    #[test]
    fn client_messages_round_trip() {
        let messages = [
            ClientMessage::CreateCapture { output_id: 0 },
            ClientMessage::RegisterBuffer {
                buffer_id: 9,
                width: 1920,
                height: 1080,
                stride: 7680,
                format: CaptureFormat::Bgra8888,
            },
            ClientMessage::RequestFrame { buffer_id: 9 },
            ClientMessage::Key {
                code: 30,
                pressed: true,
            },
            ClientMessage::PointerAbsolute { x: 100, y: 200 },
            ClientMessage::PointerButton {
                button: 0x110,
                pressed: false,
            },
            ClientMessage::PointerScroll { dx: -1, dy: 2 },
        ];

        for message in messages {
            let frame = encode_client_message(&message);
            let (header, payload) = split_frame(&frame);
            assert_eq!(decode_client_message(header, payload), Ok(message));
        }
    }

    #[test]
    fn register_buffer_is_the_only_handle_frame() {
        let frame = encode_client_message(&ClientMessage::RegisterBuffer {
            buffer_id: 1,
            width: 1,
            height: 1,
            stride: 4,
            format: CaptureFormat::Bgra8888,
        });
        let (header, _) = split_frame(&frame);
        assert_eq!(header.flags, FLAG_HAS_HANDLE);

        let frame = encode_client_message(&ClientMessage::RequestFrame { buffer_id: 1 });
        let (header, _) = split_frame(&frame);
        assert_eq!(header.flags, 0);
    }

    #[test]
    fn server_messages_round_trip() {
        let messages = [
            ServerMessage::FrameAvailable { sequence: 44 },
            ServerMessage::FrameReady {
                buffer_id: 7,
                sequence: 44,
                damage: vec![Rect::new(1, 2, 3, 4), Rect::new(20, 30, 40, 50)],
            },
            ServerMessage::OutputChanged {
                width: 1280,
                height: 720,
            },
        ];

        for message in messages {
            let frame = encode_server_message(&message);
            let (header, payload) = split_frame(&frame);
            assert_eq!(decode_server_message(header, payload), Ok(message));
        }
    }

    #[test]
    fn frame_ready_encoding_bounds_excessive_damage() {
        let message = ServerMessage::FrameReady {
            buffer_id: 7,
            sequence: 44,
            damage: vec![Rect::new(0, 0, 1, 1); MAX_DAMAGE_RECTS + 1],
        };
        let frame = encode_server_message(&message);
        let (header, payload) = split_frame(&frame);
        let decoded = decode_server_message(header, payload).unwrap();
        let ServerMessage::FrameReady { damage, .. } = decoded else {
            panic!("expected FrameReady");
        };
        assert_eq!(damage.len(), MAX_DAMAGE_RECTS);
    }

    #[test]
    fn malformed_boolean_is_rejected() {
        let frame = encode_client_message(&ClientMessage::Key {
            code: 30,
            pressed: true,
        });
        let (header, payload) = split_frame(&frame);
        let mut malformed = payload.to_vec();
        malformed[2] = 2;
        assert_eq!(
            decode_client_message(header, &malformed),
            Err(ProtocolError::InvalidPayload)
        );
    }
}
