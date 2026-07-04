//! Shared Scarlet video decode device ABI definitions.
//!
//! The video decode character device API is currently experimental, but both
//! VirtIO video and Apple AVD backends use the same user-visible control
//! contract. Keep the command values, mapped-buffer structures, and frame
//! constants here so backend implementations do not drift apart.

use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::sync::Mutex;

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

/// Capabilities advertised by a video decode backend.
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoBackendCapabilities {
    /// Maximum number of simultaneously owned decode sessions.
    pub max_sessions: u32,
    /// Maximum byte length accepted in the mapped input area.
    pub mapped_input_len: u32,
    /// Maximum byte length produced in the mapped output area.
    pub mapped_output_len: u32,
    /// Pixel format produced by decoded frames.
    pub output_pixel_format: u32,
    /// Whether H.264 access units are accepted.
    pub supports_h264: bool,
    /// Whether AV1 access units are accepted.
    pub supports_av1: bool,
}

impl VideoBackendCapabilities {
    /// Return whether this backend supports a Scarlet coded format.
    ///
    /// # Arguments
    ///
    /// * `coded_format` - Scarlet coded format value.
    ///
    /// # Returns
    ///
    /// `true` when the backend accepts the format.
    pub fn supports_format(&self, coded_format: u32) -> bool {
        match coded_format {
            SCARLET_VIDEO_FORMAT_H264 => self.supports_h264,
            SCARLET_VIDEO_FORMAT_AV1 => self.supports_av1,
            _ => false,
        }
    }
}

/// Backend decode request for a mapped access unit.
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoBackendDecodeRequest {
    /// Backend stream/session identifier.
    pub stream_id: u32,
    /// Scarlet coded stream format.
    pub coded_format: u32,
    /// Device-visible address of the coded input bytes.
    pub input_dma_addr: u64,
    /// Byte length of the coded input.
    pub input_len: u32,
    /// Device-visible address of the output frame buffer.
    pub output_dma_addr: u64,
    /// Byte capacity of the output frame buffer.
    pub output_len: u32,
    /// Presentation timestamp carried through dequeue.
    pub timestamp: u64,
}

/// Decoded frame returned by a backend.
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoBackendDecodedFrame {
    /// Backend stream/session identifier.
    pub stream_id: u32,
    /// User-visible decoded frame metadata.
    pub frame: ScarletVideoDequeuedFrame,
}

/// Common interface for Scarlet video decode backends.
pub trait VideoDecodeBackend: Send + Sync {
    /// Return a short backend name for diagnostics.
    ///
    /// # Returns
    ///
    /// Static backend name.
    fn name(&self) -> &'static str;

    /// Return backend capabilities.
    ///
    /// # Returns
    ///
    /// Capabilities used by `/dev/vvideo*` frontends.
    fn capabilities(&self) -> VideoBackendCapabilities;

    /// Create or acquire a decode session.
    ///
    /// # Arguments
    ///
    /// * `coded_format` - Scarlet coded format requested by userspace.
    ///
    /// # Returns
    ///
    /// Backend stream/session identifier.
    fn create_session(&self, coded_format: u32) -> Result<u32, &'static str>;

    /// Destroy a decode session.
    ///
    /// # Arguments
    ///
    /// * `stream_id` - Backend stream/session identifier.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the session is released.
    fn destroy_session(&self, stream_id: u32) -> Result<(), &'static str>;

    /// Submit one coded access unit.
    ///
    /// # Arguments
    ///
    /// * `request` - Mapped decode request with device-visible buffers.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the backend accepted the request.
    fn submit_decode(&self, request: &VideoBackendDecodeRequest) -> Result<(), &'static str>;

    /// Dequeue one decoded frame if available.
    ///
    /// # Arguments
    ///
    /// * `stream_id` - Backend stream/session identifier.
    ///
    /// # Returns
    ///
    /// `Some(frame)` when a decoded frame is ready, or `None` when still pending.
    fn dequeue_frame(
        &self,
        stream_id: u32,
    ) -> Result<Option<VideoBackendDecodedFrame>, &'static str>;
}

static VIDEO_BACKENDS: Mutex<Vec<Arc<dyn VideoDecodeBackend>>> = Mutex::new(Vec::new());

/// Register a video decode backend.
///
/// # Arguments
///
/// * `backend` - Backend implementation to register.
///
/// # Returns
///
/// Zero-based backend registry identifier.
pub fn register_video_backend(backend: Arc<dyn VideoDecodeBackend>) -> usize {
    let mut backends = VIDEO_BACKENDS.lock();
    let id = backends.len();
    backends.push(backend);
    id
}

/// Return the first registered video decode backend.
///
/// # Returns
///
/// First backend when any backend has registered.
pub fn first_video_backend() -> Option<Arc<dyn VideoDecodeBackend>> {
    VIDEO_BACKENDS.lock().first().cloned()
}

/// Return a video decode backend by registry identifier.
///
/// # Arguments
///
/// * `id` - Backend registry identifier.
///
/// # Returns
///
/// Backend implementation when present.
pub fn get_video_backend(id: usize) -> Option<Arc<dyn VideoDecodeBackend>> {
    VIDEO_BACKENDS.lock().get(id).cloned()
}

/// Return the number of registered video decode backends.
///
/// # Returns
///
/// Number of currently registered backends.
pub fn video_backend_count() -> usize {
    VIDEO_BACKENDS.lock().len()
}

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
