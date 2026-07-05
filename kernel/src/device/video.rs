//! Shared Scarlet video decode device ABI definitions.
//!
//! The video decode character device API is currently experimental, but both
//! VirtIO video and Apple AVD backends use the same user-visible control
//! contract. Keep the command values, mapped-buffer structures, and frame
//! constants here so backend implementations do not drift apart.

use alloc::{format, string::String, sync::Arc, vec::Vec};
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

    fn submit_mapped(
        &self,
        stream_id: u32,
        coded_format: u32,
        input_len: usize,
        timestamp: u64,
    ) -> Result<(), &'static str> {
        let layout = self.buffer_layout()?;
        if !self.backend.capabilities().supports_format(coded_format) {
            return Err("scarlet-video: backend does not support coded format");
        }
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
        let request = VideoBackendDecodeRequest {
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
        };
        self.backend.submit_decode(&request)
    }

    fn handle_get_buffer(&self, arg: usize) -> Result<i32, &'static str> {
        let info = self.buffer_info()?;
        write_user_value(arg, &info)?;
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
            "scarlet-video backend={} h264={} av1={} sessions={} input={} output={} last_error={}{}\n",
            self.backend.name(),
            caps.supports_h264,
            caps.supports_av1,
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
