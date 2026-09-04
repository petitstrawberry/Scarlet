//! Scarlet hardware video decoder client.
//!
//! This crate owns the `/dev/video0` transport and presents one decoder API to
//! stateful backends such as VirtIO Video and stateless backends such as Apple
//! AVD. Callers submit complete coded access units and receive validated NV12
//! frames without issuing raw Scarlet control calls.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(all(feature = "std", feature = "legacy-scarlet-std"))]
compile_error!("features `std` and `legacy-scarlet-std` are mutually exclusive");
#[cfg(not(any(feature = "std", feature = "legacy-scarlet-std")))]
compile_error!("enable either the default `std` feature or `legacy-scarlet-std`");

extern crate alloc;
#[cfg(feature = "legacy-scarlet-std")]
extern crate scarlet_std as std;

mod abi;
mod h264_stateless;
mod vp9_stateless;

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;

use scarlet_os::handle::Handle;
use scarlet_os::handle::capability::memory_mapping::{flags as mmap_flags, munmap, prot};
use std::sync::Mutex;
use std::thread;

use crate::abi::{
    SCARLET_VIDEO_CAP_MAPPED_BUFFERS, SCARLET_VIDEO_CAP_SESSIONS, SCARLET_VIDEO_CAP_STATEFUL_AV1,
    SCARLET_VIDEO_CAP_STATEFUL_H264, SCARLET_VIDEO_CAP_STATEFUL_HEVC,
    SCARLET_VIDEO_CAP_STATEFUL_VP9, SCARLET_VIDEO_CAP_VARIABLE_MAPPED_BUFFERS,
    SCARLET_VIDEO_CAPS_VERSION, SCARLET_VIDEO_CREATE_SESSION, SCARLET_VIDEO_DEQUEUE,
    SCARLET_VIDEO_DEQUEUE_SESSION, SCARLET_VIDEO_DESTROY_SESSION, SCARLET_VIDEO_FORMAT_AV1,
    SCARLET_VIDEO_FORMAT_H264, SCARLET_VIDEO_FORMAT_HEVC, SCARLET_VIDEO_FORMAT_VP9,
    SCARLET_VIDEO_FRAME_HEADER_LEN, SCARLET_VIDEO_FRAME_MAGIC, SCARLET_VIDEO_GET_BUFFER,
    SCARLET_VIDEO_GET_CAPS, SCARLET_VIDEO_PIXEL_FORMAT_NV12, SCARLET_VIDEO_SUBMIT,
    SCARLET_VIDEO_SUBMIT_SESSION, ScarletVideoBufferInfo, ScarletVideoCapabilities,
    ScarletVideoDequeuedFrame, ScarletVideoSessionDequeuedFrame, ScarletVideoSessionInfo,
    ScarletVideoSessionSubmit, ScarletVideoSubmit, VIDEO_DEVICE_PATH,
};

const DEQUEUE_POLL_LIMIT: usize = 10_000;
const DEQUEUE_POLL_INTERVAL_MS: u64 = 1;
const PAYLOAD_POOL_MAX_BUFFERS: usize = 1;
const PAYLOAD_POOL_MAX_BYTES: usize = 4 * 1024 * 1024;
const VIDEO_BUFFER_REQUEST_GRANULARITY: usize = 1024 * 1024;
const VIDEO_BUFFER_MIN_INPUT: usize = VIDEO_BUFFER_REQUEST_GRANULARITY;
const VIDEO_BUFFER_MIN_OUTPUT: usize = VIDEO_BUFFER_REQUEST_GRANULARITY;
const VIDEO_HARDWARE_OUTPUT_OFFSET: usize = 4096;

static PAYLOAD_POOL: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

#[cfg(feature = "std")]
fn lock_payload_pool() -> std::sync::MutexGuard<'static, Vec<Vec<u8>>> {
    PAYLOAD_POOL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(feature = "legacy-scarlet-std")]
fn lock_payload_pool() -> std::sync::MutexGuard<'static, Vec<Vec<u8>>> {
    PAYLOAD_POOL.lock()
}

fn read_device(device: &Handle, buffer: &mut [u8]) -> Result<usize, String> {
    device
        .as_stream()
        .map_err(|_| String::from("video device does not support stream reads"))?
        .read(buffer)
        .map_err(|error| format!("video device read failed: {error:?}"))
}

fn write_device(device: &Handle, buffer: &[u8]) -> Result<usize, String> {
    device
        .as_stream()
        .map_err(|_| String::from("video device does not support stream writes"))?
        .write(buffer)
        .map_err(|error| format!("video device write failed: {error:?}"))
}

/// NV12 video-range pixel format returned by the current Scarlet decoders.
pub const NV12_VIDEO_RANGE_PIXEL_FORMAT: u32 = SCARLET_VIDEO_PIXEL_FORMAT_NV12;

/// Timestamp value that requests decoder-assigned monotonic frame identifiers.
///
/// Stateless decoders replace this value before submission so their reference
/// metadata and backend frame identifiers remain consistent.
pub const AUTOMATIC_TIMESTAMP: u64 = 0;

/// Calculate the recommended mapped input capacity for a coded access unit.
///
/// The result follows the Scarlet decoder's allocation granularity and minimum
/// input size.
///
/// # Arguments
///
/// * `required` - Largest complete coded access unit expected by the caller.
///
/// # Returns
///
/// Rounded capacity in bytes, or `None` if the calculation exceeds `u32`.
pub fn recommended_input_buffer_len(required: usize) -> Option<u32> {
    rounded_buffer_request(required, VIDEO_BUFFER_MIN_INPUT)
}

/// Calculate the recommended mapped output capacity for an NV12 stream.
///
/// The result covers both the tight Scarlet frame representation and the
/// stride/scanline alignment required by current hardware backends.
///
/// # Arguments
///
/// * `width` - Coded frame width in pixels.
/// * `height` - Coded frame height in pixels.
///
/// # Returns
///
/// Rounded capacity in bytes, or `None` for zero dimensions or overflow.
pub fn recommended_nv12_output_buffer_len(width: u32, height: u32) -> Option<u32> {
    if width == 0 || height == 0 {
        return None;
    }
    let width = width as usize;
    let height = height as usize;
    let stride = width.checked_add(127)? & !127;
    let y_scanlines = height.checked_add(31)? & !31;
    let uv_height = height.checked_add(1)? / 2;
    let uv_scanlines = uv_height.checked_add(15)? & !15;
    let linear_size = stride.checked_mul(y_scanlines.checked_add(uv_scanlines)?)?;
    let hardware_size = VIDEO_HARDWARE_OUTPUT_OFFSET.checked_add(linear_size)?;
    let tight_size = width
        .checked_mul(height)?
        .checked_add(width.checked_mul(uv_height)?)?
        .checked_add(SCARLET_VIDEO_FRAME_HEADER_LEN)?;
    rounded_buffer_request(hardware_size.max(tight_size), VIDEO_BUFFER_MIN_OUTPUT)
}

/// Coded video format accepted by [`ScarletVideoDecoder`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoFormat {
    /// H.264/AVC Annex B access units.
    H264,
    /// HEVC/H.265 Annex B access units.
    Hevc,
    /// VP9 coded frames.
    Vp9,
    /// AV1 coded access units in Scarlet's stateful AV1 framing.
    Av1,
}

impl VideoFormat {
    fn coded_format(self) -> u32 {
        match self {
            Self::H264 => SCARLET_VIDEO_FORMAT_H264,
            Self::Hevc => SCARLET_VIDEO_FORMAT_HEVC,
            Self::Vp9 => SCARLET_VIDEO_FORMAT_VP9,
            Self::Av1 => SCARLET_VIDEO_FORMAT_AV1,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC",
            Self::Vp9 => "VP9",
            Self::Av1 => "AV1",
        }
    }

    fn stateful_feature_enabled(self) -> bool {
        match self {
            Self::H264 => cfg!(feature = "h264-stateful-hw"),
            Self::Hevc => cfg!(feature = "hevc-stateful-hw"),
            Self::Vp9 => cfg!(feature = "vp9-stateful-hw"),
            Self::Av1 => cfg!(feature = "av1-stateful-hw"),
        }
    }
}

/// Requested mapped input and output capacities for a decoder session.
///
/// Zero lets the backend choose its default capacity. Backends that advertise
/// variable mapped buffers clamp nonzero requests to their supported maxima.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoBufferRequest {
    input_len: u32,
    output_len: u32,
}

impl VideoBufferRequest {
    /// Construct a mapped-buffer capacity request.
    ///
    /// # Arguments
    ///
    /// * `input_len` - Requested coded-input capacity in bytes, or zero for the
    ///   backend default.
    /// * `output_len` - Requested decoded-output capacity in bytes, or zero for
    ///   the backend default.
    ///
    /// # Returns
    ///
    /// A request suitable for [`DecoderOptions::with_buffer_request`].
    pub const fn new(input_len: u32, output_len: u32) -> Self {
        Self {
            input_len,
            output_len,
        }
    }

    /// Return the requested coded-input capacity.
    ///
    /// # Returns
    ///
    /// Capacity in bytes, or zero when the backend should choose.
    pub const fn input_len(self) -> u32 {
        self.input_len
    }

    /// Return the requested decoded-output capacity.
    ///
    /// # Returns
    ///
    /// Capacity in bytes, or zero when the backend should choose.
    pub const fn output_len(self) -> u32 {
        self.output_len
    }
}

/// Options used while opening a Scarlet decoder.
pub struct DecoderOptions {
    buffer_request: VideoBufferRequest,
    cancellation: Arc<AtomicBool>,
}

impl DecoderOptions {
    /// Construct decoder options with backend-selected buffers and a fresh
    /// cancellation flag.
    ///
    /// # Returns
    ///
    /// Default decoder options.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the preferred mapped input and output capacities.
    ///
    /// # Arguments
    ///
    /// * `request` - Capacity request for the decoder session.
    ///
    /// # Returns
    ///
    /// Updated decoder options.
    pub fn with_buffer_request(mut self, request: VideoBufferRequest) -> Self {
        self.buffer_request = request;
        self
    }

    /// Use a shared cancellation flag while waiting for decoded output.
    ///
    /// Setting the flag to `true` interrupts subsequent submit/dequeue work.
    ///
    /// # Arguments
    ///
    /// * `cancellation` - Shared flag controlled by the caller.
    ///
    /// # Returns
    ///
    /// Updated decoder options.
    pub fn with_cancellation(mut self, cancellation: Arc<AtomicBool>) -> Self {
        self.cancellation = cancellation;
        self
    }
}

impl Default for DecoderOptions {
    fn default() -> Self {
        Self {
            buffer_request: VideoBufferRequest::default(),
            cancellation: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Snapshot of capabilities reported by the active Scarlet video backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecoderCapabilities {
    flags: u32,
    max_sessions: u32,
    output_pixel_format: u32,
    mapped_input_len: u32,
    mapped_output_len: u32,
}

impl DecoderCapabilities {
    /// Test whether the backend can decode a format with the enabled client
    /// features.
    ///
    /// # Arguments
    ///
    /// * `format` - Coded format to query.
    ///
    /// # Returns
    ///
    /// `true` when either an enabled stateful path or an enabled stateless path
    /// is available.
    pub fn supports(self, format: VideoFormat) -> bool {
        self.supports_stateful(format)
            || (format == VideoFormat::H264
                && cfg!(feature = "h264-stateless-hw")
                && self.flags & abi::SCARLET_VIDEO_CAP_STATELESS_H264 != 0)
            || (format == VideoFormat::Vp9
                && cfg!(feature = "vp9-stateless-hw")
                && self.flags & abi::SCARLET_VIDEO_CAP_STATELESS_VP9 != 0)
    }

    /// Test whether the backend and client support stateful decoding.
    ///
    /// # Arguments
    ///
    /// * `format` - Coded format to query.
    ///
    /// # Returns
    ///
    /// `true` when the corresponding stateful feature and backend flag are set.
    pub fn supports_stateful(self, format: VideoFormat) -> bool {
        if !format.stateful_feature_enabled() {
            return false;
        }
        let flag = match format {
            VideoFormat::H264 => SCARLET_VIDEO_CAP_STATEFUL_H264,
            VideoFormat::Hevc => SCARLET_VIDEO_CAP_STATEFUL_HEVC,
            VideoFormat::Vp9 => SCARLET_VIDEO_CAP_STATEFUL_VP9,
            VideoFormat::Av1 => SCARLET_VIDEO_CAP_STATEFUL_AV1,
        };
        self.flags & flag != 0
    }

    /// Return the maximum number of backend sessions.
    ///
    /// # Returns
    ///
    /// Maximum simultaneous sessions advertised by the backend.
    pub const fn max_sessions(self) -> u32 {
        self.max_sessions
    }

    /// Return the backend's decoded pixel format identifier.
    ///
    /// # Returns
    ///
    /// Raw Scarlet pixel format value.
    pub const fn output_pixel_format(self) -> u32 {
        self.output_pixel_format
    }

    /// Return the maximum mapped input capacity.
    ///
    /// # Returns
    ///
    /// Input capacity in bytes.
    pub const fn mapped_input_len(self) -> u32 {
        self.mapped_input_len
    }

    /// Return the maximum mapped output capacity.
    ///
    /// # Returns
    ///
    /// Output capacity in bytes.
    pub const fn mapped_output_len(self) -> u32 {
        self.mapped_output_len
    }
}

impl From<ScarletVideoCapabilities> for DecoderCapabilities {
    fn from(caps: ScarletVideoCapabilities) -> Self {
        Self {
            flags: caps.flags,
            max_sessions: caps.max_sessions,
            output_pixel_format: caps.output_pixel_format,
            mapped_input_len: caps.mapped_input_len,
            mapped_output_len: caps.mapped_output_len,
        }
    }
}

struct RecycledBuffer(Vec<u8>);

impl RecycledBuffer {
    fn acquire(len: usize) -> Result<Self, String> {
        let mut pool = lock_payload_pool();
        while let Some(buffer) = pool.pop() {
            if buffer.capacity() >= len {
                drop(pool);
                let mut buffer = buffer;
                buffer.resize(len, 0);
                return Ok(Self(buffer));
            }
        }
        drop(pool);

        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(len)
            .map_err(|_| format!("hardware decoder payload allocation failed: {len} bytes"))?;
        buffer.resize(len, 0);
        Ok(Self(buffer))
    }
}

impl Drop for RecycledBuffer {
    fn drop(&mut self) {
        let buffer = core::mem::take(&mut self.0);
        let mut pool = lock_payload_pool();
        let retained_bytes = pool.iter().fold(0usize, |total, retained| {
            total.saturating_add(retained.capacity())
        });
        if pool.len() < PAYLOAD_POOL_MAX_BUFFERS
            && (pool.is_empty()
                || retained_bytes.saturating_add(buffer.capacity()) <= PAYLOAD_POOL_MAX_BYTES)
        {
            pool.push(buffer);
        }
    }
}

enum DecodedPayload<'decoder> {
    Owned(RecycledBuffer),
    Mapped(&'decoder [u8]),
}

/// A decoded NV12 frame borrowed from a decoder dequeue operation.
///
/// Mapped frames borrow the decoder's reusable output buffer. Convert the frame
/// with [`DecodedFrame::try_into_owned`] before storing it beyond the current
/// decode iteration or sending it to another thread.
pub struct DecodedFrame<'decoder> {
    width: u32,
    height: u32,
    timestamp: u64,
    flags: u32,
    payload: DecodedPayload<'decoder>,
}

impl DecodedFrame<'_> {
    /// Return the decoded width.
    ///
    /// # Returns
    ///
    /// Width in pixels.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Return the decoded height.
    ///
    /// # Returns
    ///
    /// Height in pixels.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Return the timestamp associated with the decode request.
    ///
    /// # Returns
    ///
    /// Backend-returned timestamp for mapped decoding, or the submitted
    /// timestamp for the legacy stream path.
    pub const fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// Return backend frame flags.
    ///
    /// # Returns
    ///
    /// Raw Scarlet frame flags. They are currently zero.
    pub const fn flags(&self) -> u32 {
        self.flags
    }

    /// Borrow the contiguous NV12 video-range payload.
    ///
    /// # Returns
    ///
    /// NV12 bytes containing a full Y plane followed by interleaved UV data.
    pub fn payload(&self) -> &[u8] {
        match &self.payload {
            DecodedPayload::Owned(buffer) => &buffer.0,
            DecodedPayload::Mapped(payload) => payload,
        }
    }

    /// Copy or move this frame into thread-safe owned storage.
    ///
    /// # Returns
    ///
    /// An owned frame, or an allocation error when a mapped payload cannot be
    /// copied.
    pub fn try_into_owned(self) -> Result<OwnedDecodedFrame, String> {
        let payload = match self.payload {
            DecodedPayload::Owned(payload) => payload,
            DecodedPayload::Mapped(payload) => {
                let mut owned = RecycledBuffer::acquire(payload.len())?;
                owned.0.copy_from_slice(payload);
                owned
            }
        };
        Ok(OwnedDecodedFrame {
            width: self.width,
            height: self.height,
            timestamp: self.timestamp,
            flags: self.flags,
            payload,
        })
    }
}

/// An owned decoded NV12 frame that may outlive its decoder borrow.
pub struct OwnedDecodedFrame {
    width: u32,
    height: u32,
    timestamp: u64,
    flags: u32,
    payload: RecycledBuffer,
}

impl OwnedDecodedFrame {
    /// Return the decoded width.
    ///
    /// # Returns
    ///
    /// Width in pixels.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Return the decoded height.
    ///
    /// # Returns
    ///
    /// Height in pixels.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Return the timestamp associated with the decode request.
    ///
    /// # Returns
    ///
    /// Backend-returned timestamp for mapped decoding, or the submitted
    /// timestamp for the legacy stream path.
    pub const fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// Return backend frame flags.
    ///
    /// # Returns
    ///
    /// Raw Scarlet frame flags. They are currently zero.
    pub const fn flags(&self) -> u32 {
        self.flags
    }

    /// Borrow the contiguous NV12 video-range payload.
    ///
    /// # Returns
    ///
    /// NV12 bytes containing a full Y plane followed by interleaved UV data.
    pub fn payload(&self) -> &[u8] {
        &self.payload.0
    }
}

#[derive(Clone, Copy)]
struct MappedVideoBuffer {
    stream_id: u32,
    session_commands: bool,
    coded_format: u32,
    ptr: *mut u8,
    mmap_len: usize,
    input_offset: usize,
    input_len: usize,
    output_offset: usize,
    output_len: usize,
}

impl MappedVideoBuffer {
    fn payload(&self, payload_offset: u64, payload_len: usize) -> Result<&[u8], String> {
        let payload_offset = usize::try_from(payload_offset)
            .map_err(|_| String::from("hardware decoder mmap payload offset overflow"))?;
        let end = payload_offset
            .checked_add(payload_len)
            .ok_or_else(|| String::from("hardware decoder mmap payload length overflow"))?;
        let output_end = self
            .output_offset
            .checked_add(self.output_len)
            .ok_or_else(|| String::from("hardware decoder mmap output length overflow"))?;
        if payload_offset < self.output_offset || end > output_end || end > self.mmap_len {
            return Err(String::from(
                "hardware decoder returned invalid mmap payload range",
            ));
        }
        // SAFETY: the validated range is inside this live decoder mapping. The
        // returned lifetime is tied to the mapped-buffer borrow, and the
        // decoder API prevents a second submit while that borrow is live.
        Ok(unsafe { core::slice::from_raw_parts(self.ptr.add(payload_offset), payload_len) })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HardwareDecodeMode {
    Stateful,
    StatelessH264,
    StatelessVp9,
}

#[derive(Clone, Copy)]
enum PendingTransport {
    Stream,
    Mapped { should_display: bool },
}

#[derive(Clone, Copy)]
struct PendingDecode {
    transport: PendingTransport,
    submitted_timestamp: u64,
}

/// Decoder session backed by a Scarlet `/dev/video*` device.
pub struct ScarletVideoDecoder {
    device: Handle,
    mapped: Option<MappedVideoBuffer>,
    caps: Option<ScarletVideoCapabilities>,
    buffer_request: VideoBufferRequest,
    configured_format: Option<VideoFormat>,
    pending: Option<PendingDecode>,
    last_decode_mode: Option<HardwareDecodeMode>,
    h264_stateless_context: h264_stateless::Context,
    vp9_stateless_context: vp9_stateless::Context,
    vp9_debug_submits: u32,
    cancellation: Arc<AtomicBool>,
}

impl ScarletVideoDecoder {
    /// Open the default Scarlet video decoder with backend-selected buffers.
    ///
    /// # Returns
    ///
    /// A decoder ready to be configured, or an error if `/dev/video0` cannot
    /// be opened or initialized.
    pub fn open() -> Result<Self, String> {
        Self::open_with_options(DecoderOptions::default())
    }

    /// Open the default Scarlet video decoder with explicit options.
    ///
    /// # Arguments
    ///
    /// * `options` - Buffer sizing and cancellation configuration.
    ///
    /// # Returns
    ///
    /// A decoder ready to be configured, or an initialization error.
    pub fn open_with_options(options: DecoderOptions) -> Result<Self, String> {
        if options.cancellation.load(Ordering::Acquire) {
            return Err(String::from("hardware decoder open cancelled"));
        }
        let device = Handle::open(VIDEO_DEVICE_PATH, 0x2)
            .map_err(|_| format!("failed to open {VIDEO_DEVICE_PATH}"))?;
        let caps = Self::query_capabilities(&device);
        if let Some(caps) = caps {
            std::println!(
                "[scarlet-video-client] caps flags=0x{:x} stateful_h264={} stateful_av1={} stateful_hevc={} stateful_vp9={} stateless_h264={} stateless_vp9={}",
                caps.flags,
                caps.has_flag(SCARLET_VIDEO_CAP_STATEFUL_H264),
                caps.has_flag(SCARLET_VIDEO_CAP_STATEFUL_AV1),
                caps.has_flag(SCARLET_VIDEO_CAP_STATEFUL_HEVC),
                caps.has_flag(SCARLET_VIDEO_CAP_STATEFUL_VP9),
                caps.has_flag(abi::SCARLET_VIDEO_CAP_STATELESS_H264),
                caps.has_flag(abi::SCARLET_VIDEO_CAP_STATELESS_VP9),
            );
        }
        let mapped = Self::map_video_buffer(&device, caps, options.buffer_request);
        if let Some(buffer) = &mapped {
            std::println!(
                "[scarlet-video-client] mmap input={} output={}",
                buffer.input_len,
                buffer.output_len
            );
        }
        Ok(Self {
            device,
            mapped,
            caps,
            buffer_request: options.buffer_request,
            configured_format: None,
            pending: None,
            last_decode_mode: None,
            h264_stateless_context: h264_stateless::Context::default(),
            vp9_stateless_context: vp9_stateless::Context::default(),
            vp9_debug_submits: 0,
            cancellation: options.cancellation,
        })
    }

    /// Return the capabilities reported by the backend.
    ///
    /// # Returns
    ///
    /// A capability snapshot, or `None` for a legacy backend without the caps
    /// control command.
    pub fn capabilities(&self) -> Option<DecoderCapabilities> {
        self.caps.map(DecoderCapabilities::from)
    }

    /// Configure the coded format for subsequent submissions.
    ///
    /// This call also switches the mapped session format when required.
    ///
    /// # Arguments
    ///
    /// * `format` - Format of complete access units passed to [`Self::submit`].
    ///
    /// # Returns
    ///
    /// `Ok(())` when the active backend and enabled crate features support the
    /// format, otherwise a descriptive error.
    pub fn configure(&mut self, format: VideoFormat) -> Result<(), String> {
        if self.pending.is_some() {
            return Err(String::from(
                "hardware decoder cannot reconfigure with a pending decode",
            ));
        }
        if !self.supports_decode_format(format) {
            return Err(format!(
                "hardware decoder does not support {}",
                format.name()
            ));
        }
        if self.mapped.is_some() {
            self.ensure_mapped_session_format(format)?;
        }
        self.configured_format = Some(format);
        Ok(())
    }

    /// Submit one complete coded access unit.
    ///
    /// Stateful H.264 access units use the mapped/session API when possible and
    /// fall back to the legacy stream API when the access unit is larger than
    /// the mapped input. Stateless backends build their request parameters in
    /// userspace automatically.
    ///
    /// # Arguments
    ///
    /// * `access_unit` - Complete coded access unit in the configured format.
    /// * `timestamp` - Presentation timestamp to carry through dequeue. Zero
    ///   requests automatic nonzero assignment on stateless backends. A
    ///   caller-provided value must remain unique while a stateless reference
    ///   frame can use it.
    ///
    /// # Returns
    ///
    /// `Ok(())` after the request is accepted. Call [`Self::dequeue`] before
    /// submitting another request.
    pub fn submit(&mut self, access_unit: &[u8], timestamp: u64) -> Result<(), String> {
        if self.is_cancelled() {
            return Err(String::from("hardware decoder submit cancelled"));
        }
        if self.pending.is_some() {
            return Err(String::from(
                "hardware decoder already has a pending decode",
            ));
        }
        if access_unit.is_empty() {
            return Err(String::from("hardware decoder input is empty"));
        }
        let format = self
            .configured_format
            .ok_or_else(|| String::from("hardware decoder is not configured"))?;

        if self
            .mapped
            .is_some_and(|buffer| access_unit.len() <= buffer.input_len)
        {
            return self.submit_mapped(format, access_unit, timestamp);
        }
        if !self.supports_stateful_format(format) {
            return Err(format!(
                "hardware decoder stateless {} requires mmap input",
                format.name()
            ));
        }
        if format != VideoFormat::H264 {
            return Err(format!(
                "hardware decoder mmap input overflow for {} access unit",
                format.name()
            ));
        }

        let written = write_device(&self.device, access_unit).map_err(|error| {
            let status = self.read_decoder_status();
            format!("hardware decoder write failed: {error}{status}")
        })?;
        if written != access_unit.len() {
            return Err(format!(
                "hardware decoder accepted only {written} of {} bytes",
                access_unit.len()
            ));
        }
        self.last_decode_mode = Some(HardwareDecodeMode::Stateful);
        self.pending = Some(PendingDecode {
            transport: PendingTransport::Stream,
            submitted_timestamp: timestamp,
        });
        Ok(())
    }

    /// Wait for and dequeue the result of the pending submission.
    ///
    /// Only one decode may be pending. VP9 frames with `show_frame = 0` are
    /// fully drained and returned as `None`.
    ///
    /// # Returns
    ///
    /// A validated NV12 frame, `None` for a non-display frame or cancellation,
    /// or an error for invalid state/backend output.
    pub fn dequeue(&mut self) -> Result<Option<DecodedFrame<'_>>, String> {
        let pending = self
            .pending
            .ok_or_else(|| String::from("hardware decoder has no pending decode"))?;
        match pending.transport {
            PendingTransport::Stream => self.dequeue_stream(pending.submitted_timestamp),
            PendingTransport::Mapped { should_display } => self.dequeue_mapped(should_display),
        }
    }

    /// Configure, submit, and dequeue one access unit.
    ///
    /// This convenience operation preserves the same single-pending-request
    /// semantics as separate [`Self::submit`] and [`Self::dequeue`] calls.
    ///
    /// # Arguments
    ///
    /// * `format` - Format of the coded access unit.
    /// * `access_unit` - Complete coded access unit.
    /// * `timestamp` - Presentation timestamp to carry through dequeue.
    ///
    /// # Returns
    ///
    /// A decoded NV12 frame, or `None` for an empty/non-display access unit or
    /// cancellation.
    pub fn decode(
        &mut self,
        format: VideoFormat,
        access_unit: &[u8],
        timestamp: u64,
    ) -> Result<Option<DecodedFrame<'_>>, String> {
        if self.is_cancelled() || access_unit.is_empty() {
            return Ok(None);
        }
        if self.configured_format != Some(format) {
            self.configure(format)?;
        }
        self.submit(access_unit, timestamp)?;
        self.dequeue()
    }

    /// Reset or recreate the decoder after a stream discontinuity.
    ///
    /// Stateless H.264/VP9 contexts can be reset in place at a random-access
    /// point. Stateful sessions are recreated because the current device ABI
    /// has no flush command.
    ///
    /// # Arguments
    ///
    /// * `format` - Format decoded after the discontinuity.
    /// * `random_access` - Whether decoding resumes at an independent random-
    ///   access picture.
    ///
    /// # Returns
    ///
    /// A configured decoder with stale reference state removed.
    pub fn restart_for_discontinuity(
        mut self,
        format: VideoFormat,
        random_access: bool,
    ) -> Result<Self, String> {
        if self.pending.is_some() {
            return Err(String::from(
                "hardware decoder cannot restart with a pending decode",
            ));
        }
        let reusable_h264 = self.last_decode_mode.is_none()
            || self.last_decode_mode == Some(HardwareDecodeMode::StatelessH264);
        if random_access
            && format == VideoFormat::H264
            && reusable_h264
            && self.supports_stateless_h264()
        {
            self.configure(format)?;
            h264_stateless::reset_for_discontinuity(&mut self.h264_stateless_context);
            return Ok(self);
        }
        let reusable_vp9 = self.last_decode_mode.is_none()
            || self.last_decode_mode == Some(HardwareDecodeMode::StatelessVp9);
        if random_access
            && format == VideoFormat::Vp9
            && reusable_vp9
            && self.supports_stateless_vp9()
        {
            self.configure(format)?;
            vp9_stateless::reset_for_discontinuity(&mut self.vp9_stateless_context);
            self.vp9_debug_submits = 0;
            return Ok(self);
        }

        let options = DecoderOptions::new()
            .with_buffer_request(self.buffer_request)
            .with_cancellation(self.cancellation.clone());
        drop(self);
        let mut decoder = Self::open_with_options(options)?;
        decoder.configure(format)?;
        Ok(decoder)
    }

    fn is_cancelled(&self) -> bool {
        self.cancellation.load(Ordering::Acquire)
    }

    fn supports_decode_format(&self, format: VideoFormat) -> bool {
        self.supports_stateful_format(format)
            || (format == VideoFormat::H264 && self.supports_stateless_h264())
            || (format == VideoFormat::Vp9 && self.supports_stateless_vp9())
    }

    fn supports_stateful_format(&self, format: VideoFormat) -> bool {
        if !format.stateful_feature_enabled() {
            return false;
        }
        let Some(caps) = self.caps else {
            return format != VideoFormat::Vp9;
        };
        match format {
            VideoFormat::H264 => caps.has_flag(SCARLET_VIDEO_CAP_STATEFUL_H264),
            VideoFormat::Hevc => caps.has_flag(SCARLET_VIDEO_CAP_STATEFUL_HEVC),
            VideoFormat::Vp9 => caps.has_flag(SCARLET_VIDEO_CAP_STATEFUL_VP9),
            VideoFormat::Av1 => caps.has_flag(SCARLET_VIDEO_CAP_STATEFUL_AV1),
        }
    }

    fn supports_stateless_h264(&self) -> bool {
        h264_stateless::supported(self.caps)
    }

    fn supports_stateless_vp9(&self) -> bool {
        vp9_stateless::supported(self.caps)
    }

    fn submit_mapped(
        &mut self,
        format: VideoFormat,
        access_unit: &[u8],
        timestamp: u64,
    ) -> Result<(), String> {
        let buffer = self.ensure_mapped_session_format(format)?;
        if access_unit.len() > buffer.input_len {
            return Err(String::from("hardware decoder mmap input overflow"));
        }

        // SAFETY: the mapped input area is writable for `input_len` bytes, the
        // access unit was checked to fit, and it does not overlap the mapping.
        unsafe {
            core::ptr::copy_nonoverlapping(
                access_unit.as_ptr(),
                buffer.ptr.add(buffer.input_offset),
                access_unit.len(),
            );
        }

        let mut should_display = true;
        let submitted_timestamp;
        if format == VideoFormat::H264 && self.supports_stateless_h264() {
            self.last_decode_mode = Some(HardwareDecodeMode::StatelessH264);
            submitted_timestamp = h264_stateless::submit(
                &mut self.device,
                &mut self.h264_stateless_context,
                buffer.stream_id,
                access_unit,
                timestamp,
            )?;
        } else if format == VideoFormat::Vp9 && self.supports_stateless_vp9() {
            self.last_decode_mode = Some(HardwareDecodeMode::StatelessVp9);
            let submitted = vp9_stateless::submit(
                &mut self.device,
                &mut self.vp9_stateless_context,
                buffer.stream_id,
                access_unit,
                timestamp,
                &mut self.vp9_debug_submits,
            )?;
            should_display = submitted.0;
            submitted_timestamp = submitted.1;
        } else if buffer.session_commands {
            self.last_decode_mode = Some(HardwareDecodeMode::Stateful);
            let submit = ScarletVideoSessionSubmit {
                stream_id: buffer.stream_id,
                input_len: access_unit.len() as u32,
                coded_format: format.coded_format(),
                padding: 0,
                timestamp,
            };
            self.device
                .control(SCARLET_VIDEO_SUBMIT_SESSION, &submit as *const _ as usize)
                .map_err(|_| {
                    let status = self.read_decoder_status();
                    format!("hardware decoder mmap submit failed{status}")
                })?;
            submitted_timestamp = timestamp;
        } else {
            self.last_decode_mode = Some(HardwareDecodeMode::Stateful);
            let submit = ScarletVideoSubmit {
                input_len: access_unit.len() as u32,
                coded_format: format.coded_format(),
                timestamp,
            };
            self.device
                .control(SCARLET_VIDEO_SUBMIT, &submit as *const _ as usize)
                .map_err(|_| {
                    let status = self.read_decoder_status();
                    format!("hardware decoder mmap submit failed{status}")
                })?;
            submitted_timestamp = timestamp;
        }
        self.pending = Some(PendingDecode {
            transport: PendingTransport::Mapped { should_display },
            submitted_timestamp,
        });
        Ok(())
    }

    fn dequeue_stream(&mut self, timestamp: u64) -> Result<Option<DecodedFrame<'_>>, String> {
        let mut header = [0u8; SCARLET_VIDEO_FRAME_HEADER_LEN];
        if let Err(error) = read_exact_file(&mut self.device, &mut header, &self.cancellation) {
            self.pending = None;
            return if self.is_cancelled() {
                Ok(None)
            } else {
                Err(error)
            };
        }
        if &header[0..4] != SCARLET_VIDEO_FRAME_MAGIC {
            self.pending = None;
            let text = core::str::from_utf8(&header).unwrap_or("");
            return Err(format!(
                "hardware decoder returned invalid frame magic {:02x} {:02x} {:02x} {:02x} {}",
                header[0], header[1], header[2], header[3], text
            ));
        }

        let width = read_u32_le(&header[4..8]);
        let height = read_u32_le(&header[8..12]);
        let pixel_format = read_u32_le(&header[12..16]);
        let payload_len = read_u32_le(&header[16..20]) as usize;
        self.pending = None;
        validate_nv12_frame(width, height, pixel_format, payload_len)?;

        let mut payload = RecycledBuffer::acquire(payload_len)?;
        if let Err(error) = read_exact_file(&mut self.device, &mut payload.0, &self.cancellation) {
            self.pending = None;
            return if self.is_cancelled() {
                Ok(None)
            } else {
                Err(error)
            };
        }
        Ok(Some(DecodedFrame {
            width,
            height,
            timestamp,
            flags: 0,
            payload: DecodedPayload::Owned(payload),
        }))
    }

    fn dequeue_mapped(&mut self, should_display: bool) -> Result<Option<DecodedFrame<'_>>, String> {
        let buffer = self
            .mapped
            .ok_or_else(|| String::from("hardware decoder mmap buffer disappeared"))?;
        let mut empty_polls = 0usize;
        let frame = loop {
            if self.is_cancelled() {
                self.pending = None;
                return Ok(None);
            }
            let dequeue_result = if buffer.session_commands {
                let mut session_frame = ScarletVideoSessionDequeuedFrame {
                    stream_id: buffer.stream_id,
                    ..Default::default()
                };
                let result = self.device.control(
                    SCARLET_VIDEO_DEQUEUE_SESSION,
                    &mut session_frame as *mut _ as usize,
                );
                result.map(|value| (value, session_frame.frame))
            } else {
                let mut frame = ScarletVideoDequeuedFrame::default();
                let result = self
                    .device
                    .control(SCARLET_VIDEO_DEQUEUE, &mut frame as *mut _ as usize);
                result.map(|value| (value, frame))
            };
            match dequeue_result {
                Ok((1, frame)) => break frame,
                Ok((0, _)) => {
                    empty_polls += 1;
                    if empty_polls > DEQUEUE_POLL_LIMIT {
                        self.pending = None;
                        return Err(String::from(
                            "hardware decoder timed out before mmap frame was complete",
                        ));
                    }
                    thread::sleep(Duration::from_millis(DEQUEUE_POLL_INTERVAL_MS));
                }
                Ok((_, _)) => {
                    self.pending = None;
                    return Err(String::from(
                        "hardware decoder returned invalid dequeue result",
                    ));
                }
                Err(_) => {
                    self.pending = None;
                    let status = self.read_decoder_status();
                    return Err(format!("hardware decoder mmap dequeue failed{status}"));
                }
            }
        };

        self.pending = None;
        if !should_display {
            return Ok(None);
        }
        validate_nv12_frame(
            frame.width,
            frame.height,
            frame.pixel_format,
            frame.payload_len as usize,
        )?;
        let mapped = self
            .mapped
            .as_ref()
            .ok_or_else(|| String::from("hardware decoder mmap buffer disappeared"))?;
        let payload = mapped.payload(frame.payload_offset, frame.payload_len as usize)?;
        Ok(Some(DecodedFrame {
            width: frame.width,
            height: frame.height,
            timestamp: frame.timestamp,
            flags: frame.flags,
            payload: DecodedPayload::Mapped(payload),
        }))
    }

    fn ensure_mapped_session_format(
        &mut self,
        format: VideoFormat,
    ) -> Result<MappedVideoBuffer, String> {
        let Some(mut buffer) = self.mapped else {
            return Err(String::from("hardware decoder mmap buffer is unavailable"));
        };
        let coded_format = format.coded_format();
        if !buffer.session_commands || buffer.coded_format == coded_format {
            return Ok(buffer);
        }

        let destroy = ScarletVideoSessionInfo {
            stream_id: buffer.stream_id,
            padding: 0,
            buffer: ScarletVideoBufferInfo::default(),
        };
        let _ = self
            .device
            .control(SCARLET_VIDEO_DESTROY_SESSION, &destroy as *const _ as usize);

        let mut session_info = ScarletVideoSessionInfo {
            stream_id: 0,
            padding: coded_format,
            buffer: ScarletVideoBufferInfo {
                input_len: buffer.input_len as u32,
                output_len: buffer.output_len as u32,
                ..ScarletVideoBufferInfo::default()
            },
        };
        self.device
            .control(
                SCARLET_VIDEO_CREATE_SESSION,
                &mut session_info as *mut _ as usize,
            )
            .map_err(|_| {
                let status = self.read_decoder_status();
                format!(
                    "hardware decoder failed to create {} session{status}",
                    format.name()
                )
            })?;
        if session_info.buffer.mmap_len as usize != buffer.mmap_len {
            return Err(String::from(
                "hardware decoder changed mmap layout while switching codec sessions",
            ));
        }
        buffer.stream_id = session_info.stream_id;
        buffer.coded_format = coded_format;
        buffer.input_offset = session_info.buffer.input_offset as usize;
        buffer.input_len = session_info.buffer.input_len as usize;
        buffer.output_offset = session_info.buffer.output_offset as usize;
        buffer.output_len = session_info.buffer.output_len as usize;
        self.mapped = Some(buffer);
        Ok(buffer)
    }

    fn map_video_buffer(
        device: &Handle,
        caps: Option<ScarletVideoCapabilities>,
        buffer_request: VideoBufferRequest,
    ) -> Option<MappedVideoBuffer> {
        if caps
            .map(|caps| caps.has_flag(SCARLET_VIDEO_CAP_MAPPED_BUFFERS))
            .is_some_and(|has_mapped_buffers| !has_mapped_buffers)
        {
            return None;
        }
        let use_session_commands = caps
            .map(|caps| caps.has_flag(SCARLET_VIDEO_CAP_SESSIONS))
            .unwrap_or(true);
        let (stream_id, session_commands, info) = if use_session_commands {
            let request = caps
                .filter(|caps| caps.has_flag(SCARLET_VIDEO_CAP_VARIABLE_MAPPED_BUFFERS))
                .map(|caps| VideoBufferRequest {
                    input_len: if buffer_request.input_len == 0 {
                        0
                    } else {
                        buffer_request.input_len.min(caps.mapped_input_len)
                    },
                    output_len: if buffer_request.output_len == 0 {
                        0
                    } else {
                        buffer_request.output_len.min(caps.mapped_output_len)
                    },
                })
                .unwrap_or_default();
            let mut session_info = ScarletVideoSessionInfo {
                buffer: ScarletVideoBufferInfo {
                    input_len: request.input_len,
                    output_len: request.output_len,
                    ..ScarletVideoBufferInfo::default()
                },
                ..ScarletVideoSessionInfo::default()
            };
            if device
                .control(
                    SCARLET_VIDEO_CREATE_SESSION,
                    &mut session_info as *mut _ as usize,
                )
                .is_ok()
            {
                (session_info.stream_id, true, session_info.buffer)
            } else {
                let mut info = ScarletVideoBufferInfo::default();
                device
                    .control(SCARLET_VIDEO_GET_BUFFER, &mut info as *mut _ as usize)
                    .ok()?;
                (1, false, info)
            }
        } else {
            let mut info = ScarletVideoBufferInfo::default();
            device
                .control(SCARLET_VIDEO_GET_BUFFER, &mut info as *mut _ as usize)
                .ok()?;
            (1, false, info)
        };
        let mapper = device.as_memory_mapping().ok()?;
        let address = mapper
            .mmap(
                0,
                info.mmap_len as usize,
                prot::READ | prot::WRITE,
                mmap_flags::SHARED,
                info.mmap_offset as usize,
            )
            .ok()?;
        Some(MappedVideoBuffer {
            stream_id,
            session_commands,
            coded_format: SCARLET_VIDEO_FORMAT_H264,
            ptr: address as *mut u8,
            mmap_len: info.mmap_len as usize,
            input_offset: info.input_offset as usize,
            input_len: info.input_len as usize,
            output_offset: info.output_offset as usize,
            output_len: info.output_len as usize,
        })
    }

    fn query_capabilities(device: &Handle) -> Option<ScarletVideoCapabilities> {
        let mut caps = ScarletVideoCapabilities::default();
        device
            .control(SCARLET_VIDEO_GET_CAPS, &mut caps as *mut _ as usize)
            .ok()?;
        (caps.version == SCARLET_VIDEO_CAPS_VERSION).then_some(caps)
    }

    fn read_decoder_status(&mut self) -> String {
        let mut buffer = [0u8; 512];
        match read_device(&self.device, &mut buffer) {
            Ok(0) | Err(_) => String::new(),
            Ok(read) => {
                let status = core::str::from_utf8(&buffer[..read]).unwrap_or("<non-utf8 status>");
                format!("; {status}")
            }
        }
    }
}

impl Drop for ScarletVideoDecoder {
    fn drop(&mut self) {
        if let Some(buffer) = self.mapped.take() {
            if buffer.session_commands {
                let info = ScarletVideoSessionInfo {
                    stream_id: buffer.stream_id,
                    padding: 0,
                    buffer: ScarletVideoBufferInfo::default(),
                };
                let _ = self
                    .device
                    .control(SCARLET_VIDEO_DESTROY_SESSION, &info as *const _ as usize);
            }
            let _ = munmap(buffer.ptr as usize, buffer.mmap_len);
        }
    }
}

/// Enable diagnostic dumps for stateless VP9 submissions.
///
/// The first nonempty path wins for the lifetime of the process. When the
/// `vp9-stateless-hw` feature is disabled, the request is logged and ignored.
///
/// # Arguments
///
/// * `path` - Directory receiving input, parameter, probability, tile, and
///   manifest files.
pub fn enable_vp9_stateless_dump(path: &str) {
    vp9_stateless::enable_dump(path);
}

fn validate_nv12_frame(
    width: u32,
    height: u32,
    pixel_format: u32,
    payload_len: usize,
) -> Result<(), String> {
    if width == 0 || height == 0 || payload_len == 0 {
        return Err(String::from("hardware decoder returned empty frame"));
    }
    if pixel_format != SCARLET_VIDEO_PIXEL_FORMAT_NV12 {
        return Err(format!(
            "hardware decoder returned unsupported pixel format 0x{pixel_format:08x}"
        ));
    }
    let required_len = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(3))
        .map(|bytes| bytes / 2)
        .ok_or_else(|| String::from("hardware decoder NV12 frame size overflow"))?;
    if payload_len < required_len {
        return Err(String::from(
            "hardware decoder returned truncated NV12 frame",
        ));
    }
    Ok(())
}

fn read_exact_file(
    file: &Handle,
    output: &mut [u8],
    cancellation: &AtomicBool,
) -> Result<(), String> {
    let mut read = 0usize;
    let mut empty_reads = 0usize;
    while read < output.len() {
        if cancellation.load(Ordering::Acquire) {
            return Err(String::from("hardware decoder read cancelled"));
        }
        let count = read_device(file, &mut output[read..])?;
        if count == 0 {
            empty_reads += 1;
            if empty_reads > DEQUEUE_POLL_LIMIT {
                return Err(String::from(
                    "hardware decoder timed out before frame was complete",
                ));
            }
            thread::sleep(Duration::from_millis(DEQUEUE_POLL_INTERVAL_MS));
            continue;
        }
        empty_reads = 0;
        read += count;
    }
    Ok(())
}

fn read_u32_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn rounded_buffer_request(required: usize, minimum: usize) -> Option<u32> {
    let required = required.max(minimum);
    let aligned = required.checked_add(VIDEO_BUFFER_REQUEST_GRANULARITY - 1)?
        & !(VIDEO_BUFFER_REQUEST_GRANULARITY - 1);
    u32::try_from(aligned).ok()
}
