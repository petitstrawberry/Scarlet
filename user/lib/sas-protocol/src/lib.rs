//! Scarlet Audio Server (SAS) socket protocol.
//!
//! This crate is the single source of truth for both the SAS server (`sas`)
//! and clients (`sas_client`) for message IDs, framing, and ring buffer layout.
//!
//! # Wire format
//!
//! Each message is framed as:
//! - Header (8 bytes, little-endian)
//!   - `msg_type: u32`
//!   - `payload_size: u32`
//! - Payload (`payload_size` bytes)

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate scarlet_std as std;

extern crate alloc;

use alloc::vec::Vec;

/// Default SAS Unix domain socket path.
pub const SOCKET_PATH: &str = "/tmp/sas.sock";

/// D-Bus-style service name for sbus registration.
pub const SERVICE_NAME: &str = "org.scarlet-os.sas";

// Client -> server message types.
pub const MSG_CONFIGURE: u32 = 0x0001;
pub const MSG_DRAIN: u32 = 0x0003;
pub const MSG_CLOSE: u32 = 0x0004;
pub const MSG_GET_CONTROL_STATE: u32 = 0x0005;
pub const MSG_SET_MASTER_VOLUME: u32 = 0x0006;
pub const MSG_SET_MASTER_MUTE: u32 = 0x0007;
pub const MSG_SET_OUTPUT: u32 = 0x0008;
pub const MSG_LIST_OUTPUTS: u32 = 0x0009;

// Server -> client message types.
pub const MSG_OK: u32 = 0x1000;
pub const MSG_ERROR: u32 = 0x1001;
pub const MSG_CONTROL_STATE: u32 = 0x1002;
pub const MSG_OUTPUT_LIST: u32 = 0x1003;

/// Framed message header size in bytes.
pub const HEADER_SIZE: usize = 8;

/// `Config` payload size in bytes.
pub const CONFIG_SIZE: usize = 20;

/// Fixed bytes in an output device path.
pub const OUTPUT_PATH_LEN: usize = 32;

/// Fixed bytes in an output device stable name.
pub const OUTPUT_NAME_LEN: usize = 32;

/// Fixed bytes in an output device description.
pub const OUTPUT_DESCRIPTION_LEN: usize = 64;

/// Fixed bytes in an output preference value.
pub const OUTPUT_VALUE_LEN: usize = 64;

/// `ControlState` payload size in bytes.
pub const CONTROL_STATE_SIZE: usize =
    12 + OUTPUT_PATH_LEN + OUTPUT_NAME_LEN + OUTPUT_DESCRIPTION_LEN;

/// `MasterVolume` payload size in bytes.
pub const MASTER_VOLUME_SIZE: usize = 4;

/// `MasterMute` payload size in bytes.
pub const MASTER_MUTE_SIZE: usize = 4;

/// `OutputRequest` payload size in bytes.
pub const OUTPUT_REQUEST_SIZE: usize = 4 + OUTPUT_VALUE_LEN;

/// One output entry payload size in bytes.
pub const OUTPUT_ENTRY_SIZE: usize = 8 + OUTPUT_PATH_LEN + OUTPUT_NAME_LEN + OUTPUT_DESCRIPTION_LEN;

/// Maximum payload size accepted from the socket.
pub const MAX_PAYLOAD_SIZE: usize = 64 * 1024;

/// Magic number stored in `RingHeader::magic`.
pub const RING_MAGIC: u32 = 0x5341_5352;

/// Ring buffer layout version stored in `RingHeader::version`.
pub const RING_VERSION: u32 = 1;

/// Ring flag: client requested drain (play remaining data then stop).
pub const RING_FLAG_DRAINING: u32 = 1 << 0;

/// Ring flag: stream has been closed.
pub const RING_FLAG_CLOSED: u32 = 1 << 1;

/// Master volume unity gain in unsigned Q16.16 fixed point.
pub const MASTER_VOLUME_UNITY_Q16: u32 = 1 << 16;

/// Master output is muted.
pub const CONTROL_FLAG_MUTED: u32 = 1 << 0;

/// Select built-in speakers.
pub const OUTPUT_PREFERENCE_SPEAKERS: u32 = 1;
/// Select headphone or headset output.
pub const OUTPUT_PREFERENCE_HEADPHONES: u32 = 2;
/// Select output by `/dev/audioN` path.
pub const OUTPUT_PREFERENCE_PATH: u32 = 3;
/// Select output by stable audio device name.
pub const OUTPUT_PREFERENCE_NAME: u32 = 4;

/// Output entry is the active SAS output.
pub const OUTPUT_ENTRY_FLAG_CURRENT: u32 = 1 << 0;
/// Output entry supports SAS' fixed S16LE 48 kHz stereo stream format.
pub const OUTPUT_ENTRY_FLAG_COMPATIBLE: u32 = 1 << 1;

/// Shared-memory ring buffer header.
///
/// The server creates the ring and the client writes PCM frames into the data
/// area that follows this header.  All multi-byte fields use native endian
/// (Scarlet targets are little-endian).
///
/// # Memory layout
///
/// ```text
/// [RingHeader] [PCM sample data …]
/// ```
///
/// The data area size is `buffer_frames * frame_bytes`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RingHeader {
    pub magic: u32,
    pub version: u32,
    pub format: u32,
    pub rate: u32,
    pub channels: u32,
    pub frame_bytes: u32,
    pub period_frames: u32,
    pub buffer_frames: u32,
    pub write_frames: u64,
    pub read_frames: u64,
    pub flags: u32,
    pub xrun_count: u32,
    pub reserved: [u8; 8],
}

/// Size of `RingHeader` in bytes.
pub const RING_HEADER_SIZE: usize = core::mem::size_of::<RingHeader>();

/// Framed message header.
#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub msg_type: u32,
    pub payload_size: u32,
}

impl Header {
    /// Serialize the header to little-endian bytes.
    pub fn to_le_bytes(self) -> [u8; HEADER_SIZE] {
        let mut out = [0u8; HEADER_SIZE];
        out[0..4].copy_from_slice(&self.msg_type.to_le_bytes());
        out[4..8].copy_from_slice(&self.payload_size.to_le_bytes());
        out
    }

    /// Deserialize the header from little-endian bytes.
    pub fn from_le_bytes(bytes: [u8; HEADER_SIZE]) -> Self {
        Self {
            msg_type: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            payload_size: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        }
    }
}

/// Stream configuration sent in `MSG_CONFIGURE`.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub format: u32,
    pub rate: u32,
    pub channels: u16,
    pub reserved: u16,
    pub period_frames: u32,
    pub buffer_frames: u32,
}

/// Current SAS output control state.
#[derive(Clone, Copy, Debug)]
pub struct ControlState {
    pub master_volume_q16: u32,
    pub flags: u32,
    pub output_kind: u32,
    pub output_path: [u8; OUTPUT_PATH_LEN],
    pub output_name: [u8; OUTPUT_NAME_LEN],
    pub output_description: [u8; OUTPUT_DESCRIPTION_LEN],
}

impl ControlState {
    /// Create a control state with fixed-size output identity fields.
    pub fn new(
        master_volume_q16: u32,
        flags: u32,
        output_kind: u32,
        output_path: &str,
        output_name: &str,
        output_description: &str,
    ) -> Self {
        let mut state = Self {
            master_volume_q16,
            flags,
            output_kind,
            output_path: [0; OUTPUT_PATH_LEN],
            output_name: [0; OUTPUT_NAME_LEN],
            output_description: [0; OUTPUT_DESCRIPTION_LEN],
        };
        copy_fixed(&mut state.output_path, output_path.as_bytes());
        copy_fixed(&mut state.output_name, output_name.as_bytes());
        copy_fixed(&mut state.output_description, output_description.as_bytes());
        state
    }

    /// Serialize the control state to little-endian bytes.
    pub fn to_le_bytes(self) -> [u8; CONTROL_STATE_SIZE] {
        let mut out = [0u8; CONTROL_STATE_SIZE];
        out[0..4].copy_from_slice(&self.master_volume_q16.to_le_bytes());
        out[4..8].copy_from_slice(&self.flags.to_le_bytes());
        out[8..12].copy_from_slice(&self.output_kind.to_le_bytes());
        out[12..12 + OUTPUT_PATH_LEN].copy_from_slice(&self.output_path);
        out[12 + OUTPUT_PATH_LEN..12 + OUTPUT_PATH_LEN + OUTPUT_NAME_LEN]
            .copy_from_slice(&self.output_name);
        out[12 + OUTPUT_PATH_LEN + OUTPUT_NAME_LEN..CONTROL_STATE_SIZE]
            .copy_from_slice(&self.output_description);
        out
    }

    /// Deserialize the control state from a payload slice.
    pub fn from_payload(payload: &[u8]) -> Option<Self> {
        if payload.len() != CONTROL_STATE_SIZE {
            return None;
        }
        Some(Self {
            master_volume_q16: u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]),
            flags: u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]),
            output_kind: u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]),
            output_path: copy_array::<OUTPUT_PATH_LEN>(&payload[12..12 + OUTPUT_PATH_LEN]),
            output_name: copy_array::<OUTPUT_NAME_LEN>(
                &payload[12 + OUTPUT_PATH_LEN..12 + OUTPUT_PATH_LEN + OUTPUT_NAME_LEN],
            ),
            output_description: copy_array::<OUTPUT_DESCRIPTION_LEN>(
                &payload[12 + OUTPUT_PATH_LEN + OUTPUT_NAME_LEN..CONTROL_STATE_SIZE],
            ),
        })
    }
}

/// Master volume request.
#[derive(Clone, Copy, Debug)]
pub struct MasterVolume {
    pub master_volume_q16: u32,
}

impl MasterVolume {
    /// Serialize the master volume request to little-endian bytes.
    pub fn to_le_bytes(self) -> [u8; MASTER_VOLUME_SIZE] {
        self.master_volume_q16.to_le_bytes()
    }

    /// Deserialize the master volume request from a payload slice.
    pub fn from_payload(payload: &[u8]) -> Option<Self> {
        if payload.len() != MASTER_VOLUME_SIZE {
            return None;
        }
        Some(Self {
            master_volume_q16: u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]),
        })
    }
}

/// Master mute request.
#[derive(Clone, Copy, Debug)]
pub struct MasterMute {
    pub muted: bool,
}

impl MasterMute {
    /// Serialize the mute request to little-endian bytes.
    pub fn to_le_bytes(self) -> [u8; MASTER_MUTE_SIZE] {
        (self.muted as u32).to_le_bytes()
    }

    /// Deserialize the mute request from a payload slice.
    pub fn from_payload(payload: &[u8]) -> Option<Self> {
        if payload.len() != MASTER_MUTE_SIZE {
            return None;
        }
        Some(Self {
            muted: u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) != 0,
        })
    }
}

/// Output device selection request.
#[derive(Clone, Copy, Debug)]
pub struct OutputRequest {
    pub preference: u32,
    pub value: [u8; OUTPUT_VALUE_LEN],
}

impl OutputRequest {
    /// Create an output request.
    pub fn new(preference: u32, value: &str) -> Option<Self> {
        if value.as_bytes().len() >= OUTPUT_VALUE_LEN {
            return None;
        }
        let mut request = Self {
            preference,
            value: [0; OUTPUT_VALUE_LEN],
        };
        copy_fixed(&mut request.value, value.as_bytes());
        Some(request)
    }

    /// Serialize the output request to little-endian bytes.
    pub fn to_le_bytes(self) -> [u8; OUTPUT_REQUEST_SIZE] {
        let mut out = [0u8; OUTPUT_REQUEST_SIZE];
        out[0..4].copy_from_slice(&self.preference.to_le_bytes());
        out[4..OUTPUT_REQUEST_SIZE].copy_from_slice(&self.value);
        out
    }

    /// Deserialize the output request from a payload slice.
    pub fn from_payload(payload: &[u8]) -> Option<Self> {
        if payload.len() != OUTPUT_REQUEST_SIZE {
            return None;
        }
        Some(Self {
            preference: u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]),
            value: copy_array::<OUTPUT_VALUE_LEN>(&payload[4..OUTPUT_REQUEST_SIZE]),
        })
    }

    /// Interpret the fixed-size value as UTF-8.
    pub fn value_str(&self) -> Option<&str> {
        fixed_str(&self.value)
    }
}

/// Output device entry returned by `MSG_LIST_OUTPUTS`.
#[derive(Clone, Copy, Debug)]
pub struct OutputInfo {
    pub kind: u32,
    pub flags: u32,
    pub path: [u8; OUTPUT_PATH_LEN],
    pub name: [u8; OUTPUT_NAME_LEN],
    pub description: [u8; OUTPUT_DESCRIPTION_LEN],
}

impl OutputInfo {
    /// Create an output info entry.
    pub fn new(kind: u32, flags: u32, path: &str, name: &str, description: &str) -> Self {
        let mut info = Self {
            kind,
            flags,
            path: [0; OUTPUT_PATH_LEN],
            name: [0; OUTPUT_NAME_LEN],
            description: [0; OUTPUT_DESCRIPTION_LEN],
        };
        copy_fixed(&mut info.path, path.as_bytes());
        copy_fixed(&mut info.name, name.as_bytes());
        copy_fixed(&mut info.description, description.as_bytes());
        info
    }

    /// Serialize the output entry to little-endian bytes.
    pub fn to_le_bytes(self) -> [u8; OUTPUT_ENTRY_SIZE] {
        let mut out = [0u8; OUTPUT_ENTRY_SIZE];
        out[0..4].copy_from_slice(&self.kind.to_le_bytes());
        out[4..8].copy_from_slice(&self.flags.to_le_bytes());
        out[8..8 + OUTPUT_PATH_LEN].copy_from_slice(&self.path);
        out[8 + OUTPUT_PATH_LEN..8 + OUTPUT_PATH_LEN + OUTPUT_NAME_LEN].copy_from_slice(&self.name);
        out[8 + OUTPUT_PATH_LEN + OUTPUT_NAME_LEN..OUTPUT_ENTRY_SIZE]
            .copy_from_slice(&self.description);
        out
    }

    /// Deserialize one output entry from a payload slice.
    pub fn from_payload(payload: &[u8]) -> Option<Self> {
        if payload.len() != OUTPUT_ENTRY_SIZE {
            return None;
        }
        Some(Self {
            kind: u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]),
            flags: u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]),
            path: copy_array::<OUTPUT_PATH_LEN>(&payload[8..8 + OUTPUT_PATH_LEN]),
            name: copy_array::<OUTPUT_NAME_LEN>(
                &payload[8 + OUTPUT_PATH_LEN..8 + OUTPUT_PATH_LEN + OUTPUT_NAME_LEN],
            ),
            description: copy_array::<OUTPUT_DESCRIPTION_LEN>(
                &payload[8 + OUTPUT_PATH_LEN + OUTPUT_NAME_LEN..OUTPUT_ENTRY_SIZE],
            ),
        })
    }
}

/// Serialize output entries to a list payload.
pub fn output_list_payload(entries: &[OutputInfo]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + entries.len() * OUTPUT_ENTRY_SIZE);
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for entry in entries {
        out.extend_from_slice(&entry.to_le_bytes());
    }
    out
}

/// Deserialize output entries from a list payload.
pub fn output_list_from_payload(payload: &[u8]) -> Option<Vec<OutputInfo>> {
    if payload.len() < 4 {
        return None;
    }
    let count = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    let expected = 4usize.checked_add(count.checked_mul(OUTPUT_ENTRY_SIZE)?)?;
    if payload.len() != expected {
        return None;
    }

    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let start = 4 + index * OUTPUT_ENTRY_SIZE;
        let end = start + OUTPUT_ENTRY_SIZE;
        entries.push(OutputInfo::from_payload(&payload[start..end])?);
    }
    Some(entries)
}

impl Config {
    /// Serialize the configuration to little-endian bytes.
    pub fn to_le_bytes(self) -> [u8; CONFIG_SIZE] {
        let mut out = [0u8; CONFIG_SIZE];
        out[0..4].copy_from_slice(&self.format.to_le_bytes());
        out[4..8].copy_from_slice(&self.rate.to_le_bytes());
        out[8..10].copy_from_slice(&self.channels.to_le_bytes());
        out[10..12].copy_from_slice(&self.reserved.to_le_bytes());
        out[12..16].copy_from_slice(&self.period_frames.to_le_bytes());
        out[16..20].copy_from_slice(&self.buffer_frames.to_le_bytes());
        out
    }

    /// Deserialize the configuration from a payload slice.
    pub fn from_payload(payload: &[u8]) -> Option<Self> {
        if payload.len() != CONFIG_SIZE {
            return None;
        }
        Some(Self {
            format: u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]),
            rate: u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]),
            channels: u16::from_le_bytes([payload[8], payload[9]]),
            reserved: u16::from_le_bytes([payload[10], payload[11]]),
            period_frames: u32::from_le_bytes([payload[12], payload[13], payload[14], payload[15]]),
            buffer_frames: u32::from_le_bytes([payload[16], payload[17], payload[18], payload[19]]),
        })
    }
}

/// Encode a full framed message (header + payload) into a single buffer.
pub fn frame(msg_type: u32, payload: &[u8]) -> Vec<u8> {
    let header = Header {
        msg_type,
        payload_size: payload.len() as u32,
    };
    let mut out = Vec::with_capacity(HEADER_SIZE + payload.len());
    out.extend_from_slice(&header.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn copy_fixed(dst: &mut [u8], src: &[u8]) {
    let count = src.len().min(dst.len().saturating_sub(1));
    dst[..count].copy_from_slice(&src[..count]);
}

fn copy_array<const N: usize>(src: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    out.copy_from_slice(src);
    out
}

fn fixed_str(bytes: &[u8]) -> Option<&str> {
    let len = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..len]).ok()
}
