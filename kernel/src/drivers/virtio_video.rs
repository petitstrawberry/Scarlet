//! VirtIO video decode backend.
//!
//! The userspace ABI is provided by the shared `/dev/videoN` frontend in
//! `device::video`. This module owns the VirtIO transport, stream setup, and
//! asynchronous RESOURCE_QUEUE completion handling.

extern crate alloc;

use crate::sync::{IrqRwSpinLock, IrqSpinLock};
use alloc::vec::Vec;

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
    ExternalMapped { output_paddr: usize },
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
            next_session_index: IrqSpinLock::new(0),
            interrupt_id: IrqSpinLock::new(None),
        };

        let negotiated_features = match device.init() {
            Ok(features) => features,
            Err(e) => {
                crate::early_println!("[virtio-video] Failed to initialize: {}", e);
                0
            }
        };
        *device.features.write() = negotiated_features;

        if let Err(e) = device.bootstrap_decoder() {
            crate::early_println!("[virtio-video] Decoder bootstrap skipped: {}", e);
        }

        crate::early_println!(
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

        crate::early_println!(
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

        let mut next = self.next_session_index.lock();
        for offset in 0..MAX_VIDEO_SESSIONS {
            let index = (*next + offset) % MAX_VIDEO_SESSIONS;
            let session = &self.sessions[index];
            if !*session.stream_created.read() && session.pending_decode.lock().is_none() {
                *next = index.wrapping_add(1);
                return Ok(session);
            }
        }
        Err("No free VirtIO video stream sessions")
    }

    fn release_session(&self, session: &VideoSession) -> Result<(), &'static str> {
        if session.pending_decode.lock().is_some() {
            return Err("VirtIO video stream session has pending decode");
        }

        let _ = self.resource_destroy_all(session.stream_id, VIRTIO_VIDEO_QUEUE_TYPE_INPUT);
        let _ = self.resource_destroy_all(session.stream_id, VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT);
        *session.mapped_resources.lock() = None;
        *session.mapped_frame.lock() = None;
        *session.stream_created.write() = false;
        *session.stream_coded_format.write() = 0;
        *session.next_timestamp.lock() = 1;
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

        if let Err(e) = queue.push(req_desc) {
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
                if used_desc == req_desc {
                    queue.free_desc_chain(req_desc);
                    drop(virtqueues);

                    let mut out = alloc::vec![0u8; response_len];
                    // SAFETY: `resp_alloc` points to a live page and `out` is valid for
                    // `response_len` bytes. The buffers do not overlap.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            command_buffers.resp_alloc.as_ptr() as *const u8,
                            out.as_mut_ptr(),
                            response_len,
                        );
                    }
                    return Ok(out);
                }
                if !self.complete_pending_used_descriptor(queue, used_desc)? {
                    queue.free_desc_chain(used_desc);
                    queue.free_desc_chain(req_desc);
                    return Err("VirtIO video command response descriptor mismatch");
                }
            }

            if spins >= CONTROL_SPIN_LIMIT {
                queue.free_desc_chain(req_desc);
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
                queue.free_desc_chain(used_desc);
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
        for session in &self.sessions {
            let mut pending_guard = session.pending_decode.lock();
            let Some(pending) = pending_guard.as_mut() else {
                continue;
            };
            if Some(used_desc) == pending.output_req_desc {
                queue.free_desc_chain(used_desc);
                pending.output_req_desc = None;
            } else if used_desc == pending.input_req_desc {
                queue.free_desc_chain(used_desc);
                pending.input_done = true;
            } else {
                continue;
            }

            if pending.output_req_desc.is_none() && pending.input_done {
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
        if read_le32(&response, 0)? != VIRTIO_VIDEO_RESP_OK_RESOURCE_QUEUE {
            return Err("virtio-video async RESOURCE_QUEUE failed");
        }
        let decoded_size = read_le32(&response, 20)? as usize;
        if decoded_size == 0 || decoded_size > pending.output_len {
            return Err("VirtIO video backend returned invalid frame size");
        }

        match pending.buffer {
            PendingDecodeBuffer::ExternalMapped { output_paddr } => {
                let frame = self.read_external_frame_header(
                    output_paddr,
                    decoded_size,
                    pending.output_len,
                    pending.output_offset,
                    pending.timestamp,
                )?;
                *session.mapped_frame.lock() = Some(frame);

                let mut state = self.decoded_frame.lock();
                state.frame_count = state.frame_count.wrapping_add(1);
                state.last_error = None;
            }
        }
        Ok(())
    }

    fn read_external_frame_header(
        &self,
        output_paddr: usize,
        decoded_size: usize,
        output_len: usize,
        output_offset: usize,
        timestamp: u64,
    ) -> Result<MappedFrameInfo, &'static str> {
        let output_vaddr = phys_to_virt(output_paddr);
        // SAFETY: The pending decode retained the output resource until
        // completion and the device-reported size was checked against
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
        if let Err(e) = self.ensure_stream_format(session, coded_format) {
            let _ = self.release_session(session);
            return Err(e);
        }
        Ok(session.stream_id)
    }

    fn destroy_session(&self, stream_id: u32) -> Result<(), &'static str> {
        let session = self.session_by_stream_id(stream_id)?;
        if let Err(e) = self.drain_session_pending_decode(session) {
            self.decoded_frame.lock().last_error = Some(e);
            return Err(e);
        }
        self.release_session(session)
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
