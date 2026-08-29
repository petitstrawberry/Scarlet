//! Shared Scarlet video decode device ABI definitions.
//!
//! The video decode character device API is currently experimental, but both
//! VirtIO video and Apple AVD backends use the same user-visible control
//! contract. Keep the command values, mapped-buffer structures, and frame
//! constants here so backend implementations do not drift apart.

use alloc::{
    collections::VecDeque,
    format,
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::Any;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::arch::Trapframe;
use crate::device::{Device, DeviceType, char::CharDevice, manager::DeviceManager};
use crate::environment::PAGE_SIZE;
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::mem::page::ContiguousPages;
use crate::object::capability::{
    ControlOps, MemoryMappingInfo, MemoryMappingOps,
    selectable::{ReadyInterest, ReadySet, SelectWaitOutcome, Selectable},
};
use crate::sync::{IrqGuard, IrqSpinLock, Waker};
use crate::task::mytask;

/// FourCC-like Scarlet frame stream magic.
pub const SCARLET_VIDEO_FRAME_MAGIC: &[u8; 4] = b"SVF1";
/// Length of the `SVF1` frame header in bytes.
pub const SCARLET_VIDEO_FRAME_HEADER_LEN: usize = 20;
/// NV12 video-range pixel format value expected by `video_player`.
pub const SCARLET_VIDEO_PIXEL_FORMAT_NV12: u32 = 0x3432_3076;

/// Query the mapped buffer layout for the single-session path.
pub const SCARLET_VIDEO_GET_BUFFER: u32 = 0x5600;
/// Submit a coded access unit for the single-session path.
pub const SCARLET_VIDEO_SUBMIT: u32 = 0x5601;
/// Dequeue a decoded frame for the single-session path.
pub const SCARLET_VIDEO_DEQUEUE: u32 = 0x5602;
/// Create or query a mapped video session.
pub const SCARLET_VIDEO_CREATE_SESSION: u32 = 0x5603;
/// Submit a coded access unit for a mapped video session.
pub const SCARLET_VIDEO_SUBMIT_SESSION: u32 = 0x5604;
/// Dequeue a decoded frame for a mapped video session.
pub const SCARLET_VIDEO_DEQUEUE_SESSION: u32 = 0x5605;
/// Destroy a mapped video session.
pub const SCARLET_VIDEO_DESTROY_SESSION: u32 = 0x5606;
/// Query backend capabilities with explicit stateful/stateless mode flags.
pub const SCARLET_VIDEO_GET_CAPS: u32 = 0x5607;
/// Submit a stateless H.264 decode request for a mapped video session.
pub const SCARLET_VIDEO_SUBMIT_H264_STATELESS: u32 = 0x5608;
/// Submit a stateless VP9 decode request for a mapped video session.
pub const SCARLET_VIDEO_SUBMIT_VP9_STATELESS: u32 = 0x5609;

/// Version of `ScarletVideoCapabilities`.
pub const SCARLET_VIDEO_CAPS_VERSION: u32 = 1;
/// Backend accepts stateful H.264 access units through legacy submit ioctls.
pub const SCARLET_VIDEO_CAP_STATEFUL_H264: u32 = 1 << 0;
/// Backend accepts stateful AV1 access units through legacy submit ioctls.
pub const SCARLET_VIDEO_CAP_STATEFUL_AV1: u32 = 1 << 1;
/// Backend accepts stateful HEVC access units through legacy submit ioctls.
pub const SCARLET_VIDEO_CAP_STATEFUL_HEVC: u32 = 1 << 2;
/// Backend accepts stateful VP9 access units through legacy submit ioctls.
pub const SCARLET_VIDEO_CAP_STATEFUL_VP9: u32 = 1 << 3;
/// Backend accepts stateless H.264 requests.
pub const SCARLET_VIDEO_CAP_STATELESS_H264: u32 = 1 << 8;
/// Backend accepts stateless VP9 requests.
pub const SCARLET_VIDEO_CAP_STATELESS_VP9: u32 = 1 << 9;
/// Backend supports the common mmap input/output buffer.
pub const SCARLET_VIDEO_CAP_MAPPED_BUFFERS: u32 = 1 << 16;
/// Backend supports multiple mapped stream sessions.
pub const SCARLET_VIDEO_CAP_SESSIONS: u32 = 1 << 17;

/// H.264 SPS has `separate_colour_plane_flag` set.
pub const SCARLET_VIDEO_H264_SPS_FLAG_SEPARATE_COLOUR_PLANE: u32 = 1 << 0;
/// H.264 SPS has `qpprime_y_zero_transform_bypass_flag` set.
pub const SCARLET_VIDEO_H264_SPS_FLAG_QPPRIME_Y_ZERO_TRANSFORM_BYPASS: u32 = 1 << 1;
/// H.264 SPS has `delta_pic_order_always_zero_flag` set.
pub const SCARLET_VIDEO_H264_SPS_FLAG_DELTA_PIC_ORDER_ALWAYS_ZERO: u32 = 1 << 2;
/// H.264 SPS has `gaps_in_frame_num_value_allowed_flag` set.
pub const SCARLET_VIDEO_H264_SPS_FLAG_GAPS_IN_FRAME_NUM_VALUE_ALLOWED: u32 = 1 << 3;
/// H.264 SPS has `frame_mbs_only_flag` set.
pub const SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY: u32 = 1 << 4;
/// H.264 SPS has `mb_adaptive_frame_field_flag` set.
pub const SCARLET_VIDEO_H264_SPS_FLAG_MB_ADAPTIVE_FRAME_FIELD: u32 = 1 << 5;
/// H.264 SPS has `direct_8x8_inference_flag` set.
pub const SCARLET_VIDEO_H264_SPS_FLAG_DIRECT_8X8_INFERENCE: u32 = 1 << 6;
/// H.264 SPS has frame cropping offsets.
pub const SCARLET_VIDEO_H264_SPS_FLAG_FRAME_CROPPING: u32 = 1 << 7;
/// H.264 PPS has `entropy_coding_mode_flag` set.
pub const SCARLET_VIDEO_H264_PPS_FLAG_ENTROPY_CODING_MODE: u16 = 1 << 0;
/// H.264 PPS has `bottom_field_pic_order_in_frame_present_flag` set.
pub const SCARLET_VIDEO_H264_PPS_FLAG_BOTTOM_FIELD_PIC_ORDER_IN_FRAME_PRESENT: u16 = 1 << 1;
/// H.264 PPS has `weighted_pred_flag` set.
pub const SCARLET_VIDEO_H264_PPS_FLAG_WEIGHTED_PRED: u16 = 1 << 2;
/// H.264 PPS has `deblocking_filter_control_present_flag` set.
pub const SCARLET_VIDEO_H264_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT: u16 = 1 << 3;
/// H.264 PPS has `constrained_intra_pred_flag` set.
pub const SCARLET_VIDEO_H264_PPS_FLAG_CONSTRAINED_INTRA_PRED: u16 = 1 << 4;
/// H.264 PPS has `redundant_pic_cnt_present_flag` set.
pub const SCARLET_VIDEO_H264_PPS_FLAG_REDUNDANT_PIC_CNT_PRESENT: u16 = 1 << 5;
/// H.264 PPS has `transform_8x8_mode_flag` set.
pub const SCARLET_VIDEO_H264_PPS_FLAG_TRANSFORM_8X8_MODE: u16 = 1 << 6;
/// H.264 PPS has `pic_scaling_matrix_present_flag` set.
pub const SCARLET_VIDEO_H264_PPS_FLAG_SCALING_MATRIX_PRESENT: u16 = 1 << 7;
/// H.264 B-slice uses direct spatial motion vector prediction.
pub const SCARLET_VIDEO_H264_SLICE_FLAG_DIRECT_SPATIAL_MV_PRED: u32 = 1 << 0;
/// H.264 stateless submit includes resolved reference picture lists.
pub const SCARLET_VIDEO_H264_SLICE_FLAG_REF_LISTS_PRESENT: u32 = 1 << 1;
/// H.264 decode request is an IDR picture.
pub const SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR: u32 = 1 << 0;
/// H.264 DPB entry contains a valid reference picture.
pub const SCARLET_VIDEO_H264_DPB_FLAG_VALID: u32 = 1 << 0;
/// H.264 DPB entry is a long-term reference picture.
pub const SCARLET_VIDEO_H264_DPB_FLAG_LONG_TERM: u32 = 1 << 1;

/// VP9 loop filter deltas are enabled.
pub const SCARLET_VIDEO_VP9_LOOP_FILTER_FLAG_DELTA_ENABLED: u8 = 1 << 0;
/// VP9 loop filter deltas are updated by this frame.
pub const SCARLET_VIDEO_VP9_LOOP_FILTER_FLAG_DELTA_UPDATE: u8 = 1 << 1;
/// VP9 segmentation is enabled.
pub const SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_ENABLED: u8 = 1 << 0;
/// VP9 segmentation map is updated by this frame.
pub const SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_UPDATE_MAP: u8 = 1 << 1;
/// VP9 segmentation map uses temporal prediction.
pub const SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_TEMPORAL_UPDATE: u8 = 1 << 2;
/// VP9 segmentation feature data is updated by this frame.
pub const SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_UPDATE_DATA: u8 = 1 << 3;
/// VP9 segmentation feature data uses absolute values.
pub const SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_ABS_OR_DELTA_UPDATE: u8 = 1 << 4;
/// VP9 frame is a key frame.
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME: u32 = 1 << 0;
/// VP9 frame should be shown.
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_SHOW_FRAME: u32 = 1 << 1;
/// VP9 frame uses error resilient mode.
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_ERROR_RESILIENT: u32 = 1 << 2;
/// VP9 frame is intra-only.
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_INTRA_ONLY: u32 = 1 << 3;
/// VP9 frame allows high precision motion vectors.
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_ALLOW_HIGH_PREC_MV: u32 = 1 << 4;
/// VP9 frame refreshes the selected frame context.
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_REFRESH_FRAME_CTX: u32 = 1 << 5;
/// VP9 frame uses parallel decode mode.
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_PARALLEL_DEC_MODE: u32 = 1 << 6;
/// VP9 frame uses horizontal chroma subsampling.
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_X_SUBSAMPLING: u32 = 1 << 7;
/// VP9 frame uses vertical chroma subsampling.
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_Y_SUBSAMPLING: u32 = 1 << 8;
/// VP9 stream uses full-swing color range.
pub const SCARLET_VIDEO_VP9_FRAME_FLAG_COLOR_RANGE_FULL_SWING: u32 = 1 << 9;
/// VP9 last reference sign bias bit.
pub const SCARLET_VIDEO_VP9_SIGN_BIAS_LAST: u8 = 1 << 0;
/// VP9 golden reference sign bias bit.
pub const SCARLET_VIDEO_VP9_SIGN_BIAS_GOLDEN: u8 = 1 << 1;
/// VP9 alternate reference sign bias bit.
pub const SCARLET_VIDEO_VP9_SIGN_BIAS_ALT: u8 = 1 << 2;
/// No VP9 frame context reset.
pub const SCARLET_VIDEO_VP9_RESET_FRAME_CTX_NONE: u8 = 0;
/// Reset the selected VP9 frame context.
pub const SCARLET_VIDEO_VP9_RESET_FRAME_CTX_SPEC: u8 = 1;
/// Reset all VP9 frame contexts.
pub const SCARLET_VIDEO_VP9_RESET_FRAME_CTX_ALL: u8 = 2;
/// VP9 eighttap interpolation filter.
pub const SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP: u8 = 0;
/// VP9 smooth eighttap interpolation filter.
pub const SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP_SMOOTH: u8 = 1;
/// VP9 sharp eighttap interpolation filter.
pub const SCARLET_VIDEO_VP9_INTERP_FILTER_EIGHTTAP_SHARP: u8 = 2;
/// VP9 bilinear interpolation filter.
pub const SCARLET_VIDEO_VP9_INTERP_FILTER_BILINEAR: u8 = 3;
/// VP9 switchable interpolation filter.
pub const SCARLET_VIDEO_VP9_INTERP_FILTER_SWITCHABLE: u8 = 4;
/// VP9 single-reference prediction mode.
pub const SCARLET_VIDEO_VP9_REFERENCE_MODE_SINGLE_REFERENCE: u8 = 0;
/// VP9 compound-reference prediction mode.
pub const SCARLET_VIDEO_VP9_REFERENCE_MODE_COMPOUND_REFERENCE: u8 = 1;
/// VP9 selectable reference prediction mode.
pub const SCARLET_VIDEO_VP9_REFERENCE_MODE_SELECT: u8 = 2;
/// VP9 transform mode only 4x4.
pub const SCARLET_VIDEO_VP9_TX_MODE_ONLY_4X4: u8 = 0;
/// VP9 transform mode allows 8x8.
pub const SCARLET_VIDEO_VP9_TX_MODE_ALLOW_8X8: u8 = 1;
/// VP9 transform mode allows 16x16.
pub const SCARLET_VIDEO_VP9_TX_MODE_ALLOW_16X16: u8 = 2;
/// VP9 transform mode allows 32x32.
pub const SCARLET_VIDEO_VP9_TX_MODE_ALLOW_32X32: u8 = 3;
/// VP9 transform mode is selected per block.
pub const SCARLET_VIDEO_VP9_TX_MODE_SELECT: u8 = 4;
/// VP9 probability table bytes in Scarlet's current canonical packed layout.
pub const SCARLET_VIDEO_VP9_PROBABILITY_BYTES: usize = 0x774;
/// Maximum VP9 tiles described in one stateless request.
pub const SCARLET_VIDEO_VP9_MAX_TILES: usize = 256;

/// Scarlet coded format value for H.264.
pub const SCARLET_VIDEO_FORMAT_H264: u32 = 4098;
/// Scarlet coded format value for HEVC/H.265.
pub const SCARLET_VIDEO_FORMAT_HEVC: u32 = 4099;
/// Scarlet coded format value for VP9.
pub const SCARLET_VIDEO_FORMAT_VP9: u32 = 4102;
/// Scarlet coded format value for AV1.
pub const SCARLET_VIDEO_FORMAT_AV1: u32 = 4103;

/// Capabilities advertised by a video decode backend.
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoBackendCapabilities {
    /// Maximum number of simultaneously owned decode sessions.
    pub max_sessions: u32,
    /// Maximum number of decode requests the backend can own concurrently.
    pub max_inflight_decodes: u32,
    /// Maximum byte length accepted in the mapped input area.
    pub mapped_input_len: u32,
    /// Maximum byte length produced in the mapped output area.
    pub mapped_output_len: u32,
    /// Pixel format produced by decoded frames.
    pub output_pixel_format: u32,
    /// Whether stateful H.264 access units are accepted.
    pub supports_h264: bool,
    /// Whether stateful AV1 access units are accepted.
    pub supports_av1: bool,
    /// Whether stateful HEVC access units are accepted.
    pub supports_hevc: bool,
    /// Whether stateless H.264 requests are accepted.
    pub supports_stateless_h264: bool,
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
            SCARLET_VIDEO_FORMAT_HEVC => self.supports_hevc,
            SCARLET_VIDEO_FORMAT_AV1 => self.supports_av1,
            _ => false,
        }
    }

    /// Convert backend capabilities to user-visible bit flags.
    ///
    /// # Returns
    ///
    /// `SCARLET_VIDEO_CAP_*` bitset.
    pub fn user_flags(&self) -> u32 {
        let mut flags = 0;
        if self.supports_h264 {
            flags |= SCARLET_VIDEO_CAP_STATEFUL_H264;
        }
        if self.supports_av1 {
            flags |= SCARLET_VIDEO_CAP_STATEFUL_AV1;
        }
        if self.supports_hevc {
            flags |= SCARLET_VIDEO_CAP_STATEFUL_HEVC;
        }
        if self.supports_stateless_h264 {
            flags |= SCARLET_VIDEO_CAP_STATELESS_H264;
        }
        if self.mapped_input_len != 0 && self.mapped_output_len != 0 {
            flags |= SCARLET_VIDEO_CAP_MAPPED_BUFFERS;
        }
        if self.max_sessions > 1 {
            flags |= SCARLET_VIDEO_CAP_SESSIONS;
        }
        flags
    }
}

/// Backend decode request for a mapped access unit.
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoBackendDecodeRequest {
    /// Backend stream/session identifier.
    pub stream_id: u32,
    /// Scarlet coded stream format.
    pub coded_format: u32,
    /// Physical address backing the coded input bytes.
    pub input_paddr: usize,
    /// Kernel virtual address of the coded input bytes.
    pub input_vaddr: usize,
    /// Byte length of the coded input.
    pub input_len: u32,
    /// Physical address backing the output frame buffer.
    pub output_paddr: usize,
    /// Kernel virtual address of the output frame buffer.
    pub output_vaddr: usize,
    /// Offset of the output frame buffer inside the frontend mmap.
    pub output_offset: u64,
    /// Byte capacity of the output frame buffer.
    pub output_len: u32,
    /// Presentation timestamp carried through dequeue.
    pub timestamp: u64,
}

/// Backend request for stateless H.264 decode.
#[derive(Clone, Copy, Debug)]
pub struct VideoBackendH264StatelessRequest {
    /// Common mapped decode buffers.
    pub decode: VideoBackendDecodeRequest,
    /// Copied stateless H.264 parameters supplied by userspace.
    pub h264: ScarletVideoH264StatelessParams,
}

/// Backend request for stateless VP9 decode.
#[derive(Clone, Copy, Debug)]
pub struct VideoBackendVp9StatelessRequest {
    /// Common mapped decode buffers.
    pub decode: VideoBackendDecodeRequest,
    /// Copied stateless VP9 parameters supplied by userspace.
    pub vp9: ScarletVideoVp9StatelessParams,
}

/// Decoded frame returned by a backend.
#[derive(Clone, Copy, Debug, Default)]
pub struct VideoBackendDecodedFrame {
    /// Backend stream/session identifier.
    pub stream_id: u32,
    /// User-visible decoded frame metadata.
    pub frame: ScarletVideoDequeuedFrame,
}

/// Receiver for backend decode-completion notifications.
pub trait VideoCompletionNotifier: Send + Sync {
    /// Notify that backend decode-completion state may have changed.
    ///
    /// Backends call this after an interrupt, poll, or error path observes
    /// completion progress. The notifier should re-check backend state because
    /// notifications are edge hints and can be coalesced.
    fn notify_video_completion(&self);
}

/// Common interface for Scarlet video decode backends.
pub trait VideoDecodeBackend: Send + Sync {
    /// Return a short backend name for diagnostics.
    ///
    /// # Returns
    ///
    /// Static backend name.
    fn name(&self) -> &'static str;

    /// Return backend-specific debug status for `/dev/videoN` reads.
    ///
    /// # Returns
    ///
    /// A short status fragment without a trailing newline when the backend has
    /// useful diagnostics, or `None` when the generic status is sufficient.
    fn debug_status(&self) -> Option<String> {
        None
    }

    /// Return backend capabilities.
    ///
    /// # Returns
    ///
    /// Capabilities used by `/dev/video*` frontends.
    fn capabilities(&self) -> VideoBackendCapabilities;

    /// Return whether the backend accepts stateless VP9 requests.
    ///
    /// # Returns
    ///
    /// `true` when `SCARLET_VIDEO_SUBMIT_VP9_STATELESS` is supported.
    fn supports_stateless_vp9(&self) -> bool {
        false
    }

    /// Install or remove the frontend completion notifier.
    ///
    /// # Arguments
    ///
    /// * `notifier` - Weak reference to the frontend scheduler that should be
    ///   woken when backend completion state changes, or `None` to disconnect
    ///   the notifier.
    ///
    /// # Returns
    ///
    /// This hook is best-effort and returns no status. Backends that do not
    /// generate asynchronous completion notifications can ignore it.
    fn set_completion_notifier(&self, _notifier: Option<Weak<dyn VideoCompletionNotifier>>) {}

    /// Create a decode session.
    ///
    /// # Arguments
    ///
    /// * `coded_format` - Scarlet coded format requested by userspace.
    ///
    /// # Returns
    ///
    /// Backend stream/session identifier for a newly allocated session. The
    /// backend must return an error when no free session is available.
    fn create_session(&self, coded_format: u32) -> Result<u32, &'static str>;

    /// Destroy a decode session.
    ///
    /// # Arguments
    ///
    /// * `stream_id` - Backend stream/session identifier.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the session is released. After a successful return, the
    /// backend must not access buffers that were submitted by that session.
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

    /// Submit one stateless H.264 decode request.
    ///
    /// # Arguments
    ///
    /// * `request` - Mapped buffers and H.264 syntax parameters for one decode
    ///   request.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the backend accepted the request.
    fn submit_h264_stateless(
        &self,
        _request: &VideoBackendH264StatelessRequest,
    ) -> Result<(), &'static str> {
        Err("scarlet-video: backend does not support stateless H.264")
    }

    /// Submit one stateless VP9 decode request.
    ///
    /// # Arguments
    ///
    /// * `request` - Mapped buffers and VP9 syntax parameters for one decode
    ///   request.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the backend accepted the request.
    fn submit_vp9_stateless(
        &self,
        _request: &VideoBackendVp9StatelessRequest,
    ) -> Result<(), &'static str> {
        Err("scarlet-video: backend does not support stateless VP9")
    }

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

static VIDEO_BACKENDS: IrqSpinLock<Vec<Arc<dyn VideoDecodeBackend>>> = IrqSpinLock::new(Vec::new());
static VIDEO_DEVICE_COUNTER: AtomicUsize = AtomicUsize::new(0);
const DEFAULT_STREAM_ID: u32 = 1;
const VIDEO_MAPPED_BUFFER_ALIGN: usize = 0x4000;
const VIDEO_MAX_INFLIGHT_DECODES: usize = 32;
const VIDEO_MAX_QUEUED_JOBS: usize = 16;
const VIDEO_MAX_COMPLETED_FRAMES: usize = 16;
const VIDEO_MAX_STREAM_ERRORS: usize = 16;

#[derive(Clone, Copy)]
enum VideoQueuedJob {
    Decode(VideoBackendDecodeRequest),
    H264Stateless(VideoBackendH264StatelessRequest),
    Vp9Stateless(VideoBackendVp9StatelessRequest),
}

impl VideoQueuedJob {
    fn stream_id(&self) -> u32 {
        match self {
            Self::Decode(request) => request.stream_id,
            Self::H264Stateless(request) => request.decode.stream_id,
            Self::Vp9Stateless(request) => request.decode.stream_id,
        }
    }
}

#[derive(Clone, Copy)]
struct VideoStreamError {
    stream_id: u32,
    error: &'static str,
}

struct VideoSchedulerState {
    current_streams: Vec<u32>,
    queued: VecDeque<VideoQueuedJob>,
    completed: VecDeque<VideoBackendDecodedFrame>,
    errors: VecDeque<VideoStreamError>,
}

impl VideoSchedulerState {
    fn new(max_inflight_decodes: usize) -> Self {
        Self {
            current_streams: Vec::with_capacity(max_inflight_decodes),
            queued: VecDeque::with_capacity(VIDEO_MAX_QUEUED_JOBS),
            completed: VecDeque::with_capacity(VIDEO_MAX_COMPLETED_FRAMES),
            errors: VecDeque::with_capacity(VIDEO_MAX_STREAM_ERRORS),
        }
    }

    fn has_pending_stream(&self, stream_id: u32) -> bool {
        self.current_streams.contains(&stream_id)
            || self.queued.iter().any(|job| job.stream_id() == stream_id)
            || self
                .completed
                .iter()
                .any(|frame| frame.stream_id == stream_id)
    }

    fn push_completed(&mut self, frame: VideoBackendDecodedFrame) {
        if self.completed.len() >= VIDEO_MAX_COMPLETED_FRAMES {
            let _ = self.completed.pop_front();
        }
        self.completed.push_back(frame);
    }

    fn push_error(&mut self, stream_id: u32, error: &'static str) {
        if self.errors.len() >= VIDEO_MAX_STREAM_ERRORS {
            let _ = self.errors.pop_front();
        }
        self.errors.push_back(VideoStreamError { stream_id, error });
    }

    fn remove_completed(&mut self, stream_id: u32) -> Option<VideoBackendDecodedFrame> {
        let index = self
            .completed
            .iter()
            .position(|frame| frame.stream_id == stream_id)?;
        self.completed.remove(index)
    }

    fn remove_error(&mut self, stream_id: u32) -> Option<&'static str> {
        let index = self
            .errors
            .iter()
            .position(|error| error.stream_id == stream_id)?;
        self.errors.remove(index).map(|error| error.error)
    }

    fn remove_stream_queues(&mut self, stream_id: u32) {
        self.queued.retain(|job| job.stream_id() != stream_id);
        self.completed.retain(|frame| frame.stream_id != stream_id);
        self.errors.retain(|error| error.stream_id != stream_id);
    }

    fn clear_current_stream(&mut self, stream_id: u32) {
        if let Some(index) = self
            .current_streams
            .iter()
            .position(|current| *current == stream_id)
        {
            self.current_streams.swap_remove(index);
        }
    }
}

struct VideoSchedulerLifecycle {
    destroyed_streams: Vec<u32>,
}

impl VideoSchedulerLifecycle {
    fn new() -> Self {
        Self {
            destroyed_streams: Vec::new(),
        }
    }

    fn is_destroyed_stream(&self, stream_id: u32) -> bool {
        self.destroyed_streams.contains(&stream_id)
    }

    fn mark_stream_active(&mut self, stream_id: u32) {
        self.destroyed_streams
            .retain(|destroyed| *destroyed != stream_id);
    }

    fn mark_stream_destroyed(&mut self, stream_id: u32) {
        if !self.is_destroyed_stream(stream_id) {
            self.destroyed_streams.push(stream_id);
        }
    }
}

fn max_inflight_from_capabilities(caps: VideoBackendCapabilities) -> usize {
    let requested = if caps.max_inflight_decodes == 0 {
        1
    } else {
        caps.max_inflight_decodes as usize
    };
    let session_limited = if caps.max_sessions == 0 {
        requested
    } else {
        requested.min(caps.max_sessions as usize)
    };
    session_limited.clamp(1, VIDEO_MAX_INFLIGHT_DECODES)
}

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

/// Register a Scarlet video decode frontend for a backend.
///
/// # Arguments
///
/// * `backend` - Backend implementation served through `/dev/videoN`.
///
/// # Returns
///
/// Registered device node name.
pub fn register_video_decode_device(backend: Arc<dyn VideoDecodeBackend>) -> String {
    let id = VIDEO_DEVICE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let name = format!("video{}", id);
    let device = Arc::new(ScarletVideoDevice::new(Arc::clone(&backend)));
    let notifier: Arc<dyn VideoCompletionNotifier> = device.clone();
    backend.set_completion_notifier(Some(Arc::downgrade(&notifier)));
    let device: Arc<dyn Device> = device;
    DeviceManager::get_manager().register_device_with_name(name.clone(), device);
    name
}

struct ScarletVideoDevice {
    backend: Arc<dyn VideoDecodeBackend>,
    /// Guards only the short scheduler lifecycle state transitions.
    ///
    /// Do not use a task-owned sleeping mutex here. A userspace task may be
    /// retired while an ioctl is in flight; retaining such a mutex would then
    /// strand close/reopen forever. The IRQ spin guard prevents retirement
    /// during these bounded, non-sleeping updates and is always released
    /// before calling a backend.
    scheduler_lifecycle: IrqSpinLock<VideoSchedulerLifecycle>,
    scheduler: IrqSpinLock<VideoSchedulerState>,
    completion_waker: Waker,
    max_inflight_decodes: usize,
    mapped_buffer: IrqSpinLock<Option<ContiguousPages>>,
    last_error: IrqSpinLock<Option<&'static str>>,
    next_timestamp: IrqSpinLock<u64>,
}

impl ScarletVideoDevice {
    fn new(backend: Arc<dyn VideoDecodeBackend>) -> Self {
        let max_inflight_decodes = max_inflight_from_capabilities(backend.capabilities());
        Self {
            backend,
            scheduler_lifecycle: IrqSpinLock::new(VideoSchedulerLifecycle::new()),
            scheduler: IrqSpinLock::new(VideoSchedulerState::new(max_inflight_decodes)),
            completion_waker: Waker::new_interruptible("scarlet_video"),
            max_inflight_decodes,
            mapped_buffer: IrqSpinLock::new(None),
            last_error: IrqSpinLock::new(None),
            next_timestamp: IrqSpinLock::new(1),
        }
    }

    fn buffer_layout(&self) -> Result<VideoBufferLayout, &'static str> {
        let caps = self.backend.capabilities();
        if caps.mapped_input_len == 0 || caps.mapped_output_len == 0 {
            return Err("scarlet-video: backend does not support mapped buffers");
        }
        let input_len = caps.mapped_input_len as usize;
        let output_len = caps.mapped_output_len as usize;
        let output_offset = align_up(input_len, VIDEO_MAPPED_BUFFER_ALIGN);
        let mmap_len = output_offset
            .checked_add(output_len)
            .ok_or("scarlet-video: mapped buffer length overflow")?;
        Ok(VideoBufferLayout {
            input_len,
            output_offset,
            output_len,
            mmap_len: align_up(mmap_len, PAGE_SIZE),
        })
    }

    fn buffer_info(&self) -> Result<ScarletVideoBufferInfo, &'static str> {
        let layout = self.buffer_layout()?;
        self.ensure_mapped_buffer(layout)?;
        Ok(ScarletVideoBufferInfo {
            mmap_offset: 0,
            mmap_len: layout.mmap_len as u64,
            input_offset: 0,
            input_len: layout.input_len as u32,
            output_offset: layout.output_offset as u64,
            output_len: layout.output_len as u32,
        })
    }

    fn capabilities_info(&self) -> ScarletVideoCapabilities {
        let caps = self.backend.capabilities();
        let mut flags = caps.user_flags();
        if self.backend.supports_stateless_vp9() {
            flags |= SCARLET_VIDEO_CAP_STATELESS_VP9;
        }
        ScarletVideoCapabilities {
            version: SCARLET_VIDEO_CAPS_VERSION,
            flags,
            max_sessions: caps.max_sessions,
            output_pixel_format: caps.output_pixel_format,
            mapped_input_len: caps.mapped_input_len,
            mapped_output_len: caps.mapped_output_len,
            reserved: [0; 8],
        }
    }

    fn log_backend_state(&self, event: &str, stream_id: u32, error: Option<&'static str>) {
        let caps = self.backend.capabilities();
        let flags = caps.user_flags()
            | if self.backend.supports_stateless_vp9() {
                SCARLET_VIDEO_CAP_STATELESS_VP9
            } else {
                0
            };
        let backend_status = self.backend.debug_status().unwrap_or_default();
        if let Some(error) = error {
            crate::println!(
                "[scarlet-video] {} error={} stream={} backend={} caps=0x{:x} sessions={} inflight={} input={} output={}{}",
                event,
                error,
                stream_id,
                self.backend.name(),
                flags,
                caps.max_sessions,
                caps.max_inflight_decodes,
                caps.mapped_input_len,
                caps.mapped_output_len,
                backend_status
            );
        } else {
            crate::println!(
                "[scarlet-video] {} stream={} backend={} caps=0x{:x} sessions={} inflight={} input={} output={}{}",
                event,
                stream_id,
                self.backend.name(),
                flags,
                caps.max_sessions,
                caps.max_inflight_decodes,
                caps.mapped_input_len,
                caps.mapped_output_len,
                backend_status
            );
        }
    }

    fn ensure_mapped_buffer(&self, layout: VideoBufferLayout) -> Result<(), &'static str> {
        let mut mapped_buffer = self.mapped_buffer.lock();
        if mapped_buffer.is_none() {
            let pages = layout.mmap_len.div_ceil(PAGE_SIZE);
            *mapped_buffer = ContiguousPages::new_aligned(pages, VIDEO_MAPPED_BUFFER_ALIGN);
            if let Some(buffer) = mapped_buffer.as_ref() {
                crate::println!(
                    "[scarlet-video] legacy mmap alloc pages={} len={} paddr={:#x}",
                    pages,
                    layout.mmap_len,
                    buffer.as_paddr()
                );
            } else {
                crate::println!(
                    "[scarlet-video] legacy mmap alloc failed pages={} len={}",
                    pages,
                    layout.mmap_len
                );
            }
        }
        if mapped_buffer.is_some() {
            Ok(())
        } else {
            Err("scarlet-video: mmap buffer allocation failed")
        }
    }

    fn next_timestamp(&self) -> u64 {
        let mut next = self.next_timestamp.lock();
        let timestamp = *next;
        *next = next.wrapping_add(1);
        timestamp
    }

    fn create_scheduled_session(&self, coded_format: u32) -> Result<u32, &'static str> {
        let stream_id = self.backend.create_session(coded_format)?;
        self.scheduler_lifecycle
            .lock()
            .mark_stream_active(stream_id);
        Ok(stream_id)
    }

    fn enqueue_decode_job(&self, job: VideoQueuedJob) -> Result<(), &'static str> {
        let stream_id = job.stream_id();
        {
            // Keep the lifecycle check and queue insertion atomic with
            // respect to destroy. Both locks are spin-only and the lock order
            // is shared with `destroy_scheduled_stream`.
            let lifecycle = self.scheduler_lifecycle.lock();
            if lifecycle.is_destroyed_stream(stream_id) {
                return Err("scarlet-video: video session is destroyed");
            }
            let mut scheduler = self.scheduler.lock();
            if scheduler.has_pending_stream(stream_id) {
                return Err("scarlet-video: stream decode already pending");
            }
            if scheduler.queued.len() >= VIDEO_MAX_QUEUED_JOBS {
                return Err("scarlet-video: decode queue is full");
            }
            scheduler.queued.push_back(job);
        }

        self.pump_scheduler();
        self.completion_waker.wake_all();
        Ok(())
    }

    fn pump_scheduler(&self) {
        loop {
            let mut made_progress = false;
            let mut current_index = 0;
            while let Some(stream_id) = self.current_scheduled_stream(current_index) {
                match self.backend.dequeue_frame(stream_id) {
                    Ok(Some(frame)) => {
                        {
                            let _irq_guard = IrqGuard::new();
                            let mut scheduler = self.scheduler.lock();
                            scheduler.clear_current_stream(stream_id);
                            scheduler.push_completed(frame);
                        }
                        self.completion_waker.wake_all();
                        made_progress = true;
                        continue;
                    }
                    Ok(None) => current_index += 1,
                    Err(error) => {
                        {
                            let _irq_guard = IrqGuard::new();
                            let mut scheduler = self.scheduler.lock();
                            scheduler.clear_current_stream(stream_id);
                            scheduler.push_error(stream_id, error);
                        }
                        self.completion_waker.wake_all();
                        made_progress = true;
                        continue;
                    }
                }
            }

            let mut dispatched = false;
            while let Some(job) = self.take_next_job() {
                let stream_id = job.stream_id();
                let result = match job {
                    VideoQueuedJob::Decode(request) => self.backend.submit_decode(&request),
                    VideoQueuedJob::H264Stateless(request) => {
                        self.backend.submit_h264_stateless(&request)
                    }
                    VideoQueuedJob::Vp9Stateless(request) => {
                        self.backend.submit_vp9_stateless(&request)
                    }
                };
                match result {
                    Ok(()) => {
                        self.completion_waker.wake_all();
                        dispatched = true;
                    }
                    Err(error) => {
                        {
                            let _irq_guard = IrqGuard::new();
                            let mut scheduler = self.scheduler.lock();
                            scheduler.clear_current_stream(stream_id);
                            scheduler.push_error(stream_id, error);
                        }
                        self.completion_waker.wake_all();
                        made_progress = true;
                    }
                }
            }

            if !made_progress && !dispatched {
                return;
            }
        }
    }

    fn current_scheduled_stream(&self, index: usize) -> Option<u32> {
        let _irq_guard = IrqGuard::new();
        self.scheduler.lock().current_streams.get(index).copied()
    }

    fn take_next_job(&self) -> Option<VideoQueuedJob> {
        let _irq_guard = IrqGuard::new();
        let mut scheduler = self.scheduler.lock();
        if scheduler.current_streams.len() >= self.max_inflight_decodes {
            return None;
        }
        let job = scheduler.queued.pop_front()?;
        scheduler.current_streams.push(job.stream_id());
        Some(job)
    }

    fn dequeue_scheduled_frame(
        &self,
        stream_id: u32,
    ) -> Result<Option<VideoBackendDecodedFrame>, &'static str> {
        self.pump_scheduler();
        let _irq_guard = IrqGuard::new();
        let mut scheduler = self.scheduler.lock();
        if let Some(error) = scheduler.remove_error(stream_id) {
            return Err(error);
        }
        Ok(scheduler.remove_completed(stream_id))
    }

    fn destroy_scheduled_stream(&self, stream_id: u32) -> Result<(), &'static str> {
        {
            // Publish destruction and remove every queued result as one
            // bounded transition. No sleeping/backend work is allowed while
            // either spin lock is held.
            let mut lifecycle = self.scheduler_lifecycle.lock();
            lifecycle.mark_stream_destroyed(stream_id);
            let mut scheduler = self.scheduler.lock();
            scheduler.remove_stream_queues(stream_id);
            scheduler.clear_current_stream(stream_id);
        }
        let result = self.backend.destroy_session(stream_id);
        {
            // A scheduler pump racing the backend teardown may have observed
            // the old active session. Sweep once more after teardown so no
            // stale completion/error survives into a later open.
            let mut scheduler = self.scheduler.lock();
            scheduler.remove_stream_queues(stream_id);
            scheduler.clear_current_stream(stream_id);
        }
        result?;
        self.pump_scheduler();
        self.completion_waker.wake_all();
        Ok(())
    }

    fn scheduler_ready(&self, interest: ReadyInterest) -> ReadySet {
        self.pump_scheduler();
        let _irq_guard = IrqGuard::new();
        let scheduler = self.scheduler.lock();
        let mut set = ReadySet::none();
        if interest.read && (!scheduler.completed.is_empty() || !scheduler.errors.is_empty()) {
            set.read = true;
        }
        if interest.write && scheduler.queued.len() < VIDEO_MAX_QUEUED_JOBS {
            set.write = true;
        }
        set
    }

    fn mapped_decode_request(
        &self,
        stream_id: u32,
        coded_format: u32,
        input_len: usize,
        timestamp: u64,
    ) -> Result<VideoBackendDecodeRequest, &'static str> {
        let layout = self.buffer_layout()?;
        if input_len == 0 {
            return Err("scarlet-video: input is empty");
        }
        if input_len > layout.input_len {
            return Err("scarlet-video: input exceeds mapped buffer");
        }

        self.ensure_mapped_buffer(layout)?;
        let (input_paddr, input_vaddr, output_paddr, output_vaddr) = {
            let mapped_buffer = self.mapped_buffer.lock();
            let buffer = mapped_buffer
                .as_ref()
                .ok_or("scarlet-video: mmap buffer missing")?;
            (
                buffer.as_paddr(),
                buffer.as_vaddr(),
                buffer.as_paddr() + layout.output_offset,
                buffer.as_vaddr() + layout.output_offset,
            )
        };
        let timestamp = if timestamp == 0 {
            self.next_timestamp()
        } else {
            timestamp
        };
        Ok(VideoBackendDecodeRequest {
            stream_id,
            coded_format,
            input_paddr,
            input_vaddr,
            input_len: input_len as u32,
            output_paddr,
            output_vaddr,
            output_offset: layout.output_offset as u64,
            output_len: layout.output_len as u32,
            timestamp,
        })
    }

    fn submit_mapped(
        &self,
        stream_id: u32,
        coded_format: u32,
        input_len: usize,
        timestamp: u64,
    ) -> Result<(), &'static str> {
        if !self.backend.capabilities().supports_format(coded_format) {
            return Err("scarlet-video: backend does not support coded format");
        }
        let request = self.mapped_decode_request(stream_id, coded_format, input_len, timestamp)?;
        self.enqueue_decode_job(VideoQueuedJob::Decode(request))
    }

    fn h264_stateless_params(
        &self,
        ptrs: ScarletVideoH264ParamPtrs,
    ) -> Result<ScarletVideoH264StatelessParams, &'static str> {
        if ptrs.sps == 0
            || ptrs.pps == 0
            || ptrs.scaling_matrix == 0
            || ptrs.pred_weights == 0
            || ptrs.slice_params == 0
            || ptrs.decode_params == 0
        {
            return Err("scarlet-video: stateless H.264 parameter pointer is null");
        }

        Ok(ScarletVideoH264StatelessParams {
            sps: read_user_value(ptrs.sps as usize)?,
            pps: read_user_value(ptrs.pps as usize)?,
            scaling_matrix: read_user_value(ptrs.scaling_matrix as usize)?,
            pred_weights: read_user_value(ptrs.pred_weights as usize)?,
            slice_params: read_user_value(ptrs.slice_params as usize)?,
            decode_params: read_user_value(ptrs.decode_params as usize)?,
        })
    }

    fn vp9_stateless_params(
        &self,
        ptrs: ScarletVideoVp9ParamPtrs,
    ) -> Result<ScarletVideoVp9StatelessParams, &'static str> {
        if ptrs.frame == 0 || ptrs.probabilities == 0 || ptrs.tiles == 0 {
            return Err("scarlet-video: stateless VP9 parameter pointer is null");
        }

        Ok(ScarletVideoVp9StatelessParams {
            frame: read_user_value(ptrs.frame as usize)?,
            probabilities: read_user_value(ptrs.probabilities as usize)?,
            tiles: read_user_value(ptrs.tiles as usize)?,
        })
    }

    fn handle_get_buffer(&self, arg: usize) -> Result<i32, &'static str> {
        let info = self.buffer_info()?;
        write_user_value(arg, &info)?;
        Ok(0)
    }

    fn handle_get_caps(&self, arg: usize) -> Result<i32, &'static str> {
        let caps = self.capabilities_info();
        write_user_value(arg, &caps)?;
        Ok(0)
    }

    fn handle_create_session(&self, arg: usize) -> Result<i32, &'static str> {
        let mut info: ScarletVideoSessionInfo = read_user_value(arg)?;
        let coded_format = if info.padding == 0 {
            SCARLET_VIDEO_FORMAT_H264
        } else {
            info.padding
        };
        let stream_id = if info.stream_id == 0 {
            self.create_scheduled_session(coded_format)?
        } else {
            info.stream_id
        };
        info.stream_id = stream_id;
        info.padding = 0;
        info.buffer = self.buffer_info()?;
        write_user_value(arg, &info)?;
        Ok(0)
    }

    fn handle_destroy_session(&self, arg: usize) -> Result<i32, &'static str> {
        let info: ScarletVideoSessionInfo = read_user_value(arg)?;
        self.destroy_scheduled_stream(info.stream_id)?;
        *self.next_timestamp.lock() = 1;
        Ok(0)
    }

    fn handle_submit(&self, arg: usize) -> Result<i32, &'static str> {
        let submit: ScarletVideoSubmit = read_user_value(arg)?;
        match self.submit_mapped(
            DEFAULT_STREAM_ID,
            submit.coded_format,
            submit.input_len as usize,
            submit.timestamp,
        ) {
            Ok(()) => {
                *self.last_error.lock() = None;
                Ok(0)
            }
            Err(e) => {
                *self.last_error.lock() = Some(e);
                Err(e)
            }
        }
    }

    fn handle_submit_session(&self, arg: usize) -> Result<i32, &'static str> {
        let submit: ScarletVideoSessionSubmit = read_user_value(arg)?;
        match self.submit_mapped(
            submit.stream_id,
            submit.coded_format,
            submit.input_len as usize,
            submit.timestamp,
        ) {
            Ok(()) => {
                *self.last_error.lock() = None;
                Ok(0)
            }
            Err(e) => {
                *self.last_error.lock() = Some(e);
                Err(e)
            }
        }
    }

    fn handle_submit_h264_stateless(&self, arg: usize) -> Result<i32, &'static str> {
        let submit: ScarletVideoH264StatelessSubmit = read_user_value(arg)?;
        let result = (|| {
            if submit.flags != 0 {
                return Err("scarlet-video: stateless H.264 submit flags must be zero");
            }
            if !self.backend.capabilities().supports_stateless_h264 {
                return Err("scarlet-video: backend does not support stateless H.264");
            }
            let stream_id = if submit.stream_id == 0 {
                DEFAULT_STREAM_ID
            } else {
                submit.stream_id
            };
            let decode = self.mapped_decode_request(
                stream_id,
                SCARLET_VIDEO_FORMAT_H264,
                submit.input_len as usize,
                submit.timestamp,
            )?;
            let h264 = self.h264_stateless_params(submit.params)?;
            let request = VideoBackendH264StatelessRequest { decode, h264 };
            self.enqueue_decode_job(VideoQueuedJob::H264Stateless(request))
        })();

        match result {
            Ok(()) => {
                *self.last_error.lock() = None;
                Ok(0)
            }
            Err(e) => {
                *self.last_error.lock() = Some(e);
                Err(e)
            }
        }
    }

    fn handle_submit_vp9_stateless(&self, arg: usize) -> Result<i32, &'static str> {
        let submit: ScarletVideoVp9StatelessSubmit = read_user_value(arg)?;
        let result = (|| {
            if submit.flags != 0 {
                return Err("scarlet-video: stateless VP9 submit flags must be zero");
            }
            if !self.backend.supports_stateless_vp9() {
                return Err("scarlet-video: backend does not support stateless VP9");
            }
            let stream_id = if submit.stream_id == 0 {
                DEFAULT_STREAM_ID
            } else {
                submit.stream_id
            };
            let decode = self.mapped_decode_request(
                stream_id,
                SCARLET_VIDEO_FORMAT_VP9,
                submit.input_len as usize,
                submit.timestamp,
            )?;
            let vp9 = self.vp9_stateless_params(submit.params)?;
            let request = VideoBackendVp9StatelessRequest { decode, vp9 };
            self.enqueue_decode_job(VideoQueuedJob::Vp9Stateless(request))
        })();

        match result {
            Ok(()) => {
                *self.last_error.lock() = None;
                Ok(0)
            }
            Err(e) => {
                *self.last_error.lock() = Some(e);
                Err(e)
            }
        }
    }

    fn handle_dequeue(&self, arg: usize) -> Result<i32, &'static str> {
        let decoded = match self.dequeue_scheduled_frame(DEFAULT_STREAM_ID) {
            Ok(decoded) => decoded,
            Err(e) => {
                *self.last_error.lock() = Some(e);
                return Err(e);
            }
        };
        let Some(decoded) = decoded else {
            *self.last_error.lock() = None;
            return Ok(0);
        };
        write_user_value(arg, &decoded.frame)?;
        *self.last_error.lock() = None;
        Ok(1)
    }

    fn handle_dequeue_session(&self, arg: usize) -> Result<i32, &'static str> {
        let mut dequeued: ScarletVideoSessionDequeuedFrame = read_user_value(arg)?;
        let stream_id = if dequeued.stream_id == 0 {
            DEFAULT_STREAM_ID
        } else {
            dequeued.stream_id
        };
        let decoded = match self.dequeue_scheduled_frame(stream_id) {
            Ok(decoded) => decoded,
            Err(e) => {
                *self.last_error.lock() = Some(e);
                return Err(e);
            }
        };
        let Some(decoded) = decoded else {
            *self.last_error.lock() = None;
            return Ok(0);
        };
        dequeued.stream_id = decoded.stream_id;
        dequeued.padding = 0;
        dequeued.frame = decoded.frame;
        write_user_value(arg, &dequeued)?;
        *self.last_error.lock() = None;
        Ok(1)
    }

    fn status_line(&self) -> String {
        let caps = self.backend.capabilities();
        let last_error = self.last_error.lock().unwrap_or("none");
        let backend_status = self.backend.debug_status().unwrap_or_default();
        format!(
            "scarlet-video backend={} stateful_h264={} stateful_av1={} stateless_h264={} stateless_vp9={} sessions={} inflight={} input={} output={} last_error={}{}\n",
            self.backend.name(),
            caps.supports_h264,
            caps.supports_av1,
            caps.supports_stateless_h264,
            self.backend.supports_stateless_vp9(),
            caps.max_sessions,
            caps.max_inflight_decodes,
            caps.mapped_input_len,
            caps.mapped_output_len,
            last_error,
            backend_status
        )
    }
}

struct ScarletVideoOpen {
    device: Arc<ScarletVideoDevice>,
    mapped_buffer: IrqSpinLock<Option<ContiguousPages>>,
    last_error: IrqSpinLock<Option<&'static str>>,
    next_timestamp: IrqSpinLock<u64>,
    stream_id: IrqSpinLock<Option<u32>>,
    coded_format: IrqSpinLock<u32>,
}

impl ScarletVideoOpen {
    fn new(device: Arc<ScarletVideoDevice>) -> Result<Self, &'static str> {
        let stream_id = match device.create_scheduled_session(SCARLET_VIDEO_FORMAT_H264) {
            Ok(stream_id) => {
                device.log_backend_state("open create_session ok", stream_id, None);
                stream_id
            }
            Err(e) => {
                device.log_backend_state("open create_session failed", 0, Some(e));
                return Err(e);
            }
        };
        Ok(Self {
            device,
            mapped_buffer: IrqSpinLock::new(None),
            last_error: IrqSpinLock::new(None),
            next_timestamp: IrqSpinLock::new(1),
            stream_id: IrqSpinLock::new(Some(stream_id)),
            coded_format: IrqSpinLock::new(SCARLET_VIDEO_FORMAT_H264),
        })
    }

    fn stream_id(&self) -> Result<u32, &'static str> {
        (*self.stream_id.lock()).ok_or("scarlet-video: video session is closed")
    }

    fn create_or_query_session(
        &self,
        requested_stream_id: u32,
        coded_format: u32,
    ) -> Result<u32, &'static str> {
        let coded_format = if coded_format == 0 {
            SCARLET_VIDEO_FORMAT_H264
        } else {
            coded_format
        };
        let existing_stream = *self.stream_id.lock();
        if let Some(current) = existing_stream {
            let current_format = *self.coded_format.lock();
            if current_format != coded_format && requested_stream_id == 0 {
                self.destroy_current_session(current)?;
            }
        }
        let mut stream_id = self.stream_id.lock();
        match *stream_id {
            Some(current) if requested_stream_id == 0 || requested_stream_id == current => {
                let current_format = *self.coded_format.lock();
                if current_format != coded_format {
                    return Err("scarlet-video: stream already exists with another coded format");
                }
                crate::println!(
                    "[scarlet-video] create_session query open={:#x} requested={} stream={} format={}",
                    self as *const _ as usize,
                    requested_stream_id,
                    current,
                    coded_format
                );
                Ok(current)
            }
            Some(_) => Err("scarlet-video: stream id belongs to another open"),
            None if requested_stream_id == 0 => {
                let new_stream_id = match self.device.create_scheduled_session(coded_format) {
                    Ok(new_stream_id) => {
                        self.device.log_backend_state(
                            "create_session reopen ok",
                            new_stream_id,
                            None,
                        );
                        new_stream_id
                    }
                    Err(e) => {
                        self.device
                            .log_backend_state("create_session reopen failed", 0, Some(e));
                        return Err(e);
                    }
                };
                *stream_id = Some(new_stream_id);
                *self.coded_format.lock() = coded_format;
                Ok(new_stream_id)
            }
            None => Err("scarlet-video: cannot claim an explicit closed stream id"),
        }
    }

    fn checked_stream_id(&self, requested_stream_id: u32) -> Result<u32, &'static str> {
        let current = self.stream_id()?;
        if requested_stream_id == 0 || requested_stream_id == current {
            Ok(current)
        } else {
            Err("scarlet-video: stream id belongs to another open")
        }
    }

    fn destroy_current_session(&self, requested_stream_id: u32) -> Result<(), &'static str> {
        let current = self.checked_stream_id(requested_stream_id)?;
        crate::println!(
            "[scarlet-video] destroy_session begin open={:#x} requested={} stream={}",
            self as *const _ as usize,
            requested_stream_id,
            current
        );
        if let Err(e) = self.device.destroy_scheduled_stream(current) {
            self.device
                .log_backend_state("destroy_session failed", current, Some(e));
            return Err(e);
        }
        self.device
            .log_backend_state("destroy_session ok", current, None);
        *self.stream_id.lock() = None;
        *self.coded_format.lock() = 0;
        *self.next_timestamp.lock() = 1;
        Ok(())
    }

    fn buffer_info(&self) -> Result<ScarletVideoBufferInfo, &'static str> {
        let layout = self.device.buffer_layout()?;
        self.ensure_mapped_buffer(layout)?;
        Ok(ScarletVideoBufferInfo {
            mmap_offset: 0,
            mmap_len: layout.mmap_len as u64,
            input_offset: 0,
            input_len: layout.input_len as u32,
            output_offset: layout.output_offset as u64,
            output_len: layout.output_len as u32,
        })
    }

    fn ensure_mapped_buffer(&self, layout: VideoBufferLayout) -> Result<(), &'static str> {
        let stream_id = (*self.stream_id.lock()).unwrap_or(0);
        let mut mapped_buffer = self.mapped_buffer.lock();
        if mapped_buffer.is_none() {
            let pages = layout.mmap_len.div_ceil(PAGE_SIZE);
            *mapped_buffer = ContiguousPages::new_aligned(pages, VIDEO_MAPPED_BUFFER_ALIGN);
            if let Some(buffer) = mapped_buffer.as_ref() {
                crate::println!(
                    "[scarlet-video] mmap alloc open={:#x} stream={} pages={} len={} paddr={:#x}",
                    self as *const _ as usize,
                    stream_id,
                    pages,
                    layout.mmap_len,
                    buffer.as_paddr()
                );
            } else {
                crate::println!(
                    "[scarlet-video] mmap alloc failed open={:#x} stream={} pages={} len={}",
                    self as *const _ as usize,
                    stream_id,
                    pages,
                    layout.mmap_len
                );
            }
        }
        if mapped_buffer.is_some() {
            Ok(())
        } else {
            Err("scarlet-video: mmap buffer allocation failed")
        }
    }

    fn next_timestamp(&self) -> u64 {
        let mut next = self.next_timestamp.lock();
        let timestamp = *next;
        *next = next.wrapping_add(1);
        timestamp
    }

    fn mapped_decode_request(
        &self,
        stream_id: u32,
        coded_format: u32,
        input_len: usize,
        timestamp: u64,
    ) -> Result<VideoBackendDecodeRequest, &'static str> {
        let layout = self.device.buffer_layout()?;
        if input_len == 0 {
            return Err("scarlet-video: input is empty");
        }
        if input_len > layout.input_len {
            return Err("scarlet-video: input exceeds mapped buffer");
        }

        self.ensure_mapped_buffer(layout)?;
        let (input_paddr, input_vaddr, output_paddr, output_vaddr) = {
            let mapped_buffer = self.mapped_buffer.lock();
            let buffer = mapped_buffer
                .as_ref()
                .ok_or("scarlet-video: mmap buffer missing")?;
            (
                buffer.as_paddr(),
                buffer.as_vaddr(),
                buffer.as_paddr() + layout.output_offset,
                buffer.as_vaddr() + layout.output_offset,
            )
        };
        let timestamp = if timestamp == 0 {
            self.next_timestamp()
        } else {
            timestamp
        };
        Ok(VideoBackendDecodeRequest {
            stream_id,
            coded_format,
            input_paddr,
            input_vaddr,
            input_len: input_len as u32,
            output_paddr,
            output_vaddr,
            output_offset: layout.output_offset as u64,
            output_len: layout.output_len as u32,
            timestamp,
        })
    }

    fn submit_mapped(
        &self,
        coded_format: u32,
        input_len: usize,
        timestamp: u64,
    ) -> Result<(), &'static str> {
        let stream_id = self.stream_id()?;
        self.submit_mapped_for_stream(stream_id, coded_format, input_len, timestamp)
    }

    fn submit_mapped_for_stream(
        &self,
        stream_id: u32,
        coded_format: u32,
        input_len: usize,
        timestamp: u64,
    ) -> Result<(), &'static str> {
        if !self
            .device
            .backend
            .capabilities()
            .supports_format(coded_format)
        {
            return Err("scarlet-video: backend does not support coded format");
        }
        let request = self.mapped_decode_request(stream_id, coded_format, input_len, timestamp)?;
        self.device
            .enqueue_decode_job(VideoQueuedJob::Decode(request))
    }

    fn dequeue_frame(
        &self,
        stream_id: u32,
    ) -> Result<Option<VideoBackendDecodedFrame>, &'static str> {
        let stream_id = self.checked_stream_id(stream_id)?;
        self.device.dequeue_scheduled_frame(stream_id)
    }

    fn handle_get_buffer(&self, arg: usize) -> Result<i32, &'static str> {
        let info = self.buffer_info()?;
        write_user_value(arg, &info)?;
        Ok(0)
    }

    fn status_line(&self) -> String {
        let caps = self.device.backend.capabilities();
        let last_error = self.last_error.lock().unwrap_or("none");
        let backend_status = self.device.backend.debug_status().unwrap_or_default();
        format!(
            "scarlet-video backend={} stateful_h264={} stateful_av1={} stateless_h264={} stateless_vp9={} sessions={} inflight={} input={} output={} last_error={}{}\n",
            self.device.backend.name(),
            caps.supports_h264,
            caps.supports_av1,
            caps.supports_stateless_h264,
            self.device.backend.supports_stateless_vp9(),
            caps.max_sessions,
            caps.max_inflight_decodes,
            caps.mapped_input_len,
            caps.mapped_output_len,
            last_error,
            backend_status
        )
    }
}

impl Drop for ScarletVideoOpen {
    fn drop(&mut self) {
        let stream_id = *self.stream_id.lock();
        if let Some(stream_id) = stream_id {
            crate::println!(
                "[scarlet-video] drop begin open={:#x} stream={}",
                self as *const _ as usize,
                stream_id
            );
            match self.device.destroy_scheduled_stream(stream_id) {
                Ok(()) => {
                    self.device
                        .log_backend_state("drop destroy ok", stream_id, None);
                    *self.stream_id.lock() = None;
                }
                Err(e) => {
                    self.device
                        .log_backend_state("drop destroy failed", stream_id, Some(e));
                    *self.last_error.lock() = Some(e);
                }
            }
        } else {
            crate::println!(
                "[scarlet-video] drop without session open={:#x}",
                self as *const _ as usize
            );
        }
    }
}

impl Device for ScarletVideoOpen {
    fn open(self: Arc<Self>) -> Result<Arc<dyn Device>, &'static str> {
        Ok(self)
    }

    fn device_type(&self) -> DeviceType {
        self.device.device_type()
    }

    fn name(&self) -> &'static str {
        self.device.name()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for ScarletVideoOpen {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("scarlet-video: write a complete coded access unit")
    }

    fn read(&self, buffer: &mut [u8]) -> usize {
        let status = self.status_line();
        let bytes = status.as_bytes();
        let count = core::cmp::min(buffer.len(), bytes.len());
        buffer[..count].copy_from_slice(&bytes[..count]);
        count
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        let layout = self.device.buffer_layout()?;
        if buffer.len() > layout.input_len {
            return Err("scarlet-video: input exceeds mapped buffer");
        }

        self.ensure_mapped_buffer(layout)?;
        {
            let mut mapped_buffer = self.mapped_buffer.lock();
            let buffer_pages = mapped_buffer
                .as_mut()
                .ok_or("scarlet-video: mmap buffer missing")?;
            // SAFETY: `buffer_pages` owns at least `layout.input_len` bytes and
            // `buffer.len()` was checked against that capacity. The source and
            // destination do not overlap.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buffer.as_ptr(),
                    buffer_pages.as_ptr() as *mut u8,
                    buffer.len(),
                );
            }
        }

        match self.submit_mapped(
            SCARLET_VIDEO_FORMAT_H264,
            buffer.len(),
            self.next_timestamp(),
        ) {
            Ok(()) => {
                *self.last_error.lock() = None;
                Ok(buffer.len())
            }
            Err(e) => {
                *self.last_error.lock() = Some(e);
                Err(e)
            }
        }
    }

    fn can_read(&self) -> bool {
        true
    }

    fn can_write(&self) -> bool {
        true
    }

    fn read_at(&self, _position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        Ok(self.read(buffer))
    }
}

impl ControlOps for ScarletVideoOpen {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            SCARLET_VIDEO_GET_BUFFER => self.handle_get_buffer(arg),
            SCARLET_VIDEO_GET_CAPS => self.device.handle_get_caps(arg),
            SCARLET_VIDEO_CREATE_SESSION => {
                let mut info: ScarletVideoSessionInfo = read_user_value(arg)?;
                let stream_id = self.create_or_query_session(info.stream_id, info.padding)?;
                info.stream_id = stream_id;
                info.padding = 0;
                info.buffer = self.buffer_info()?;
                write_user_value(arg, &info)?;
                Ok(0)
            }
            SCARLET_VIDEO_DESTROY_SESSION => {
                let info: ScarletVideoSessionInfo = read_user_value(arg)?;
                self.destroy_current_session(info.stream_id)?;
                Ok(0)
            }
            SCARLET_VIDEO_SUBMIT => {
                let submit: ScarletVideoSubmit = read_user_value(arg)?;
                match self.submit_mapped(
                    submit.coded_format,
                    submit.input_len as usize,
                    submit.timestamp,
                ) {
                    Ok(()) => {
                        *self.last_error.lock() = None;
                        Ok(0)
                    }
                    Err(e) => {
                        *self.last_error.lock() = Some(e);
                        Err(e)
                    }
                }
            }
            SCARLET_VIDEO_SUBMIT_SESSION => {
                let submit: ScarletVideoSessionSubmit = read_user_value(arg)?;
                let stream_id = self.checked_stream_id(submit.stream_id)?;
                match self.submit_mapped_for_stream(
                    stream_id,
                    submit.coded_format,
                    submit.input_len as usize,
                    submit.timestamp,
                ) {
                    Ok(()) => {
                        *self.last_error.lock() = None;
                        Ok(0)
                    }
                    Err(e) => {
                        *self.last_error.lock() = Some(e);
                        Err(e)
                    }
                }
            }
            SCARLET_VIDEO_SUBMIT_H264_STATELESS => {
                let submit: ScarletVideoH264StatelessSubmit = read_user_value(arg)?;
                let result = (|| {
                    if submit.flags != 0 {
                        return Err("scarlet-video: stateless H.264 submit flags must be zero");
                    }
                    if !self.device.backend.capabilities().supports_stateless_h264 {
                        return Err("scarlet-video: backend does not support stateless H.264");
                    }
                    let stream_id = self.checked_stream_id(submit.stream_id)?;
                    let decode = self.mapped_decode_request(
                        stream_id,
                        SCARLET_VIDEO_FORMAT_H264,
                        submit.input_len as usize,
                        submit.timestamp,
                    )?;
                    let h264 = self.device.h264_stateless_params(submit.params)?;
                    let request = VideoBackendH264StatelessRequest { decode, h264 };
                    self.device
                        .enqueue_decode_job(VideoQueuedJob::H264Stateless(request))
                })();

                match result {
                    Ok(()) => {
                        *self.last_error.lock() = None;
                        Ok(0)
                    }
                    Err(e) => {
                        *self.last_error.lock() = Some(e);
                        Err(e)
                    }
                }
            }
            SCARLET_VIDEO_SUBMIT_VP9_STATELESS => {
                let submit: ScarletVideoVp9StatelessSubmit = read_user_value(arg)?;
                let result = (|| {
                    if submit.flags != 0 {
                        return Err("scarlet-video: stateless VP9 submit flags must be zero");
                    }
                    if !self.device.backend.supports_stateless_vp9() {
                        return Err("scarlet-video: backend does not support stateless VP9");
                    }
                    let stream_id = self.checked_stream_id(submit.stream_id)?;
                    let decode = self.mapped_decode_request(
                        stream_id,
                        SCARLET_VIDEO_FORMAT_VP9,
                        submit.input_len as usize,
                        submit.timestamp,
                    )?;
                    let vp9 = self.device.vp9_stateless_params(submit.params)?;
                    let request = VideoBackendVp9StatelessRequest { decode, vp9 };
                    self.device
                        .enqueue_decode_job(VideoQueuedJob::Vp9Stateless(request))
                })();

                match result {
                    Ok(()) => {
                        *self.last_error.lock() = None;
                        Ok(0)
                    }
                    Err(e) => {
                        *self.last_error.lock() = Some(e);
                        Err(e)
                    }
                }
            }
            SCARLET_VIDEO_DEQUEUE => {
                let decoded = match self.dequeue_frame(0) {
                    Ok(decoded) => decoded,
                    Err(e) => {
                        *self.last_error.lock() = Some(e);
                        return Err(e);
                    }
                };
                let Some(decoded) = decoded else {
                    *self.last_error.lock() = None;
                    return Ok(0);
                };
                write_user_value(arg, &decoded.frame)?;
                *self.last_error.lock() = None;
                Ok(1)
            }
            SCARLET_VIDEO_DEQUEUE_SESSION => {
                let mut dequeued: ScarletVideoSessionDequeuedFrame = read_user_value(arg)?;
                let decoded = match self.dequeue_frame(dequeued.stream_id) {
                    Ok(decoded) => decoded,
                    Err(e) => {
                        *self.last_error.lock() = Some(e);
                        return Err(e);
                    }
                };
                let Some(decoded) = decoded else {
                    *self.last_error.lock() = None;
                    return Ok(0);
                };
                dequeued.stream_id = decoded.stream_id;
                dequeued.padding = 0;
                dequeued.frame = decoded.frame;
                write_user_value(arg, &dequeued)?;
                *self.last_error.lock() = None;
                Ok(1)
            }
            _ => Err("scarlet-video: unsupported control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        self.device.supported_control_commands()
    }
}

impl MemoryMappingOps for ScarletVideoOpen {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        let layout = self.device.buffer_layout()?;
        if offset % PAGE_SIZE != 0 || length % PAGE_SIZE != 0 {
            return Err("scarlet-video: mmap offset and length must be page-aligned");
        }
        if offset >= layout.mmap_len {
            return Err("scarlet-video: mmap offset exceeds buffer size");
        }
        if length > layout.mmap_len - offset {
            return Err("scarlet-video: mmap length exceeds buffer size");
        }

        self.ensure_mapped_buffer(layout)?;
        let mapped_buffer = self.mapped_buffer.lock();
        let buffer = mapped_buffer
            .as_ref()
            .ok_or("scarlet-video: mmap buffer missing")?;
        let stream_id = (*self.stream_id.lock()).unwrap_or(0);
        crate::println!(
            "[scarlet-video] mmap map open={:#x} stream={} offset={} length={} paddr={:#x}",
            self as *const _ as usize,
            stream_id,
            offset,
            length,
            buffer.as_paddr() + offset
        );
        Ok(MemoryMappingInfo::new(
            buffer.as_paddr() + offset,
            0x3,
            true,
        ))
    }

    fn supports_mmap(&self) -> bool {
        true
    }

    fn mmap_owner_name(&self) -> String {
        String::from("scarlet-video")
    }
}

impl Selectable for ScarletVideoOpen {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        self.device.current_ready(interest)
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut Trapframe,
        timeout_ticks: Option<u64>,
        min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        self.device
            .wait_until_ready(interest, trapframe, timeout_ticks, min_wait_ticks)
    }

    fn is_nonblocking(&self) -> bool {
        self.device.is_nonblocking()
    }
}

#[derive(Clone, Copy)]
struct VideoBufferLayout {
    input_len: usize,
    output_offset: usize,
    output_len: usize,
    mmap_len: usize,
}

impl Device for ScarletVideoDevice {
    fn open(self: Arc<Self>) -> Result<Arc<dyn Device>, &'static str> {
        Ok(Arc::new(ScarletVideoOpen::new(self)?))
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "scarlet-video"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for ScarletVideoDevice {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("scarlet-video: write a complete coded access unit")
    }

    fn read(&self, buffer: &mut [u8]) -> usize {
        let status = self.status_line();
        let bytes = status.as_bytes();
        let count = core::cmp::min(buffer.len(), bytes.len());
        buffer[..count].copy_from_slice(&bytes[..count]);
        count
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        let layout = self.buffer_layout()?;
        if buffer.len() > layout.input_len {
            return Err("scarlet-video: input exceeds mapped buffer");
        }

        self.ensure_mapped_buffer(layout)?;
        {
            let mut mapped_buffer = self.mapped_buffer.lock();
            let buffer_pages = mapped_buffer
                .as_mut()
                .ok_or("scarlet-video: mmap buffer missing")?;
            // SAFETY: `buffer_pages` owns at least `layout.input_len` bytes and
            // `buffer.len()` was checked against that capacity. The source and
            // destination do not overlap.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buffer.as_ptr(),
                    buffer_pages.as_ptr() as *mut u8,
                    buffer.len(),
                );
            }
        }

        match self.submit_mapped(
            DEFAULT_STREAM_ID,
            SCARLET_VIDEO_FORMAT_H264,
            buffer.len(),
            self.next_timestamp(),
        ) {
            Ok(()) => {
                *self.last_error.lock() = None;
                Ok(buffer.len())
            }
            Err(e) => {
                *self.last_error.lock() = Some(e);
                Err(e)
            }
        }
    }

    fn can_read(&self) -> bool {
        true
    }

    fn can_write(&self) -> bool {
        true
    }

    fn read_at(&self, _position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        Ok(self.read(buffer))
    }
}

impl ControlOps for ScarletVideoDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            SCARLET_VIDEO_GET_BUFFER => self.handle_get_buffer(arg),
            SCARLET_VIDEO_SUBMIT => self.handle_submit(arg),
            SCARLET_VIDEO_DEQUEUE => self.handle_dequeue(arg),
            SCARLET_VIDEO_CREATE_SESSION => self.handle_create_session(arg),
            SCARLET_VIDEO_SUBMIT_SESSION => self.handle_submit_session(arg),
            SCARLET_VIDEO_DEQUEUE_SESSION => self.handle_dequeue_session(arg),
            SCARLET_VIDEO_DESTROY_SESSION => self.handle_destroy_session(arg),
            SCARLET_VIDEO_GET_CAPS => self.handle_get_caps(arg),
            SCARLET_VIDEO_SUBMIT_H264_STATELESS => self.handle_submit_h264_stateless(arg),
            SCARLET_VIDEO_SUBMIT_VP9_STATELESS => self.handle_submit_vp9_stateless(arg),
            _ => Err("scarlet-video: unsupported control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        alloc::vec![
            (SCARLET_VIDEO_GET_BUFFER, "Get mmap video buffer layout"),
            (
                SCARLET_VIDEO_SUBMIT,
                "Submit mmap-written coded video access unit"
            ),
            (SCARLET_VIDEO_DEQUEUE, "Dequeue a decoded mmap video frame"),
            (
                SCARLET_VIDEO_CREATE_SESSION,
                "Create or query mmap video stream session"
            ),
            (
                SCARLET_VIDEO_SUBMIT_SESSION,
                "Submit mmap-written coded video access unit for a stream"
            ),
            (
                SCARLET_VIDEO_DEQUEUE_SESSION,
                "Dequeue a decoded mmap video frame for a stream"
            ),
            (
                SCARLET_VIDEO_DESTROY_SESSION,
                "Destroy mmap video stream session"
            ),
            (SCARLET_VIDEO_GET_CAPS, "Get video backend capabilities"),
            (
                SCARLET_VIDEO_SUBMIT_H264_STATELESS,
                "Submit stateless H.264 decode request for a stream"
            ),
            (
                SCARLET_VIDEO_SUBMIT_VP9_STATELESS,
                "Submit stateless VP9 decode request for a stream"
            ),
        ]
    }
}

impl MemoryMappingOps for ScarletVideoDevice {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        let layout = self.buffer_layout()?;
        if offset % PAGE_SIZE != 0 || length % PAGE_SIZE != 0 {
            return Err("scarlet-video: mmap offset and length must be page-aligned");
        }
        if offset >= layout.mmap_len {
            return Err("scarlet-video: mmap offset exceeds buffer size");
        }
        if length > layout.mmap_len - offset {
            return Err("scarlet-video: mmap length exceeds buffer size");
        }

        self.ensure_mapped_buffer(layout)?;
        let mapped_buffer = self.mapped_buffer.lock();
        let buffer = mapped_buffer
            .as_ref()
            .ok_or("scarlet-video: mmap buffer missing")?;
        crate::println!(
            "[scarlet-video] legacy mmap map offset={} length={} paddr={:#x}",
            offset,
            length,
            buffer.as_paddr() + offset
        );
        Ok(MemoryMappingInfo::new(
            buffer.as_paddr() + offset,
            0x3,
            true,
        ))
    }

    fn supports_mmap(&self) -> bool {
        true
    }

    fn mmap_owner_name(&self) -> String {
        String::from("scarlet-video")
    }
}

impl Selectable for ScarletVideoDevice {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        self.scheduler_ready(interest)
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        trapframe: &mut Trapframe,
        timeout_ticks: Option<u64>,
        min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        let current = self.current_ready(interest);
        if (interest.read && current.read) || (interest.write && current.write) {
            return SelectWaitOutcome::Ready;
        }

        let task_id = {
            use crate::arch::get_cpu;
            let cpu_id = get_cpu().get_cpuid();
            crate::sched::scheduler::current_task_id(cpu_id).unwrap_or(0)
        };

        let woke = if min_wait_ticks > 0 {
            self.completion_waker.wait_with_min_timeout(
                task_id,
                trapframe,
                timeout_ticks,
                min_wait_ticks,
            )
        } else {
            self.completion_waker
                .wait_with_timeout(task_id, trapframe, timeout_ticks)
        };

        if timeout_ticks.is_some() && !woke {
            let after = self.current_ready(interest);
            if !((interest.read && after.read) || (interest.write && after.write)) {
                return SelectWaitOutcome::TimedOut;
            }
        }

        SelectWaitOutcome::Ready
    }

    fn is_nonblocking(&self) -> bool {
        true
    }
}

impl VideoCompletionNotifier for ScarletVideoDevice {
    fn notify_video_completion(&self) {
        self.completion_waker.wake_all();
    }
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
    /// Offset of frame payload inside the full mapped video buffer.
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

/// H.264 sequence parameter set for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ScarletVideoH264Sps {
    /// H.264 `profile_idc`.
    pub profile_idc: u8,
    /// H.264 constraint set flags.
    pub constraint_set_flags: u8,
    /// H.264 `level_idc`.
    pub level_idc: u8,
    /// H.264 sequence parameter set id.
    pub seq_parameter_set_id: u8,
    /// H.264 chroma format idc.
    pub chroma_format_idc: u8,
    /// H.264 luma bit depth minus eight.
    pub bit_depth_luma_minus8: u8,
    /// H.264 chroma bit depth minus eight.
    pub bit_depth_chroma_minus8: u8,
    /// H.264 log2 max frame number minus four.
    pub log2_max_frame_num_minus4: u8,
    /// H.264 picture order count type.
    pub pic_order_cnt_type: u8,
    /// H.264 log2 max picture order count LSB minus four.
    pub log2_max_pic_order_cnt_lsb_minus4: u8,
    /// H.264 maximum reference frame count.
    pub max_num_ref_frames: u8,
    /// H.264 POC type 1 offset cycle length.
    pub num_ref_frames_in_pic_order_cnt_cycle: u8,
    /// H.264 POC type 1 reference offsets.
    pub offset_for_ref_frame: [i32; 255],
    /// H.264 POC type 1 non-reference offset.
    pub offset_for_non_ref_pic: i32,
    /// H.264 POC type 1 top-to-bottom offset.
    pub offset_for_top_to_bottom_field: i32,
    /// H.264 coded width in macroblocks minus one.
    pub pic_width_in_mbs_minus1: u16,
    /// H.264 coded height in map units minus one.
    pub pic_height_in_map_units_minus1: u16,
    /// H.264 left frame crop offset.
    pub frame_crop_left_offset: u32,
    /// H.264 right frame crop offset.
    pub frame_crop_right_offset: u32,
    /// H.264 top frame crop offset.
    pub frame_crop_top_offset: u32,
    /// H.264 bottom frame crop offset.
    pub frame_crop_bottom_offset: u32,
    /// `SCARLET_VIDEO_H264_SPS_FLAG_*` bitset.
    pub flags: u32,
}

impl Default for ScarletVideoH264Sps {
    fn default() -> Self {
        Self {
            profile_idc: 0,
            constraint_set_flags: 0,
            level_idc: 0,
            seq_parameter_set_id: 0,
            chroma_format_idc: 0,
            bit_depth_luma_minus8: 0,
            bit_depth_chroma_minus8: 0,
            log2_max_frame_num_minus4: 0,
            pic_order_cnt_type: 0,
            log2_max_pic_order_cnt_lsb_minus4: 0,
            max_num_ref_frames: 0,
            num_ref_frames_in_pic_order_cnt_cycle: 0,
            offset_for_ref_frame: [0; 255],
            offset_for_non_ref_pic: 0,
            offset_for_top_to_bottom_field: 0,
            pic_width_in_mbs_minus1: 0,
            pic_height_in_map_units_minus1: 0,
            frame_crop_left_offset: 0,
            frame_crop_right_offset: 0,
            frame_crop_top_offset: 0,
            frame_crop_bottom_offset: 0,
            flags: 0,
        }
    }
}

/// H.264 picture parameter set for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoH264Pps {
    /// H.264 picture parameter set id.
    pub pic_parameter_set_id: u8,
    /// Referenced sequence parameter set id.
    pub seq_parameter_set_id: u8,
    /// H.264 slice group count minus one.
    pub num_slice_groups_minus1: u8,
    /// Default active L0 reference count minus one.
    pub num_ref_idx_l0_default_active_minus1: u8,
    /// Default active L1 reference count minus one.
    pub num_ref_idx_l1_default_active_minus1: u8,
    /// H.264 weighted bipred idc.
    pub weighted_bipred_idc: u8,
    /// H.264 initial picture QP minus 26.
    pub pic_init_qp_minus26: i8,
    /// H.264 initial picture QS minus 26.
    pub pic_init_qs_minus26: i8,
    /// H.264 chroma QP index offset.
    pub chroma_qp_index_offset: i8,
    /// H.264 second chroma QP index offset.
    pub second_chroma_qp_index_offset: i8,
    /// `SCARLET_VIDEO_H264_PPS_FLAG_*` bitset.
    pub flags: u16,
}

/// H.264 scaling matrices for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ScarletVideoH264ScalingMatrix {
    /// H.264 4x4 scaling lists in raster order.
    pub scaling_list_4x4: [[u8; 16]; 6],
    /// H.264 8x8 scaling lists in raster order.
    pub scaling_list_8x8: [[u8; 64]; 6],
}

impl Default for ScarletVideoH264ScalingMatrix {
    fn default() -> Self {
        Self {
            scaling_list_4x4: [[0; 16]; 6],
            scaling_list_8x8: [[0; 64]; 6],
        }
    }
}

/// One H.264 reference picture list entry.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScarletVideoH264Reference {
    /// Reference field selector.
    pub fields: u8,
    /// Index into `ScarletVideoH264DecodeParams::dpb`.
    pub index: u8,
}

/// H.264 prediction weight factors.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoH264WeightFactors {
    /// Luma weights.
    pub luma_weight: [i16; 32],
    /// Luma offsets.
    pub luma_offset: [i16; 32],
    /// Chroma weights.
    pub chroma_weight: [[i16; 2]; 32],
    /// Chroma offsets.
    pub chroma_offset: [[i16; 2]; 32],
}

/// H.264 prediction weights for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoH264PredWeights {
    /// H.264 luma log2 weight denominator.
    pub luma_log2_weight_denom: u16,
    /// H.264 chroma log2 weight denominator.
    pub chroma_log2_weight_denom: u16,
    /// List 0 and list 1 weight factors.
    pub weight_factors: [ScarletVideoH264WeightFactors; 2],
}

/// H.264 per-slice parameters for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ScarletVideoH264SliceParams {
    /// Offset in bits from the slice NAL payload start to `slice_data()`.
    pub header_bit_size: u32,
    /// Byte offset of the slice NAL header in the mapped coded input.
    pub nal_offset: u32,
    /// Byte length of the slice NAL payload including the NAL header.
    pub nal_len: u32,
    /// H.264 first macroblock in slice.
    pub first_mb_in_slice: u32,
    /// H.264 slice type.
    pub slice_type: u8,
    /// H.264 picture parameter set id referenced by this slice.
    pub pic_parameter_set_id: u8,
    /// H.264 colour plane id.
    pub colour_plane_id: u8,
    /// H.264 redundant picture count.
    pub redundant_pic_cnt: u8,
    /// H.264 CABAC init idc.
    pub cabac_init_idc: u8,
    /// H.264 slice QP delta.
    pub slice_qp_delta: i8,
    /// H.264 slice QS delta.
    pub slice_qs_delta: i8,
    /// H.264 deblocking filter idc.
    pub disable_deblocking_filter_idc: u8,
    /// H.264 alpha C0 deblocking offset divided by two.
    pub slice_alpha_c0_offset_div2: i8,
    /// H.264 beta deblocking offset divided by two.
    pub slice_beta_offset_div2: i8,
    /// Active L0 reference count minus one.
    pub num_ref_idx_l0_active_minus1: u8,
    /// Active L1 reference count minus one.
    pub num_ref_idx_l1_active_minus1: u8,
    /// Reserved padding.
    pub reserved: u8,
    /// Reference picture list 0.
    pub ref_pic_list0: [ScarletVideoH264Reference; 32],
    /// Reference picture list 1.
    pub ref_pic_list1: [ScarletVideoH264Reference; 32],
    /// `SCARLET_VIDEO_H264_SLICE_FLAG_*` bitset.
    pub flags: u32,
}

impl Default for ScarletVideoH264SliceParams {
    fn default() -> Self {
        Self {
            header_bit_size: 0,
            nal_offset: 0,
            nal_len: 0,
            first_mb_in_slice: 0,
            slice_type: 0,
            pic_parameter_set_id: 0,
            colour_plane_id: 0,
            redundant_pic_cnt: 0,
            cabac_init_idc: 0,
            slice_qp_delta: 0,
            slice_qs_delta: 0,
            disable_deblocking_filter_idc: 0,
            slice_alpha_c0_offset_div2: 0,
            slice_beta_offset_div2: 0,
            num_ref_idx_l0_active_minus1: 0,
            num_ref_idx_l1_active_minus1: 0,
            reserved: 0,
            ref_pic_list0: [ScarletVideoH264Reference::default(); 32],
            ref_pic_list1: [ScarletVideoH264Reference::default(); 32],
            flags: 0,
        }
    }
}

/// H.264 decoded picture buffer entry.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoH264DpbEntry {
    /// Timestamp identifying the decoded reference frame.
    pub reference_ts: u64,
    /// H.264 PicNum.
    pub pic_num: i32,
    /// H.264 frame_num.
    pub frame_num: u16,
    /// Reference field selector.
    pub fields: u8,
    /// Reserved padding.
    pub reserved: [u8; 5],
    /// H.264 top field order count.
    pub top_field_order_cnt: i32,
    /// H.264 bottom field order count.
    pub bottom_field_order_cnt: i32,
    /// `SCARLET_VIDEO_H264_DPB_FLAG_*` bitset.
    pub flags: u32,
}

/// H.264 per-frame decode parameters for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoH264DecodeParams {
    /// Decoded picture buffer entries.
    pub dpb: [ScarletVideoH264DpbEntry; 16],
    /// H.264 NAL reference idc.
    pub nal_ref_idc: u16,
    /// H.264 frame number.
    pub frame_num: u16,
    /// H.264 top field order count.
    pub top_field_order_cnt: i32,
    /// H.264 bottom field order count.
    pub bottom_field_order_cnt: i32,
    /// H.264 IDR picture id.
    pub idr_pic_id: u16,
    /// H.264 picture order count LSB.
    pub pic_order_cnt_lsb: u16,
    /// H.264 bottom POC delta.
    pub delta_pic_order_cnt_bottom: i32,
    /// H.264 POC delta 0.
    pub delta_pic_order_cnt0: i32,
    /// H.264 POC delta 1.
    pub delta_pic_order_cnt1: i32,
    /// Bit size of `dec_ref_pic_marking()`.
    pub dec_ref_pic_marking_bit_size: u32,
    /// Bit size of POC syntax in the slice header.
    pub pic_order_cnt_bit_size: u32,
    /// H.264 slice group change cycle.
    pub slice_group_change_cycle: u32,
    /// Reserved padding.
    pub reserved: u32,
    /// `SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_*` bitset.
    pub flags: u32,
}

/// Copied stateless H.264 parameters for a backend submit.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoH264StatelessParams {
    /// H.264 sequence parameter set.
    pub sps: ScarletVideoH264Sps,
    /// H.264 picture parameter set.
    pub pps: ScarletVideoH264Pps,
    /// H.264 scaling matrices.
    pub scaling_matrix: ScarletVideoH264ScalingMatrix,
    /// H.264 prediction weights.
    pub pred_weights: ScarletVideoH264PredWeights,
    /// H.264 slice parameters.
    pub slice_params: ScarletVideoH264SliceParams,
    /// H.264 decode parameters.
    pub decode_params: ScarletVideoH264DecodeParams,
}

/// VP9 loop filter parameters for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoVp9LoopFilter {
    /// VP9 reference loop-filter deltas.
    pub ref_deltas: [i8; 4],
    /// VP9 mode loop-filter deltas.
    pub mode_deltas: [i8; 2],
    /// VP9 loop filter level.
    pub level: u8,
    /// VP9 loop filter sharpness.
    pub sharpness: u8,
    /// `SCARLET_VIDEO_VP9_LOOP_FILTER_FLAG_*` bitset.
    pub flags: u8,
    /// Reserved padding.
    pub reserved: [u8; 7],
}

/// VP9 quantization parameters for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoVp9Quantization {
    /// VP9 base qindex.
    pub base_q_idx: u8,
    /// VP9 Y DC quantizer delta.
    pub delta_q_y_dc: i8,
    /// VP9 UV DC quantizer delta.
    pub delta_q_uv_dc: i8,
    /// VP9 UV AC quantizer delta.
    pub delta_q_uv_ac: i8,
    /// Reserved padding.
    pub reserved: [u8; 4],
}

/// VP9 segmentation parameters for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoVp9Segmentation {
    /// Segment feature data indexed by segment and feature id.
    pub feature_data: [[i16; 4]; 8],
    /// Segment feature enable masks.
    pub feature_enabled: [u8; 8],
    /// Segment tree probabilities.
    pub tree_probs: [u8; 7],
    /// Temporal prediction probabilities.
    pub pred_probs: [u8; 3],
    /// `SCARLET_VIDEO_VP9_SEGMENTATION_FLAG_*` bitset.
    pub flags: u8,
    /// Reserved padding.
    pub reserved: [u8; 5],
}

/// VP9 per-frame parameters for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoVp9FrameParams {
    /// VP9 loop filter parameters.
    pub loop_filter: ScarletVideoVp9LoopFilter,
    /// VP9 quantization parameters.
    pub quantization: ScarletVideoVp9Quantization,
    /// VP9 segmentation parameters.
    pub segmentation: ScarletVideoVp9Segmentation,
    /// `SCARLET_VIDEO_VP9_FRAME_FLAG_*` bitset.
    pub flags: u32,
    /// Compressed VP9 header byte size.
    pub compressed_header_size: u16,
    /// Uncompressed VP9 header byte size.
    pub uncompressed_header_size: u16,
    /// VP9 coded frame width minus one.
    pub frame_width_minus_1: u16,
    /// VP9 coded frame height minus one.
    pub frame_height_minus_1: u16,
    /// VP9 render width minus one.
    pub render_width_minus_1: u16,
    /// VP9 render height minus one.
    pub render_height_minus_1: u16,
    /// Last reference frame timestamp.
    pub last_frame_ts: u64,
    /// Golden reference frame timestamp.
    pub golden_frame_ts: u64,
    /// Alternate reference frame timestamp.
    pub alt_frame_ts: u64,
    /// VP9 reference sign-bias bitset.
    pub ref_frame_sign_bias: u8,
    /// VP9 frame context reset mode.
    pub reset_frame_context: u8,
    /// VP9 frame context index.
    pub frame_context_idx: u8,
    /// VP9 profile.
    pub profile: u8,
    /// VP9 component bit depth.
    pub bit_depth: u8,
    /// VP9 interpolation filter.
    pub interpolation_filter: u8,
    /// Log2 VP9 tile column count.
    pub tile_cols_log2: u8,
    /// Log2 VP9 tile row count.
    pub tile_rows_log2: u8,
    /// VP9 reference mode.
    pub reference_mode: u8,
    /// VP9 refresh frame flags from the uncompressed header.
    pub refresh_frame_flags: u8,
    /// VP9 show-existing-frame reference index.
    pub show_existing_frame_index: u8,
    /// VP9 transform mode.
    pub tx_mode: u8,
    /// Reserved padding.
    pub reserved: [u8; 4],
}

/// VP9 tile byte range for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoVp9Tile {
    /// Tile row index.
    pub row: u16,
    /// Tile column index.
    pub col: u16,
    /// Byte offset of the tile payload in the mapped coded input.
    pub offset: u32,
    /// Byte size of the tile payload.
    pub size: u32,
}

/// VP9 tile table for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ScarletVideoVp9Tiles {
    /// Number of valid tile entries.
    pub tile_count: u32,
    /// Reserved padding.
    pub reserved: u32,
    /// Valid tile byte ranges.
    pub tiles: [ScarletVideoVp9Tile; SCARLET_VIDEO_VP9_MAX_TILES],
}

impl Default for ScarletVideoVp9Tiles {
    fn default() -> Self {
        Self {
            tile_count: 0,
            reserved: 0,
            tiles: [ScarletVideoVp9Tile::default(); SCARLET_VIDEO_VP9_MAX_TILES],
        }
    }
}

/// VP9 probability state for stateless decode.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ScarletVideoVp9Probabilities {
    /// Packed VP9 probability state prepared by userspace.
    pub data: [u8; SCARLET_VIDEO_VP9_PROBABILITY_BYTES],
}

impl Default for ScarletVideoVp9Probabilities {
    fn default() -> Self {
        Self {
            data: [0; SCARLET_VIDEO_VP9_PROBABILITY_BYTES],
        }
    }
}

/// Copied stateless VP9 parameters for a backend submit.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoVp9StatelessParams {
    /// VP9 per-frame syntax parameters.
    pub frame: ScarletVideoVp9FrameParams,
    /// VP9 packed probability state.
    pub probabilities: ScarletVideoVp9Probabilities,
    /// VP9 tile table.
    pub tiles: ScarletVideoVp9Tiles,
}

/// Userspace pointers to stateless H.264 parameter structures.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoH264ParamPtrs {
    /// Pointer to `ScarletVideoH264Sps`.
    pub sps: u64,
    /// Pointer to `ScarletVideoH264Pps`.
    pub pps: u64,
    /// Pointer to `ScarletVideoH264ScalingMatrix`.
    pub scaling_matrix: u64,
    /// Pointer to `ScarletVideoH264PredWeights`.
    pub pred_weights: u64,
    /// Pointer to `ScarletVideoH264SliceParams`.
    pub slice_params: u64,
    /// Pointer to `ScarletVideoH264DecodeParams`.
    pub decode_params: u64,
}

/// Userspace pointers to stateless VP9 parameter structures.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoVp9ParamPtrs {
    /// Pointer to `ScarletVideoVp9FrameParams`.
    pub frame: u64,
    /// Pointer to `ScarletVideoVp9Probabilities`.
    pub probabilities: u64,
    /// Pointer to `ScarletVideoVp9Tiles`.
    pub tiles: u64,
}

/// Stateless H.264 mapped-buffer submit request.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoH264StatelessSubmit {
    /// Backend stream/session identifier, or zero for the default stream.
    pub stream_id: u32,
    /// Number of bytes written to the mapped input area.
    pub input_len: u32,
    /// Presentation timestamp carried through dequeue.
    pub timestamp: u64,
    /// Pointers to userspace parameter structures.
    pub params: ScarletVideoH264ParamPtrs,
    /// Reserved for future per-submit flags.
    pub flags: u32,
    /// Reserved padding.
    pub padding: u32,
}

/// Stateless VP9 mapped-buffer submit request.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoVp9StatelessSubmit {
    /// Backend stream/session identifier, or zero for the default stream.
    pub stream_id: u32,
    /// Number of bytes written to the mapped input area.
    pub input_len: u32,
    /// Presentation timestamp carried through dequeue.
    pub timestamp: u64,
    /// Pointers to userspace parameter structures.
    pub params: ScarletVideoVp9ParamPtrs,
    /// Reserved for future per-submit flags.
    pub flags: u32,
    /// Reserved padding.
    pub padding: u32,
}

/// User-visible video backend capabilities.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScarletVideoCapabilities {
    /// Structure version.
    pub version: u32,
    /// `SCARLET_VIDEO_CAP_*` bitset.
    pub flags: u32,
    /// Maximum backend sessions.
    pub max_sessions: u32,
    /// Decoded output pixel format.
    pub output_pixel_format: u32,
    /// Mapped input byte capacity.
    pub mapped_input_len: u32,
    /// Mapped output byte capacity.
    pub mapped_output_len: u32,
    /// Reserved for future ABI fields.
    pub reserved: [u32; 8],
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

fn read_user_value<T: Copy>(ptr: usize) -> Result<T, &'static str> {
    if ptr == 0 {
        return Err("scarlet-video: ioctl pointer is null");
    }
    let task = mytask().ok_or("scarlet-video: no current task for ioctl")?;
    let mut value = core::mem::MaybeUninit::<T>::uninit();
    // SAFETY: `value` is uninitialized storage for `T`; this byte slice covers
    // exactly that storage and `copy_from_user` initializes every byte before
    // `assume_init`.
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(value.as_mut_ptr() as *mut u8, core::mem::size_of::<T>())
    };
    copy_from_user(&task, ptr, bytes).map_err(|_| "scarlet-video: failed to copy from user")?;
    // SAFETY: The usercopy above initialized all bytes in `value`.
    Ok(unsafe { value.assume_init() })
}

fn write_user_value<T: Copy>(ptr: usize, value: &T) -> Result<(), &'static str> {
    if ptr == 0 {
        return Err("scarlet-video: ioctl pointer is null");
    }
    let task = mytask().ok_or("scarlet-video: no current task for ioctl")?;
    // SAFETY: `value` is valid for `size_of::<T>()` bytes and is only read.
    let bytes = unsafe {
        core::slice::from_raw_parts(value as *const T as *const u8, core::mem::size_of::<T>())
    };
    copy_to_user(&task, ptr, bytes).map_err(|_| "scarlet-video: failed to copy to user")
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

/// Apple AVD firmware-to-kernel mailbox ABI.
pub mod avd_fw {
    /// Video pipe decode completed.
    pub const MSG_VP_DONE: u32 = 0x0000_0100;
    /// Video pipe decode or IRQ acknowledgement failed.
    pub const MSG_VP_ERROR: u32 = 0x0000_0200;
    /// Post-process pipe completed.
    pub const MSG_PP_DONE: u32 = 0x0000_1000;
    /// Unexpected IRQ vector.
    pub const MSG_UNKNOWN_IRQ: u32 = 0x0001_0000;
}
