//! Scarlet Audio Server socket protocol.

extern crate alloc;

use alloc::vec::Vec;

pub const SOCKET_PATH: &str = "/tmp/sas.sock";
pub const SERVICE_NAME: &str = "org.scarlet-os.sas";

pub const MSG_CONFIGURE: u32 = 0x0001;
pub const MSG_DRAIN: u32 = 0x0003;
pub const MSG_CLOSE: u32 = 0x0004;
pub const MSG_OK: u32 = 0x1000;
pub const MSG_ERROR: u32 = 0x1001;

pub const HEADER_SIZE: usize = 8;
pub const CONFIG_SIZE: usize = 20;
pub const MAX_PAYLOAD_SIZE: usize = 64 * 1024;
pub const RING_MAGIC: u32 = 0x5341_5352;
pub const RING_VERSION: u32 = 1;
pub const RING_FLAG_DRAINING: u32 = 1 << 0;
pub const RING_FLAG_CLOSED: u32 = 1 << 1;

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

pub const RING_HEADER_SIZE: usize = core::mem::size_of::<RingHeader>();

#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub msg_type: u32,
    pub payload_size: u32,
}

impl Header {
    pub fn to_le_bytes(self) -> [u8; HEADER_SIZE] {
        let mut out = [0u8; HEADER_SIZE];
        out[0..4].copy_from_slice(&self.msg_type.to_le_bytes());
        out[4..8].copy_from_slice(&self.payload_size.to_le_bytes());
        out
    }

    pub fn from_le_bytes(bytes: [u8; HEADER_SIZE]) -> Self {
        Self {
            msg_type: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            payload_size: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub format: u32,
    pub rate: u32,
    pub channels: u16,
    pub reserved: u16,
    pub period_frames: u32,
    pub buffer_frames: u32,
}

impl Config {
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
