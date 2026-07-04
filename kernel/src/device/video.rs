//! Shared Scarlet video decode device ABI definitions.
//!
//! The video decode character device API is currently experimental, but both
//! VirtIO video and Apple AVD backends use the same user-visible control
//! contract. Keep the command values, mapped-buffer structures, and frame
//! constants here so backend implementations do not drift apart.

/// FourCC-like Scarlet frame stream magic.
pub const SCARLET_VIDEO_FRAME_MAGIC: &[u8; 4] = b"SVF1";
/// Length of the `SVF1` frame header in bytes.
pub const SCARLET_VIDEO_FRAME_HEADER_LEN: usize = 20;
/// NV12 video-range pixel format value expected by `video_player`.
pub const SCARLET_VIDEO_PIXEL_FORMAT_NV12: u32 = 0x3432_3076;

/// Query the mapped buffer layout for the legacy single-session path.
pub const VVIDEO_GET_BUFFER: u32 = 0x5600;
/// Submit a coded access unit for the legacy single-session path.
pub const VVIDEO_SUBMIT: u32 = 0x5601;
/// Dequeue a decoded frame for the legacy single-session path.
pub const VVIDEO_DEQUEUE: u32 = 0x5602;
/// Create or query a mapped video session.
pub const VVIDEO_CREATE_SESSION: u32 = 0x5603;
/// Submit a coded access unit for a mapped video session.
pub const VVIDEO_SUBMIT_SESSION: u32 = 0x5604;
/// Dequeue a decoded frame for a mapped video session.
pub const VVIDEO_DEQUEUE_SESSION: u32 = 0x5605;
/// Destroy a mapped video session.
pub const VVIDEO_DESTROY_SESSION: u32 = 0x5606;

/// Scarlet coded format value for H.264.
pub const SCARLET_VIDEO_FORMAT_H264: u32 = 4098;
/// Scarlet coded format value for AV1.
pub const SCARLET_VIDEO_FORMAT_AV1: u32 = 4103;

/// Information returned to userspace for a mapped video buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoBufferInfo {
    /// Offset passed to `mmap`.
    pub mmap_offset: u64,
    /// Total byte length to map.
    pub mmap_len: u64,
    /// Offset of the coded input area inside the mapping.
    pub input_offset: u64,
    /// Byte length of the coded input area.
    pub input_len: u32,
    /// Offset of the decoded output area inside the mapping.
    pub output_offset: u64,
    /// Byte length of the decoded output area.
    pub output_len: u32,
}

/// Legacy single-session submit request.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoSubmit {
    /// Number of bytes written to the mapped input area.
    pub input_len: u32,
    /// Coded stream format.
    pub coded_format: u32,
    /// Presentation timestamp carried through dequeue.
    pub timestamp: u64,
}

/// Decoded frame metadata returned to userspace.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoDequeuedFrame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Decoded pixel format.
    pub pixel_format: u32,
    /// Offset of frame payload inside the mapped output area.
    pub payload_offset: u64,
    /// Byte length of the decoded payload.
    pub payload_len: u32,
    /// Backend-specific frame flags.
    pub flags: u32,
    /// Presentation timestamp from submit.
    pub timestamp: u64,
}

/// Mapped video session creation/query result.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoSessionInfo {
    /// Backend stream/session identifier.
    pub stream_id: u32,
    /// Reserved padding for ABI stability.
    pub padding: u32,
    /// Mapped buffer layout for this stream.
    pub buffer: ScarletVideoBufferInfo,
}

/// Mapped video session submit request.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoSessionSubmit {
    /// Backend stream/session identifier.
    pub stream_id: u32,
    /// Number of bytes written to the mapped input area.
    pub input_len: u32,
    /// Coded stream format.
    pub coded_format: u32,
    /// Reserved padding for ABI stability.
    pub padding: u32,
    /// Presentation timestamp carried through dequeue.
    pub timestamp: u64,
}

/// Mapped video session dequeue result.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoSessionDequeuedFrame {
    /// Backend stream/session identifier.
    pub stream_id: u32,
    /// Reserved padding for ABI stability.
    pub padding: u32,
    /// Decoded frame metadata.
    pub frame: ScarletVideoDequeuedFrame,
}

/// Write an `SVF1` header into `bytes`.
///
/// # Arguments
///
/// * `bytes` - Destination byte vector.
/// * `width` - Frame width in pixels.
/// * `height` - Frame height in pixels.
/// * `pixel_format` - Decoded pixel format.
/// * `payload_len` - Frame payload length in bytes.
pub fn push_svf1_header(
    bytes: &mut alloc::vec::Vec<u8>,
    width: u32,
    height: u32,
    pixel_format: u32,
    payload_len: u32,
) {
    bytes.extend_from_slice(SCARLET_VIDEO_FRAME_MAGIC);
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes.extend_from_slice(&pixel_format.to_le_bytes());
    bytes.extend_from_slice(&payload_len.to_le_bytes());
}

/// Apple AVD firmware-to-kernel mailbox ABI.
pub mod avd_fw {
    /// Firmware initialized and waiting for work.
    pub const MSG_READY: u32 = 0x0000_0001;
    /// Firmware panic or hardfault.
    pub const MSG_PANIC: u32 = 0x0000_0002;
    /// Video pipe decode completed.
    pub const MSG_VP_DONE: u32 = 0x0000_0100;
    /// Video pipe decode error.
    pub const MSG_VP_ERROR: u32 = 0x0000_0200;
    /// Post-process pipe completed.
    pub const MSG_PP_DONE: u32 = 0x0000_1000;
    /// Unexpected IRQ vector.
    pub const MSG_UNKNOWN_IRQ: u32 = 0x0001_0000;
}
