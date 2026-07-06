//! Shared Scarlet video decode device ABI definitions.
//!
//! The video decode character device API is currently experimental, but both
//! VirtIO video and Apple AVD backends use the same user-visible control
//! contract. Keep the command values, mapped-buffer structures, and frame
//! constants here so backend implementations do not drift apart.

use alloc::{boxed::Box, format, string::String, sync::Arc, vec::Vec};
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
use crate::sync::Mutex;
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

/// Version of `ScarletVideoCapabilities`.
pub const SCARLET_VIDEO_CAPS_VERSION: u32 = 1;
/// Backend accepts stateful H.264 access units through legacy submit ioctls.
pub const SCARLET_VIDEO_CAP_STATEFUL_H264: u32 = 1 << 0;
/// Backend accepts stateful AV1 access units through legacy submit ioctls.
pub const SCARLET_VIDEO_CAP_STATEFUL_AV1: u32 = 1 << 1;
/// Backend accepts stateless H.264 requests.
pub const SCARLET_VIDEO_CAP_STATELESS_H264: u32 = 1 << 8;
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
/// H.264 decode request is an IDR picture.
pub const SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR: u32 = 1 << 0;

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
    /// Whether stateful H.264 access units are accepted.
    pub supports_h264: bool,
    /// Whether stateful AV1 access units are accepted.
    pub supports_av1: bool,
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
pub struct VideoBackendH264StatelessRequest {
    /// Common mapped decode buffers.
    pub decode: VideoBackendDecodeRequest,
    /// Copied stateless H.264 parameters supplied by userspace.
    pub h264: Box<ScarletVideoH264StatelessParams>,
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
static VIDEO_DEVICE_COUNTER: AtomicUsize = AtomicUsize::new(0);
const DEFAULT_STREAM_ID: u32 = 1;
const VIDEO_MAPPED_BUFFER_ALIGN: usize = 0x4000;

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
    let device: Arc<dyn Device> = Arc::new(ScarletVideoDevice::new(backend));
    DeviceManager::get_manager().register_device_with_name(name.clone(), device);
    name
}

struct ScarletVideoDevice {
    backend: Arc<dyn VideoDecodeBackend>,
    mapped_buffer: Mutex<Option<ContiguousPages>>,
    last_error: Mutex<Option<&'static str>>,
    next_timestamp: Mutex<u64>,
}

impl ScarletVideoDevice {
    fn new(backend: Arc<dyn VideoDecodeBackend>) -> Self {
        Self {
            backend,
            mapped_buffer: Mutex::new(None),
            last_error: Mutex::new(None),
            next_timestamp: Mutex::new(1),
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
        ScarletVideoCapabilities {
            version: SCARLET_VIDEO_CAPS_VERSION,
            flags: caps.user_flags(),
            max_sessions: caps.max_sessions,
            output_pixel_format: caps.output_pixel_format,
            mapped_input_len: caps.mapped_input_len,
            mapped_output_len: caps.mapped_output_len,
            reserved: [0; 8],
        }
    }

    fn ensure_mapped_buffer(&self, layout: VideoBufferLayout) -> Result<(), &'static str> {
        let mut mapped_buffer = self.mapped_buffer.lock();
        if mapped_buffer.is_none() {
            let pages = layout.mmap_len.div_ceil(PAGE_SIZE);
            *mapped_buffer = ContiguousPages::new_aligned(pages, VIDEO_MAPPED_BUFFER_ALIGN);
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
        self.backend.submit_decode(&request)
    }

    fn h264_stateless_params(
        &self,
        ptrs: ScarletVideoH264ParamPtrs,
    ) -> Result<Box<ScarletVideoH264StatelessParams>, &'static str> {
        if ptrs.sps == 0
            || ptrs.pps == 0
            || ptrs.scaling_matrix == 0
            || ptrs.pred_weights == 0
            || ptrs.slice_params == 0
            || ptrs.decode_params == 0
        {
            return Err("scarlet-video: stateless H.264 parameter pointer is null");
        }

        Ok(Box::new(ScarletVideoH264StatelessParams {
            sps: read_user_value(ptrs.sps as usize)?,
            pps: read_user_value(ptrs.pps as usize)?,
            scaling_matrix: read_user_value(ptrs.scaling_matrix as usize)?,
            pred_weights: read_user_value(ptrs.pred_weights as usize)?,
            slice_params: read_user_value(ptrs.slice_params as usize)?,
            decode_params: read_user_value(ptrs.decode_params as usize)?,
        }))
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
        let stream_id = if info.stream_id == 0 {
            self.backend.create_session(SCARLET_VIDEO_FORMAT_H264)?
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
        self.backend.destroy_session(info.stream_id)?;
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
            self.backend.submit_h264_stateless(&request)
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
        let decoded = match self.backend.dequeue_frame(DEFAULT_STREAM_ID) {
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
        let decoded = match self.backend.dequeue_frame(stream_id) {
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
            "scarlet-video backend={} stateful_h264={} stateful_av1={} stateless_h264={} sessions={} input={} output={} last_error={}{}\n",
            self.backend.name(),
            caps.supports_h264,
            caps.supports_av1,
            caps.supports_stateless_h264,
            caps.max_sessions,
            caps.mapped_input_len,
            caps.mapped_output_len,
            last_error,
            backend_status
        )
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
        let mut set = ReadySet::none();
        if interest.read {
            set.read = true;
        }
        if interest.write {
            set.write = true;
        }
        set
    }

    fn wait_until_ready(
        &self,
        _interest: ReadyInterest,
        _trapframe: &mut Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }

    fn is_nonblocking(&self) -> bool {
        true
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
#[derive(Clone, Copy, Debug, Default)]
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
    pub pic_num: u32,
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
    copy_from_user(task, ptr, bytes).map_err(|_| "scarlet-video: failed to copy from user")?;
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
    copy_to_user(task, ptr, bytes).map_err(|_| "scarlet-video: failed to copy to user")
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
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
