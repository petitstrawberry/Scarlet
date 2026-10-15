//! VirtIO video decode backend.
//!
//! The userspace ABI is provided by the shared `/dev/videoN` frontend in
//! `device::video`. This module owns the VirtIO transport, stream setup, and
//! asynchronous RESOURCE_QUEUE completion handling.

extern crate alloc;

use crate::sync::{IrqRwSpinLock, IrqSpinLock};
use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use crate::device::video::{
    SCARLET_VIDEO_FORMAT_AV1, SCARLET_VIDEO_FORMAT_H264, SCARLET_VIDEO_FORMAT_HEVC,
    SCARLET_VIDEO_FRAME_HEADER_LEN, SCARLET_VIDEO_FRAME_MAGIC, SCARLET_VIDEO_PIXEL_FORMAT_NV12,
    ScarletVideoDequeuedFrame, VideoBackendCapabilities, VideoBackendDecodeRequest,
    VideoBackendDecodedFrame, VideoDecodeBackend,
};
use crate::drivers::virtio::device::Register;
use crate::drivers::virtio::features::VIRTIO_F_VERSION_1;
use crate::drivers::virtio::{
    device::VirtioDevice,
    pci::VirtioPciTransport,
    queue::{DescriptorFlag, VirtQueue},
};
use crate::environment::PAGE_SIZE;
use crate::interrupt::{InterruptClaim, InterruptId};
use crate::mem::page::ContiguousPages;
use crate::vm::addr::{phys_to_virt, virt_to_phys};

const QUEUE_COMMAND: usize = 0;
const QUEUE_COUNT: usize = 2;
const QUEUE_SIZE: usize = 256;
const CONTROL_SPIN_LIMIT: usize = 1_000_000;

const VIRTIO_VIDEO_F_RESOURCE_GUEST_PAGES: u32 = 0;

const VIRTIO_VIDEO_CMD_QUERY_CAPABILITY: u32 = 256;
const VIRTIO_VIDEO_CMD_STREAM_CREATE: u32 = 257;
const VIRTIO_VIDEO_CMD_RESOURCE_CREATE: u32 = 260;
const VIRTIO_VIDEO_CMD_RESOURCE_QUEUE: u32 = 261;
const VIRTIO_VIDEO_CMD_RESOURCE_DESTROY_ALL: u32 = 262;
const VIRTIO_VIDEO_RESP_OK_NODATA: u32 = 512;
const VIRTIO_VIDEO_RESP_OK_QUERY_CAPABILITY: u32 = 513;
const VIRTIO_VIDEO_RESP_OK_RESOURCE_QUEUE: u32 = 514;
const VIRTIO_VIDEO_QUEUE_TYPE_INPUT: u32 = 256;
const VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT: u32 = 257;
const VIRTIO_VIDEO_PLANES_LAYOUT_SINGLE_BUFFER: u32 = 1;
const VIRTIO_VIDEO_FORMAT_H264: u32 = SCARLET_VIDEO_FORMAT_H264;
const VIRTIO_VIDEO_FORMAT_HEVC: u32 = SCARLET_VIDEO_FORMAT_HEVC;
const VIRTIO_VIDEO_FORMAT_AV1: u32 = SCARLET_VIDEO_FORMAT_AV1;
const VIRTIO_VIDEO_MEM_TYPE_GUEST_PAGES: u32 = 0;

const DEFAULT_STREAM_ID: u32 = 1;
const MAX_VIDEO_SESSIONS: usize = 4;
const INPUT_RESOURCE_ID: u32 = 1;
const OUTPUT_RESOURCE_ID: u32 = 2;
const MAX_DECODED_FRAME_BYTES: usize = 16 * 1024 * 1024;
const MAPPED_INPUT_BYTES: usize = 8 * 1024 * 1024;
const MAPPED_OUTPUT_BYTES: usize = align_up_const(
    MAX_DECODED_FRAME_BYTES + SCARLET_VIDEO_FRAME_HEADER_LEN,
    PAGE_SIZE,
);

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioVideoCmdHdr {
    type_: u32,
    stream_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioVideoQueryCapability {
    hdr: VirtioVideoCmdHdr,
    queue_type: u32,
    padding: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioVideoStreamCreate {
    hdr: VirtioVideoCmdHdr,
    in_mem_type: u32,
    out_mem_type: u32,
    coded_format: u32,
    padding: [u8; 4],
    tag: [u8; 64],
}

struct DecodedFrameState {
    frame_count: u64,
    last_error: Option<&'static str>,
}

#[derive(Clone, Copy)]
struct MappedFrameInfo {
    width: u32,
    height: u32,
    pixel_format: u32,
    payload_offset: u64,
    payload_len: u32,
    timestamp: u64,
}

enum PendingDecodeBuffer {
    ExternalMapped {
        output_paddr: u64,
        buffer_lease: Arc<ContiguousPages>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct MappedResourceSet {
    input_paddr: u64,
    input_len: u32,
    output_paddr: u64,
    output_len: u32,
}

struct PendingDecode {
    output_req_desc: Option<usize>,
    input_req_desc: usize,
    input_done: bool,
    buffer: PendingDecodeBuffer,
    output_len: usize,
    output_offset: usize,
    timestamp: u64,
}

impl PendingDecode {
    /// Consume one used chain while retaining the allocation until both return.
    fn complete_descriptor(&mut self, used_desc: usize) -> Option<bool> {
        if Some(used_desc) == self.output_req_desc {
            self.output_req_desc = None;
        } else if used_desc == self.input_req_desc && !self.input_done {
            self.input_done = true;
        } else {
            return None;
        }
        Some(self.output_req_desc.is_none() && self.input_done)
    }
}

struct CommandBuffers {
    req_alloc: ContiguousPages,
    resp_alloc: ContiguousPages,
}

impl CommandBuffers {
    fn new() -> Option<Self> {
        Some(Self {
            req_alloc: ContiguousPages::new(1)?,
            resp_alloc: ContiguousPages::new(1)?,
        })
    }
}

/// Ownership of the shared control-command pages and their published chain.
/// All used-ring consumers must route this completion before asynchronous work.
#[derive(Default)]
struct SyncCommandState {
    pending_desc: Option<usize>,
    completed: bool,
}

impl SyncCommandState {
    fn begin(&mut self, descriptor: usize) -> Result<(), &'static str> {
        if self.pending_desc.is_some() {
            return Err("VirtIO video previous command is still pending");
        }
        self.pending_desc = Some(descriptor);
        self.completed = false;
        Ok(())
    }

    fn complete_descriptor(&mut self, descriptor: usize) -> bool {
        if self.pending_desc != Some(descriptor) {
            return false;
        }
        self.pending_desc = None;
        self.completed = true;
        true
    }
}

const SESSION_ACTIVE: u8 = 0;
const SESSION_RETIRING: u8 = 1;
const SESSION_REAPING: u8 = 2;

/// Exactly one caller may clean up an ownerless session. Failed teardown
/// returns to RETIRING without permitting allocation; success returns ACTIVE.
struct SessionRetirement<'a> {
    state: &'a AtomicU8,
    finished: bool,
}

impl<'a> SessionRetirement<'a> {
    fn claim(state: &'a AtomicU8, retry_only: bool) -> Result<Self, &'static str> {
        let previous = state.load(Ordering::Acquire);
        if previous == SESSION_REAPING || (retry_only && previous != SESSION_RETIRING) {
            return Err("VirtIO video session retirement is already claimed");
        }
        state
            .compare_exchange(
                previous,
                SESSION_REAPING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| "VirtIO video session retirement changed")?;
        Ok(Self {
            state,
            finished: false,
        })
    }

    fn finish(mut self) {
        self.finished = true;
        self.state.store(SESSION_ACTIVE, Ordering::Release);
    }
}

impl Drop for SessionRetirement<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.state.store(SESSION_RETIRING, Ordering::Release);
        }
    }
}

struct DecodeCommandBuffers {
    output: CommandBuffers,
    input: CommandBuffers,
}

impl DecodeCommandBuffers {
    fn new() -> Option<Self> {
        Some(Self {
            output: CommandBuffers::new()?,
            input: CommandBuffers::new()?,
        })
    }
}

struct VideoSession {
    stream_id: u32,
    reserved: AtomicBool,
    retirement: AtomicU8,
    operation: IrqSpinLock<()>,
    stream_created: IrqRwSpinLock<bool>,
    stream_coded_format: IrqRwSpinLock<u32>,
    mapped_frame: IrqSpinLock<Option<MappedFrameInfo>>,
    mapped_resources: IrqSpinLock<Option<MappedResourceSet>>,
    async_command_buffers: IrqSpinLock<Option<DecodeCommandBuffers>>,
    pending_decode: IrqSpinLock<Option<PendingDecode>>,
    next_timestamp: IrqSpinLock<u64>,
}

impl VideoSession {
    fn new(index: usize) -> Self {
        Self {
            stream_id: (index + 1) as u32,
            // Stream zero in the userspace ABI addresses this legacy slot.
            reserved: AtomicBool::new(index == 0),
            retirement: AtomicU8::new(SESSION_ACTIVE),
            operation: IrqSpinLock::new(()),
            stream_created: IrqRwSpinLock::new(false),
            stream_coded_format: IrqRwSpinLock::new(0),
            mapped_frame: IrqSpinLock::new(None),
            mapped_resources: IrqSpinLock::new(None),
            async_command_buffers: IrqSpinLock::new(DecodeCommandBuffers::new()),
            pending_decode: IrqSpinLock::new(None),
            next_timestamp: IrqSpinLock::new(1),
        }
    }
}

/// Prototype VirtIO video decode device.
pub struct VirtioVideoDevice {
    base_addr: usize,
    pci_transport: Option<VirtioPciTransport>,
    virtqueues: IrqSpinLock<[VirtQueue<'static>; QUEUE_COUNT]>,
    features: IrqRwSpinLock<u64>,
    input_capability_descs: IrqRwSpinLock<u32>,
    output_capability_descs: IrqRwSpinLock<u32>,
    sessions: [VideoSession; MAX_VIDEO_SESSIONS],
    decoded_frame: IrqSpinLock<DecodedFrameState>,
    sync_command_buffers: IrqSpinLock<Option<CommandBuffers>>,
    sync_command_state: IrqSpinLock<SyncCommandState>,
    next_session_index: IrqSpinLock<usize>,
    interrupt_id: IrqSpinLock<Option<InterruptId>>,
}

impl VirtioVideoDevice {
    /// Create a VirtIO video device backed by MMIO transport.
    ///
    /// # Arguments
    ///
    /// * `base_addr` - Mapped VirtIO MMIO base address.
    ///
    /// # Returns
    ///
    /// A new initialized VirtIO video prototype device.
    pub fn new(base_addr: usize) -> Self {
        Self::new_with_transport(base_addr, None)
    }

    /// Create a VirtIO video device backed by PCI transport.
    ///
    /// # Arguments
    ///
    /// * `transport` - Mapped VirtIO PCI transport regions.
    ///
    /// # Returns
    ///
    /// A new initialized VirtIO video prototype device.
    pub fn new_pci(transport: VirtioPciTransport) -> Self {
        Self::new_with_transport(transport.common_cfg, Some(transport))
    }

    fn new_with_transport(base_addr: usize, pci_transport: Option<VirtioPciTransport>) -> Self {
        let mut device = Self {
            base_addr,
            pci_transport,
            virtqueues: IrqSpinLock::new([VirtQueue::new(QUEUE_SIZE), VirtQueue::new(QUEUE_SIZE)]),
            features: IrqRwSpinLock::new(0),
            input_capability_descs: IrqRwSpinLock::new(0),
            output_capability_descs: IrqRwSpinLock::new(0),
            sessions: core::array::from_fn(VideoSession::new),
            decoded_frame: IrqSpinLock::new(DecodedFrameState {
                frame_count: 0,
                last_error: None,
            }),
            sync_command_buffers: IrqSpinLock::new(CommandBuffers::new()),
            sync_command_state: IrqSpinLock::new(SyncCommandState::default()),
            next_session_index: IrqSpinLock::new(0),
            interrupt_id: IrqSpinLock::new(None),
        };

        let negotiated_features = match device.init() {
            Ok(features) => features,
            Err(e) => {
                crate::println!("[virtio-video] Failed to initialize: {}", e);
                0
            }
        };
        *device.features.write() = negotiated_features;

        if let Err(e) = device.bootstrap_decoder() {
            crate::println!("[virtio-video] Decoder bootstrap skipped: {}", e);
        }

        crate::println!(
            "[virtio-video] Prototype device initialized, features=0x{:x}",
            negotiated_features
        );

        device
    }

    fn bootstrap_decoder(&self) -> Result<(), &'static str> {
        let features = *self.features.read();
        if features & (1u64 << VIRTIO_VIDEO_F_RESOURCE_GUEST_PAGES) == 0 {
            return Err("backend did not negotiate guest-page resources");
        }

        let input_descs = self.query_capability(VIRTIO_VIDEO_QUEUE_TYPE_INPUT)?;
        let output_descs = self.query_capability(VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT)?;
        *self.input_capability_descs.write() = input_descs;
        *self.output_capability_descs.write() = output_descs;

        let session = self.default_session();
        self.create_stream(session.stream_id, VIRTIO_VIDEO_FORMAT_H264)?;
        *session.stream_created.write() = true;
        *session.stream_coded_format.write() = VIRTIO_VIDEO_FORMAT_H264;

        crate::println!(
            "[virtio-video] H.264 decoder bootstrap ok: input_descs={} output_descs={}",
            input_descs,
            output_descs
        );
        Ok(())
    }

    fn query_capability(&self, queue_type: u32) -> Result<u32, &'static str> {
        let request = VirtioVideoQueryCapability {
            hdr: VirtioVideoCmdHdr {
                type_: VIRTIO_VIDEO_CMD_QUERY_CAPABILITY,
                stream_id: 0,
            },
            queue_type,
            padding: [0; 4],
        };
        let response = self.command_request(
            bytes_of(&request),
            core::mem::size_of::<VirtioVideoCmdHdr>() + 8,
        )?;
        let response_type = read_le32(&response, 0)?;
        if response_type != VIRTIO_VIDEO_RESP_OK_QUERY_CAPABILITY {
            return Err("virtio-video QUERY_CAPABILITY failed");
        }
        read_le32(&response, core::mem::size_of::<VirtioVideoCmdHdr>())
    }

    fn default_session(&self) -> &VideoSession {
        &self.sessions[0]
    }

    fn session_by_stream_id(&self, stream_id: u32) -> Result<&VideoSession, &'static str> {
        let stream_id = if stream_id == 0 {
            DEFAULT_STREAM_ID
        } else {
            stream_id
        };
        let index = stream_id
            .checked_sub(1)
            .ok_or("Invalid VirtIO video stream id")? as usize;
        self.sessions
            .get(index)
            .ok_or("Invalid VirtIO video stream id")
    }

    fn allocate_session(&self) -> Result<&VideoSession, &'static str> {
        let _ = self.try_complete_pending_decode();
        // Failed create/drop can lose its userspace owner while a command is
        // still device-owned. Once late completions return, retry its teardown
        // outside the queue lock before considering the reserved slot reusable.
        for session in &self.sessions {
            if session.retirement.load(Ordering::Acquire) == SESSION_RETIRING
                && session.pending_decode.lock().is_none()
            {
                let _ = self.release_session(session, true);
            }
        }

        let mut next = self.next_session_index.lock();
        for offset in 0..MAX_VIDEO_SESSIONS {
            let index = (*next + offset) % MAX_VIDEO_SESSIONS;
            let session = &self.sessions[index];
            if session.pending_decode.lock().is_none()
                && session
                    .reserved
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                *next = index.wrapping_add(1);
                return Ok(session);
            }
        }
        Err("No free VirtIO video stream sessions")
    }

    fn release_session(
        &self,
        session: &VideoSession,
        retry_only: bool,
    ) -> Result<(), &'static str> {
        let retirement = SessionRetirement::claim(&session.retirement, retry_only)?;
        let _operation = session.operation.lock();
        self.drain_session_pending_decode(session)?;

        self.resource_destroy_all(session.stream_id, VIRTIO_VIDEO_QUEUE_TYPE_INPUT)?;
        self.resource_destroy_all(session.stream_id, VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT)?;
        *session.mapped_resources.lock() = None;
        *session.mapped_frame.lock() = None;
        *session.stream_created.write() = false;
        *session.stream_coded_format.write() = 0;
        *session.next_timestamp.lock() = 1;
        retirement.finish();
        session
            .reserved
            .store(session.stream_id == DEFAULT_STREAM_ID, Ordering::Release);
        Ok(())
    }

    fn drain_session_pending_decode(&self, session: &VideoSession) -> Result<(), &'static str> {
        let mut spins = 0;
        loop {
            if session.pending_decode.lock().is_none() {
                return Ok(());
            }
            let made_progress = self.try_complete_pending_decode()?;
            if session.pending_decode.lock().is_none() {
                return Ok(());
            }
            if spins >= CONTROL_SPIN_LIMIT {
                return Err("VirtIO video pending decode timed out during destroy");
            }
            if !made_progress {
                spins += 1;
                core::hint::spin_loop();
            }
        }
    }

    fn create_stream(&self, stream_id: u32, coded_format: u32) -> Result<(), &'static str> {
        let mut tag = [0u8; 64];
        let name = match coded_format {
            VIRTIO_VIDEO_FORMAT_H264 => b"scarlet-videotoolbox-h264".as_slice(),
            VIRTIO_VIDEO_FORMAT_HEVC => b"scarlet-videotoolbox-hevc".as_slice(),
            VIRTIO_VIDEO_FORMAT_AV1 => b"scarlet-videotoolbox-av1".as_slice(),
            _ => return Err("Unsupported VirtIO video coded format"),
        };
        tag[..name.len()].copy_from_slice(name);

        let request = VirtioVideoStreamCreate {
            hdr: VirtioVideoCmdHdr {
                type_: VIRTIO_VIDEO_CMD_STREAM_CREATE,
                stream_id,
            },
            in_mem_type: VIRTIO_VIDEO_MEM_TYPE_GUEST_PAGES,
            out_mem_type: VIRTIO_VIDEO_MEM_TYPE_GUEST_PAGES,
            coded_format,
            padding: [0; 4],
            tag,
        };
        let response = self.command_request(
            bytes_of(&request),
            core::mem::size_of::<VirtioVideoCmdHdr>(),
        )?;
        let response_type = read_le32(&response, 0)?;
        if response_type != VIRTIO_VIDEO_RESP_OK_NODATA {
            return Err("virtio-video STREAM_CREATE failed");
        }
        Ok(())
    }

    fn ensure_stream_format(
        &self,
        session: &VideoSession,
        coded_format: u32,
    ) -> Result<(), &'static str> {
        if coded_format == 0 {
            return Err("VirtIO video coded format is missing");
        }
        if !matches!(
            coded_format,
            VIRTIO_VIDEO_FORMAT_H264 | VIRTIO_VIDEO_FORMAT_HEVC | VIRTIO_VIDEO_FORMAT_AV1
        ) {
            return Err("Unsupported VirtIO video coded format");
        }
        if *session.stream_created.read() && *session.stream_coded_format.read() == coded_format {
            return Ok(());
        }
        if session.pending_decode.lock().is_some() {
            return Err("VirtIO video decode already pending");
        }

        self.resource_destroy_all(session.stream_id, VIRTIO_VIDEO_QUEUE_TYPE_INPUT)?;
        self.resource_destroy_all(session.stream_id, VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT)?;
        *session.mapped_resources.lock() = None;
        self.create_stream(session.stream_id, coded_format)?;
        *session.stream_created.write() = true;
        *session.stream_coded_format.write() = coded_format;
        *session.next_timestamp.lock() = 1;
        Ok(())
    }

    fn decode_backend_access_unit(
        &self,
        request: &VideoBackendDecodeRequest,
    ) -> Result<(), &'static str> {
        let session = self.session_by_stream_id(request.stream_id)?;
        let _operation = session.operation.lock();
        if !session.reserved.load(Ordering::Acquire)
            || session.retirement.load(Ordering::Acquire) != SESSION_ACTIVE
        {
            return Err("VirtIO video stream session is retiring");
        }
        let _ = self.try_complete_pending_decode()?;
        if session.pending_decode.lock().is_some() {
            return Err("VirtIO video decode already pending");
        }
        self.ensure_stream_format(session, request.coded_format)?;
        if request.input_len == 0 {
            return Err("VirtIO video input is empty");
        }
        if (request.input_len as usize) > MAPPED_INPUT_BYTES {
            return Err("VirtIO video input exceeds mapped video input buffer");
        }
        if (request.output_len as usize) <= SCARLET_VIDEO_FRAME_HEADER_LEN {
            return Err("VirtIO video output buffer is too small");
        }
        if (request.output_len as usize) > MAPPED_OUTPUT_BYTES {
            return Err("VirtIO video output exceeds mapped video output buffer");
        }

        let buffer_lease = Arc::clone(
            request
                .buffer_lease
                .as_ref()
                .ok_or("VirtIO video mapped buffer owner is missing")?,
        );
        *session.mapped_frame.lock() = None;
        self.ensure_mapped_resources(session, request)?;

        let timestamp = if request.timestamp == 0 {
            self.next_timestamp(session)
        } else {
            request.timestamp
        };
        self.resource_queue_decode_pair_async(
            session,
            timestamp,
            request.input_len,
            PendingDecodeBuffer::ExternalMapped {
                output_paddr: request.output_paddr,
                buffer_lease,
            },
            request.output_len as usize,
            request.output_offset as usize,
        )
    }

    fn next_timestamp(&self, session: &VideoSession) -> u64 {
        let mut next_timestamp = session.next_timestamp.lock();
        let timestamp = *next_timestamp;
        *next_timestamp = next_timestamp.wrapping_add(1);
        timestamp
    }

    fn ensure_mapped_resources(
        &self,
        session: &VideoSession,
        request: &VideoBackendDecodeRequest,
    ) -> Result<(), &'static str> {
        let desired = MappedResourceSet {
            input_paddr: request.input_paddr as u64,
            input_len: MAPPED_INPUT_BYTES as u32,
            output_paddr: request.output_paddr as u64,
            output_len: request.output_len,
        };
        {
            let resources = session.mapped_resources.lock();
            if *resources == Some(desired) {
                return Ok(());
            }
        }
        if session.pending_decode.lock().is_some() {
            return Err("VirtIO video decode already pending");
        }

        self.resource_destroy_all(session.stream_id, VIRTIO_VIDEO_QUEUE_TYPE_INPUT)?;
        self.resource_destroy_all(session.stream_id, VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT)?;
        self.resource_create(
            session.stream_id,
            VIRTIO_VIDEO_QUEUE_TYPE_INPUT,
            INPUT_RESOURCE_ID,
            desired.input_paddr,
            desired.input_len,
        )?;
        self.resource_create(
            session.stream_id,
            VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT,
            OUTPUT_RESOURCE_ID,
            desired.output_paddr,
            desired.output_len,
        )?;
        *session.mapped_resources.lock() = Some(desired);
        Ok(())
    }

    fn resource_create(
        &self,
        stream_id: u32,
        queue_type: u32,
        resource_id: u32,
        paddr: u64,
        length: u32,
    ) -> Result<(), &'static str> {
        let mut request = Vec::new();
        push_le32(&mut request, VIRTIO_VIDEO_CMD_RESOURCE_CREATE);
        push_le32(&mut request, stream_id);
        push_le32(&mut request, queue_type);
        push_le32(&mut request, resource_id);
        push_le32(&mut request, VIRTIO_VIDEO_PLANES_LAYOUT_SINGLE_BUFFER);
        push_le32(&mut request, 1);
        for _ in 0..8 {
            push_le32(&mut request, 0);
        }
        push_le32(&mut request, 1);
        for _ in 1..8 {
            push_le32(&mut request, 0);
        }
        push_le64(&mut request, paddr);
        push_le32(&mut request, length);
        push_le32(&mut request, 0);

        let response = self.command_request(&request, core::mem::size_of::<VirtioVideoCmdHdr>())?;
        if read_le32(&response, 0)? != VIRTIO_VIDEO_RESP_OK_NODATA {
            return Err("virtio-video RESOURCE_CREATE failed");
        }
        Ok(())
    }

    fn resource_destroy_all(&self, stream_id: u32, queue_type: u32) -> Result<(), &'static str> {
        let mut request = Vec::new();
        push_le32(&mut request, VIRTIO_VIDEO_CMD_RESOURCE_DESTROY_ALL);
        push_le32(&mut request, stream_id);
        push_le32(&mut request, queue_type);
        push_le32(&mut request, 0);

        let response = self.command_request(&request, core::mem::size_of::<VirtioVideoCmdHdr>())?;
        if read_le32(&response, 0)? != VIRTIO_VIDEO_RESP_OK_NODATA {
            return Err("virtio-video RESOURCE_DESTROY_ALL failed");
        }
        Ok(())
    }

    fn resource_queue_request(
        &self,
        stream_id: u32,
        queue_type: u32,
        resource_id: u32,
        timestamp: u64,
        data_size: u32,
    ) -> Vec<u8> {
        let mut request = Vec::new();
        push_le32(&mut request, VIRTIO_VIDEO_CMD_RESOURCE_QUEUE);
        push_le32(&mut request, stream_id);
        push_le32(&mut request, queue_type);
        push_le32(&mut request, resource_id);
        push_le64(&mut request, timestamp);
        push_le32(&mut request, 1);
        push_le32(&mut request, data_size);
        for _ in 1..8 {
            push_le32(&mut request, 0);
        }
        push_le32(&mut request, 0);
        request
    }

    fn resource_queue_decode_pair_async(
        &self,
        session: &VideoSession,
        timestamp: u64,
        input_len: u32,
        buffer: PendingDecodeBuffer,
        output_len: usize,
        output_offset: usize,
    ) -> Result<(), &'static str> {
        let output_request = self.resource_queue_request(
            session.stream_id,
            VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT,
            OUTPUT_RESOURCE_ID,
            timestamp,
            0,
        );
        let input_request = self.resource_queue_request(
            session.stream_id,
            VIRTIO_VIDEO_QUEUE_TYPE_INPUT,
            INPUT_RESOURCE_ID,
            timestamp,
            input_len,
        );
        if output_request.len() > PAGE_SIZE || input_request.len() > PAGE_SIZE {
            return Err("VirtIO video async command message too large");
        }

        let mut virtqueues = self.virtqueues.lock();
        let queue = &mut virtqueues[QUEUE_COMMAND];
        let mut pending_guard = session.pending_decode.lock();
        if pending_guard.is_some() {
            return Err("VirtIO video decode already pending");
        }
        let async_buffers = session.async_command_buffers.lock();
        let command_buffers = async_buffers
            .as_ref()
            .ok_or("VirtIO video async command buffers are not available")?;

        // SAFETY: request slices and the allocated command pages are valid,
        // non-overlapping buffers. The queue/pending/buffer lock order matches
        // completion handling, so the device cannot observe a partially
        // initialized decode pair.
        unsafe {
            core::ptr::copy_nonoverlapping(
                output_request.as_ptr(),
                command_buffers.output.req_alloc.as_ptr() as *mut u8,
                output_request.len(),
            );
            core::ptr::copy_nonoverlapping(
                input_request.as_ptr(),
                command_buffers.input.req_alloc.as_ptr() as *mut u8,
                input_request.len(),
            );
        }

        let output_req_desc = self.prepare_command_descriptors(
            queue,
            &command_buffers.output,
            output_request.len(),
            24,
        )?;
        let input_req_desc = match self.prepare_command_descriptors(
            queue,
            &command_buffers.input,
            input_request.len(),
            24,
        ) {
            Ok(desc) => desc,
            Err(e) => {
                queue.free_desc_chain(output_req_desc);
                return Err(e);
            }
        };
        *pending_guard = Some(PendingDecode {
            output_req_desc: Some(output_req_desc),
            input_req_desc,
            input_done: false,
            buffer,
            output_len,
            output_offset,
            timestamp,
        });
        // push_many validates the entire batch before publishing avail.idx,
        // so an error here leaves neither descriptor owned by the device.
        if let Err(error) = queue.push_many(&[output_req_desc, input_req_desc]) {
            *pending_guard = None;
            queue.free_desc_chain(input_req_desc);
            queue.free_desc_chain(output_req_desc);
            return Err(error);
        }
        drop(async_buffers);
        drop(pending_guard);
        drop(virtqueues);
        self.notify(QUEUE_COMMAND);
        Ok(())
    }

    fn prepare_command_descriptors(
        &self,
        queue: &mut VirtQueue<'static>,
        command_buffers: &CommandBuffers,
        req_len: usize,
        response_len: u32,
    ) -> Result<usize, &'static str> {
        let req_desc = queue.alloc_desc().ok_or("No video request descriptor")?;
        let resp_desc = match queue.alloc_desc() {
            Some(desc) => desc,
            None => {
                queue.free_desc(req_desc);
                return Err("No video response descriptor");
            }
        };

        queue.desc[req_desc].addr = command_buffers.req_alloc.as_paddr() as u64;
        queue.desc[req_desc].len = req_len as u32;
        queue.desc[req_desc].flags = DescriptorFlag::Next as u16;
        queue.desc[req_desc].next = resp_desc as u16;

        queue.desc[resp_desc].addr = command_buffers.resp_alloc.as_paddr() as u64;
        queue.desc[resp_desc].len = response_len;
        queue.desc[resp_desc].flags = DescriptorFlag::Write as u16;
        queue.desc[resp_desc].next = 0;

        Ok(req_desc)
    }

    fn command_request(
        &self,
        request: &[u8],
        response_len: usize,
    ) -> Result<alloc::vec::Vec<u8>, &'static str> {
        let req_len = request.len();
        if req_len > PAGE_SIZE || response_len > PAGE_SIZE {
            return Err("VirtIO video command message too large");
        }

        let sync_buffers = self.sync_command_buffers.lock();
        let command_buffers = sync_buffers
            .as_ref()
            .ok_or("VirtIO video command buffers are not available")?;
        if self.sync_command_state.lock().pending_desc.is_some() {
            // A prior timeout is not cancellation. Its chain and request page
            // remain owned by the device until a used entry returns them.
            return Err("VirtIO video previous command is still pending");
        }
        // SAFETY: `request` and the allocated page are both valid for `req_len`
        // bytes, non-overlapping, and the destination page is writable.
        unsafe {
            core::ptr::copy_nonoverlapping(
                request.as_ptr(),
                command_buffers.req_alloc.as_ptr() as *mut u8,
                req_len,
            );
        }

        let mut virtqueues = self.virtqueues.lock();
        let queue = &mut virtqueues[QUEUE_COMMAND];
        let req_desc = queue.alloc_desc().ok_or("No video request descriptor")?;
        let resp_desc = match queue.alloc_desc() {
            Some(desc) => desc,
            None => {
                queue.free_desc(req_desc);
                return Err("No video response descriptor");
            }
        };

        queue.desc[req_desc].addr = command_buffers.req_alloc.as_paddr() as u64;
        queue.desc[req_desc].len = req_len as u32;
        queue.desc[req_desc].flags = DescriptorFlag::Next as u16;
        queue.desc[req_desc].next = resp_desc as u16;

        queue.desc[resp_desc].addr = command_buffers.resp_alloc.as_paddr() as u64;
        queue.desc[resp_desc].len = response_len as u32;
        queue.desc[resp_desc].flags = DescriptorFlag::Write as u16;
        queue.desc[resp_desc].next = 0;

        if let Err(error) = self.sync_command_state.lock().begin(req_desc) {
            queue.free_desc_chain(req_desc);
            return Err(error);
        }
        if let Err(e) = queue.push(req_desc) {
            // Failed publication leaves the chain exclusively CPU-owned.
            *self.sync_command_state.lock() = SyncCommandState::default();
            queue.free_desc_chain(req_desc);
            return Err(e);
        }
        drop(virtqueues);
        self.notify(QUEUE_COMMAND);

        let mut virtqueues = self.virtqueues.lock();
        let queue = &mut virtqueues[QUEUE_COMMAND];
        let mut spins = 0;
        loop {
            let mut made_progress = false;
            while let Some((used_desc, _used_len)) = queue.pop_used() {
                made_progress = true;
                if !self.complete_pending_used_descriptor(queue, used_desc)? {
                    // Do not free our still-published command on an unrelated
                    // protocol failure; the device can still read its pages.
                    return Err("VirtIO video command response descriptor mismatch");
                }
            }

            if self.sync_command_state.lock().completed {
                drop(virtqueues);
                let mut out = alloc::vec![0u8; response_len];
                // SAFETY: the used entry returned the command pages; the
                // sync-buffer lock prevents a new request from overwriting them.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        command_buffers.resp_alloc.as_ptr() as *const u8,
                        out.as_mut_ptr(),
                        response_len,
                    );
                }
                return Ok(out);
            }
            if spins >= CONTROL_SPIN_LIMIT {
                return Err("VirtIO video command timed out");
            }
            if !made_progress {
                spins += 1;
                core::hint::spin_loop();
            }
        }
    }

    fn try_complete_pending_decode(&self) -> Result<bool, &'static str> {
        let mut virtqueues = self.virtqueues.lock();
        let queue = &mut virtqueues[QUEUE_COMMAND];
        let mut completed_any = false;
        while let Some((used_desc, _used_len)) = queue.pop_used() {
            if !self.complete_pending_used_descriptor(queue, used_desc)? {
                // An unknown used entry does not establish our ownership of
                // that chain; reclaiming it can free another in-flight request.
                return Err("VirtIO video async response descriptor mismatch");
            }
            completed_any = true;
        }
        Ok(completed_any)
    }

    fn complete_pending_used_descriptor(
        &self,
        queue: &mut VirtQueue<'static>,
        used_desc: usize,
    ) -> Result<bool, &'static str> {
        if self
            .sync_command_state
            .lock()
            .complete_descriptor(used_desc)
        {
            queue.free_desc_chain(used_desc);
            return Ok(true);
        }
        for session in &self.sessions {
            let mut pending_guard = session.pending_decode.lock();
            let Some(pending) = pending_guard.as_mut() else {
                continue;
            };
            let Some(complete) = pending.complete_descriptor(used_desc) else {
                continue;
            };
            queue.free_desc_chain(used_desc);

            if complete {
                let response = self.copy_async_response(session)?;
                let pending = pending_guard
                    .take()
                    .ok_or("VirtIO video pending state missing")?;
                drop(pending_guard);
                self.finish_pending_decode(session, pending, &response)?;
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn copy_async_response(
        &self,
        session: &VideoSession,
    ) -> Result<alloc::vec::Vec<u8>, &'static str> {
        let async_buffers = session.async_command_buffers.lock();
        let command_buffers = async_buffers
            .as_ref()
            .ok_or("VirtIO video async command buffers are not available")?;
        let mut response = alloc::vec![0u8; 24];
        // SAFETY: `resp_alloc` points to a live page and `response` is valid
        // for 24 bytes. The buffers do not overlap.
        unsafe {
            core::ptr::copy_nonoverlapping(
                command_buffers.input.resp_alloc.as_ptr() as *const u8,
                response.as_mut_ptr(),
                response.len(),
            );
        }
        Ok(response)
    }

    fn finish_pending_decode(
        &self,
        session: &VideoSession,
        pending: PendingDecode,
        response: &[u8],
    ) -> Result<(), &'static str> {
        if read_le32(&response, 4)? != session.stream_id {
            return Err("VirtIO video async response stream mismatch");
        }
        if read_le32(&response, 0)? != VIRTIO_VIDEO_RESP_OK_RESOURCE_QUEUE {
            return Err("virtio-video async RESOURCE_QUEUE failed");
        }
        let decoded_size = read_le32(&response, 20)? as usize;
        if decoded_size == 0 || decoded_size > pending.output_len {
            return Err("VirtIO video backend returned invalid frame size");
        }

        match pending.buffer {
            PendingDecodeBuffer::ExternalMapped {
                output_paddr,
                buffer_lease,
            } => {
                let frame = Self::read_external_frame_header(
                    output_paddr,
                    decoded_size,
                    pending.output_len,
                    pending.output_offset,
                    pending.timestamp,
                )?;
                // Both descriptor chains have returned; the CPU header read
                // above is the final access requiring the mapped allocation.
                drop(buffer_lease);
                *session.mapped_frame.lock() = Some(frame);

                let mut state = self.decoded_frame.lock();
                state.frame_count = state.frame_count.wrapping_add(1);
                state.last_error = None;
            }
        }
        Ok(())
    }

    fn read_external_frame_header(
        output_paddr: u64,
        decoded_size: usize,
        output_len: usize,
        output_offset: usize,
        timestamp: u64,
    ) -> Result<MappedFrameInfo, &'static str> {
        let output_vaddr = phys_to_virt(output_paddr);
        // SAFETY: The caller retains the mapped allocation lease through this
        // read after both device descriptors completed. Its size was checked against
        // `output_len` by the caller.
        let frame = unsafe { core::slice::from_raw_parts(output_vaddr as *const u8, decoded_size) };
        Self::parse_mapped_frame_header(frame, decoded_size, output_len, output_offset, timestamp)
    }

    fn parse_mapped_frame_header(
        frame: &[u8],
        decoded_size: usize,
        output_len: usize,
        output_offset: usize,
        timestamp: u64,
    ) -> Result<MappedFrameInfo, &'static str> {
        if decoded_size < SCARLET_VIDEO_FRAME_HEADER_LEN {
            return Err("VirtIO video backend returned truncated mapped frame");
        }
        if frame.get(0..4) != Some(SCARLET_VIDEO_FRAME_MAGIC.as_slice()) {
            return Err("VirtIO video backend returned invalid mapped frame magic");
        }
        let width = read_le32(frame, 4)?;
        let height = read_le32(frame, 8)?;
        let pixel_format = read_le32(frame, 12)?;
        let payload_len = read_le32(frame, 16)?;
        if width == 0 || height == 0 || payload_len == 0 {
            return Err("VirtIO video backend returned empty mapped frame");
        }
        let total_len = SCARLET_VIDEO_FRAME_HEADER_LEN
            .checked_add(payload_len as usize)
            .ok_or("VirtIO video mapped frame length overflow")?;
        if total_len > decoded_size || total_len > output_len {
            return Err("VirtIO video backend returned invalid mapped frame length");
        }
        Ok(MappedFrameInfo {
            width,
            height,
            pixel_format,
            payload_offset: (output_offset + SCARLET_VIDEO_FRAME_HEADER_LEN) as u64,
            payload_len,
            timestamp,
        })
    }

    /// Enable interrupt-driven completion for the VirtIO video device.
    ///
    /// # Arguments
    ///
    /// * `interrupt_id` - Platform interrupt line registered for this PCI device.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the interrupt line was recorded.
    pub fn enable_interrupts(&self, interrupt_id: InterruptId) -> Result<(), &'static str> {
        *self.interrupt_id.lock() = Some(interrupt_id);
        Ok(())
    }
}

const fn align_up_const(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

fn bytes_of<T: Copy>(value: &T) -> &[u8] {
    // SAFETY: `value` is valid for `size_of::<T>()` bytes and the returned
    // slice is tied to the lifetime of `value`.
    unsafe {
        core::slice::from_raw_parts(value as *const T as *const u8, core::mem::size_of::<T>())
    }
}

fn push_le32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn push_le64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn read_le32(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
    let end = offset
        .checked_add(4)
        .ok_or("VirtIO video response overflow")?;
    let value = bytes
        .get(offset..end)
        .ok_or("VirtIO video response too short")?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

impl VideoDecodeBackend for VirtioVideoDevice {
    fn name(&self) -> &'static str {
        "virtio-video"
    }

    fn capabilities(&self) -> VideoBackendCapabilities {
        VideoBackendCapabilities {
            max_sessions: MAX_VIDEO_SESSIONS as u32,
            max_inflight_decodes: MAX_VIDEO_SESSIONS as u32,
            mapped_input_len: MAPPED_INPUT_BYTES as u32,
            mapped_output_len: MAPPED_OUTPUT_BYTES as u32,
            output_pixel_format: SCARLET_VIDEO_PIXEL_FORMAT_NV12,
            supports_h264: true,
            supports_av1: true,
            supports_hevc: true,
            supports_stateless_h264: false,
        }
    }

    fn create_session(&self, coded_format: u32) -> Result<u32, &'static str> {
        let session = self.allocate_session()?;
        let result = {
            let _operation = session.operation.lock();
            self.ensure_stream_format(session, coded_format)
        };
        if let Err(e) = result {
            let _ = self.release_session(session, false);
            return Err(e);
        }
        Ok(session.stream_id)
    }

    fn destroy_session(&self, stream_id: u32) -> Result<(), &'static str> {
        let session = self.session_by_stream_id(stream_id)?;
        if let Err(error) = self.release_session(session, false) {
            self.decoded_frame.lock().last_error = Some(error);
            return Err(error);
        }
        Ok(())
    }

    fn submit_decode(&self, request: &VideoBackendDecodeRequest) -> Result<(), &'static str> {
        match self.decode_backend_access_unit(request) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.decoded_frame.lock().last_error = Some(e);
                Err(e)
            }
        }
    }

    fn dequeue_frame(
        &self,
        stream_id: u32,
    ) -> Result<Option<VideoBackendDecodedFrame>, &'static str> {
        if let Err(e) = self.try_complete_pending_decode() {
            self.decoded_frame.lock().last_error = Some(e);
            return Err(e);
        }
        let session = self.session_by_stream_id(stream_id)?;
        let Some(frame) = session.mapped_frame.lock().take() else {
            return Ok(None);
        };
        Ok(Some(VideoBackendDecodedFrame {
            image: None,
            stream_id: session.stream_id,
            frame: ScarletVideoDequeuedFrame {
                width: frame.width,
                height: frame.height,
                pixel_format: frame.pixel_format,
                payload_offset: frame.payload_offset,
                payload_len: frame.payload_len,
                flags: 0,
                timestamp: frame.timestamp,
            },
        }))
    }
}

impl crate::device::events::InterruptCapableDevice for VirtioVideoDevice {
    fn handle_interrupt(&self) -> crate::interrupt::InterruptResult<()> {
        let _ = self.claim_interrupt()?;
        Ok(())
    }

    fn interrupt_id(&self) -> Option<InterruptId> {
        *self.interrupt_id.lock()
    }

    fn claim_interrupt(&self) -> crate::interrupt::InterruptResult<InterruptClaim> {
        let isr_status = self.read32_register(Register::InterruptStatus);
        if isr_status == 0 {
            return Ok(InterruptClaim::NotMine);
        }

        self.write32_register(Register::InterruptAck, isr_status);
        if let Err(e) = self.try_complete_pending_decode() {
            self.decoded_frame.lock().last_error = Some(e);
        }
        Ok(InterruptClaim::Handled)
    }
}

impl VirtioDevice for VirtioVideoDevice {
    fn pci_transport(&self) -> Option<VirtioPciTransport> {
        self.pci_transport
    }

    fn get_base_addr(&self) -> usize {
        self.base_addr
    }

    fn get_virtqueue_count(&self) -> usize {
        QUEUE_COUNT
    }

    fn get_virtqueue_size(&self, queue_idx: usize) -> usize {
        if queue_idx >= QUEUE_COUNT {
            panic!("Invalid queue index for VirtIO video device: {}", queue_idx);
        }
        let virtqueues = self.virtqueues.lock();
        virtqueues[queue_idx].get_queue_size()
    }

    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= QUEUE_COUNT {
            return None;
        }
        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].get_raw_ptr() as usize) as u64)
    }

    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= QUEUE_COUNT {
            return None;
        }
        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].avail.flags as *const _ as usize) as u64)
    }

    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= QUEUE_COUNT {
            return None;
        }
        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].used.flags as *const _ as usize) as u64)
    }

    fn get_supported_features(&self, device_features: u64) -> u64 {
        device_features
            & ((1u64 << VIRTIO_F_VERSION_1) | (1u64 << VIRTIO_VIDEO_F_RESOURCE_GUEST_PAGES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verify_pending_buffer_lifetime(input_first: bool) {
        let pages = Arc::new(ContiguousPages::new(1).unwrap());
        let allocation = Arc::downgrade(&pages);
        // A 2x2 NV12 frame: 20-byte header followed by 6 bytes of pixels.
        let mut frame = [0u8; SCARLET_VIDEO_FRAME_HEADER_LEN + 6];
        frame[..4].copy_from_slice(SCARLET_VIDEO_FRAME_MAGIC);
        frame[4..8].copy_from_slice(&2u32.to_le_bytes());
        frame[8..12].copy_from_slice(&2u32.to_le_bytes());
        frame[12..16].copy_from_slice(&SCARLET_VIDEO_PIXEL_FORMAT_NV12.to_le_bytes());
        frame[16..20].copy_from_slice(&6u32.to_le_bytes());
        // SAFETY: We exclusively own a live page and the frame fits in it.
        unsafe {
            core::ptr::copy_nonoverlapping(frame.as_ptr(), pages.as_ptr() as *mut u8, frame.len());
        }
        let mut pending = PendingDecode {
            output_req_desc: Some(2),
            input_req_desc: 4,
            input_done: false,
            buffer: PendingDecodeBuffer::ExternalMapped {
                output_paddr: pages.as_paddr(),
                buffer_lease: Arc::clone(&pages),
            },
            output_len: PAGE_SIZE,
            output_offset: 0,
            timestamp: 11,
        };
        drop(pages); // Frontend closes while the device still owns both chains.
        assert_eq!(allocation.strong_count(), 1);
        assert_eq!(pending.complete_descriptor(99), None);
        let (first, second) = if input_first { (4, 2) } else { (2, 4) };
        assert_eq!(pending.complete_descriptor(first), Some(false));
        assert_eq!(pending.complete_descriptor(first), None);
        assert!(allocation.upgrade().is_some()); // One returned chain is insufficient.
        assert_eq!(pending.complete_descriptor(second), Some(true));
        assert!(allocation.upgrade().is_some()); // CPU header read still needs the page.
        let PendingDecodeBuffer::ExternalMapped { output_paddr, .. } = &pending.buffer;
        let decoded = VirtioVideoDevice::read_external_frame_header(
            *output_paddr,
            frame.len(),
            pending.output_len,
            pending.output_offset,
            pending.timestamp,
        )
        .unwrap();
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.payload_len, 6);
        assert_eq!(decoded.timestamp, 11);
        drop(pending);
        assert!(allocation.upgrade().is_none()); // Normal completion does not leak.
    }

    #[test_case]
    fn sync_command_completion_can_be_claimed_by_another_used_ring_consumer() {
        let mut sync = SyncCommandState::default();
        sync.begin(6).unwrap();
        assert!(!sync.complete_descriptor(2)); // Another stream's output.
        assert_eq!(sync.pending_desc, Some(6));
        assert!(sync.complete_descriptor(6)); // IRQ/dequeue beats sync waiter.
        assert!(sync.completed);
        assert_eq!(sync.pending_desc, None);
        assert!(!sync.complete_descriptor(6)); // Never free twice.
        sync.begin(8).unwrap();
        assert!(!sync.completed);
        assert!(sync.complete_descriptor(8));
    }

    #[test_case]
    fn timed_out_sync_command_cannot_reuse_the_device_owned_chain_or_pages() {
        let mut sync = SyncCommandState::default();
        sync.begin(6).unwrap();
        // A timeout leaves this ownership state intact. New commands cannot
        // overwrite the shared request page, even if a different head is free.
        assert!(sync.begin(8).is_err());
        assert_eq!(sync.pending_desc, Some(6));
        assert!(!sync.completed);
        assert!(sync.complete_descriptor(6)); // Late used entry releases it.
        sync.begin(8).unwrap();
        assert!(sync.complete_descriptor(8));
    }

    #[test_case]
    fn failed_session_retirement_has_one_retry_owner_until_acknowledged() {
        let state = AtomicU8::new(SESSION_ACTIVE);
        assert!(SessionRetirement::claim(&state, true).is_err());
        let first = SessionRetirement::claim(&state, false).unwrap();
        assert!(SessionRetirement::claim(&state, false).is_err());
        assert!(SessionRetirement::claim(&state, true).is_err());
        drop(first); // Timeout: retain the slot and request a later retry.
        assert_eq!(state.load(Ordering::Acquire), SESSION_RETIRING);
        let retry = SessionRetirement::claim(&state, true).unwrap();
        assert!(SessionRetirement::claim(&state, true).is_err());
        retry.finish();
        assert_eq!(state.load(Ordering::Acquire), SESSION_ACTIVE);
        // A stale reaper must not clean up a newly allocated active owner.
        assert!(SessionRetirement::claim(&state, true).is_err());
    }

    #[test_case]
    fn allocated_session_is_reserved_before_stream_create_completes() {
        let session = VideoSession::new(1);
        assert!(!*session.stream_created.read());
        assert!(
            session
                .reserved
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        );
        // A second opener must not select this still-uncreated stream.
        assert!(
            session
                .reserved
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        );
        session.reserved.store(false, Ordering::Release);
        assert!(
            session
                .reserved
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        );
        assert!(VideoSession::new(0).reserved.load(Ordering::Acquire));
    }

    #[test_case]
    fn pending_decode_retains_pages_when_output_completes_first() {
        verify_pending_buffer_lifetime(false);
    }

    #[test_case]
    fn pending_decode_retains_pages_when_input_completes_first() {
        verify_pending_buffer_lifetime(true);
    }
}
