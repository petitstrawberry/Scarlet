//! VirtIO video decode prototype driver.
//!
//! The device currently exposes a small experimental character-device API:
//! writing H.264 Annex B bytes submits one decode job, and reading returns the
//! most recent decoded frame as `SVF1` followed by width, height, pixel format,
//! payload length, and NV12 bytes.

extern crate alloc;

use alloc::{format, string::String, vec::Vec};
use core::any::Any;

use spin::{Mutex, RwLock};

use crate::device::{Device, DeviceType, char::CharDevice};
use crate::drivers::virtio::device::Register;
use crate::drivers::virtio::features::VIRTIO_F_VERSION_1;
use crate::drivers::virtio::{
    device::VirtioDevice,
    pci::VirtioPciTransport,
    queue::{DescriptorFlag, VirtQueue},
};
use crate::environment::PAGE_SIZE;
use crate::interrupt::InterruptId;
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::mem::page::ContiguousPages;
use crate::object::capability::selectable::{ReadyInterest, SelectWaitOutcome, Selectable};
use crate::object::capability::{ControlOps, MemoryMappingOps};
use crate::task::mytask;
use crate::vm::addr::virt_to_phys;

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
const VIRTIO_VIDEO_FORMAT_H264: u32 = 4098;
const VIRTIO_VIDEO_FORMAT_AV1: u32 = 4103;
const VIRTIO_VIDEO_MEM_TYPE_GUEST_PAGES: u32 = 0;

const STREAM_ID: u32 = 1;
const INPUT_RESOURCE_ID: u32 = 1;
const OUTPUT_RESOURCE_ID: u32 = 2;
const MAX_DECODED_FRAME_BYTES: usize = 16 * 1024 * 1024;
const SCARLET_VIDEO_FRAME_HEADER_LEN: usize = 20;
const MAPPED_INPUT_BYTES: usize = 8 * 1024 * 1024;
const MAPPED_OUTPUT_OFFSET: usize = MAPPED_INPUT_BYTES;
const MAPPED_OUTPUT_BYTES: usize = align_up_const(
    MAX_DECODED_FRAME_BYTES + SCARLET_VIDEO_FRAME_HEADER_LEN,
    PAGE_SIZE,
);
const MAPPED_BUFFER_BYTES: usize = MAPPED_OUTPUT_OFFSET + MAPPED_OUTPUT_BYTES;
const MAPPED_BUFFER_PAGES: usize = MAPPED_BUFFER_BYTES / PAGE_SIZE;
const VVIDEO_GET_BUFFER: u32 = 0x5600;
const VVIDEO_SUBMIT: u32 = 0x5601;
const VVIDEO_DEQUEUE: u32 = 0x5602;

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

#[repr(C)]
#[derive(Clone, Copy)]
struct ScarletVideoBufferInfo {
    mmap_offset: u64,
    mmap_len: u64,
    input_offset: u64,
    input_len: u32,
    output_offset: u64,
    output_len: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ScarletVideoSubmit {
    input_len: u32,
    coded_format: u32,
    timestamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ScarletVideoDequeuedFrame {
    width: u32,
    height: u32,
    pixel_format: u32,
    payload_offset: u64,
    payload_len: u32,
    flags: u32,
    timestamp: u64,
}

struct DecodedFrameState {
    bytes: Vec<u8>,
    read_cursor: usize,
    frame_count: u64,
    last_error: Option<&'static str>,
}

#[derive(Clone, Copy)]
struct MappedFrameInfo {
    width: u32,
    height: u32,
    pixel_format: u32,
    payload_len: u32,
    timestamp: u64,
}

enum PendingDecodeBuffer {
    Owned {
        input: ContiguousPages,
        output: ContiguousPages,
    },
    Mapped,
}

struct PendingDecode {
    output_req_desc: Option<usize>,
    input_req_desc: usize,
    input_done: bool,
    buffer: PendingDecodeBuffer,
    output_len: usize,
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

/// Prototype VirtIO video decode device.
pub struct VirtioVideoDevice {
    base_addr: usize,
    pci_transport: Option<VirtioPciTransport>,
    virtqueues: Mutex<[VirtQueue<'static>; QUEUE_COUNT]>,
    features: RwLock<u64>,
    input_capability_descs: RwLock<u32>,
    output_capability_descs: RwLock<u32>,
    stream_created: RwLock<bool>,
    stream_coded_format: RwLock<u32>,
    decoded_frame: Mutex<DecodedFrameState>,
    mapped_buffer: RwLock<Option<ContiguousPages>>,
    mapped_frame: Mutex<Option<MappedFrameInfo>>,
    mapped_resources_created: Mutex<bool>,
    sync_command_buffers: Mutex<Option<CommandBuffers>>,
    async_command_buffers: Mutex<Option<DecodeCommandBuffers>>,
    next_timestamp: Mutex<u64>,
    pending_decode: Mutex<Option<PendingDecode>>,
    interrupt_id: Mutex<Option<InterruptId>>,
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
            virtqueues: Mutex::new([VirtQueue::new(QUEUE_SIZE), VirtQueue::new(QUEUE_SIZE)]),
            features: RwLock::new(0),
            input_capability_descs: RwLock::new(0),
            output_capability_descs: RwLock::new(0),
            stream_created: RwLock::new(false),
            stream_coded_format: RwLock::new(0),
            decoded_frame: Mutex::new(DecodedFrameState {
                bytes: Vec::new(),
                read_cursor: 0,
                frame_count: 0,
                last_error: None,
            }),
            mapped_buffer: RwLock::new(ContiguousPages::new(MAPPED_BUFFER_PAGES)),
            mapped_frame: Mutex::new(None),
            mapped_resources_created: Mutex::new(false),
            sync_command_buffers: Mutex::new(CommandBuffers::new()),
            async_command_buffers: Mutex::new(DecodeCommandBuffers::new()),
            next_timestamp: Mutex::new(1),
            pending_decode: Mutex::new(None),
            interrupt_id: Mutex::new(None),
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

        self.create_stream(STREAM_ID, VIRTIO_VIDEO_FORMAT_H264)?;
        *self.stream_created.write() = true;
        *self.stream_coded_format.write() = VIRTIO_VIDEO_FORMAT_H264;

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

    fn create_stream(&self, stream_id: u32, coded_format: u32) -> Result<(), &'static str> {
        let mut tag = [0u8; 64];
        let name = match coded_format {
            VIRTIO_VIDEO_FORMAT_H264 => b"scarlet-videotoolbox-h264".as_slice(),
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

    fn ensure_stream_format(&self, coded_format: u32) -> Result<(), &'static str> {
        if coded_format == 0 {
            return Err("VirtIO video coded format is missing");
        }
        if !matches!(coded_format, VIRTIO_VIDEO_FORMAT_H264 | VIRTIO_VIDEO_FORMAT_AV1) {
            return Err("Unsupported VirtIO video coded format");
        }
        if *self.stream_created.read() && *self.stream_coded_format.read() == coded_format {
            return Ok(());
        }

        self.invalidate_mapped_resources();
        self.resource_destroy_all(VIRTIO_VIDEO_QUEUE_TYPE_INPUT)?;
        self.resource_destroy_all(VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT)?;
        self.create_stream(STREAM_ID, coded_format)?;
        *self.stream_created.write() = true;
        *self.stream_coded_format.write() = coded_format;
        *self.next_timestamp.lock() = 1;
        Ok(())
    }

    fn decode_h264_access_unit(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        self.ensure_stream_format(VIRTIO_VIDEO_FORMAT_H264)?;
        if buffer.is_empty() {
            return Err("H.264 input is empty");
        }

        let input_pages = buffer.len().div_ceil(PAGE_SIZE);
        let output_len = MAX_DECODED_FRAME_BYTES
            .checked_add(SCARLET_VIDEO_FRAME_HEADER_LEN)
            .ok_or("Decoded frame buffer overflow")?;
        let output_pages = output_len.div_ceil(PAGE_SIZE);
        let input = ContiguousPages::new(input_pages).ok_or("Failed to allocate video input")?;
        let output = ContiguousPages::new(output_pages).ok_or("Failed to allocate video output")?;

        // SAFETY: `input` points to `input_pages` live pages and `buffer` is
        // valid for `buffer.len()` bytes. The buffers do not overlap.
        unsafe {
            core::ptr::copy_nonoverlapping(
                buffer.as_ptr(),
                input.as_ptr() as *mut u8,
                buffer.len(),
            );
        }

        self.invalidate_mapped_resources();
        self.resource_destroy_all(VIRTIO_VIDEO_QUEUE_TYPE_INPUT)?;
        self.resource_destroy_all(VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT)?;
        self.resource_create(
            VIRTIO_VIDEO_QUEUE_TYPE_INPUT,
            INPUT_RESOURCE_ID,
            input.as_paddr() as u64,
            buffer.len() as u32,
        )?;
        self.resource_create(
            VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT,
            OUTPUT_RESOURCE_ID,
            output.as_paddr() as u64,
            output_len as u32,
        )?;

        let timestamp = {
            let mut next_timestamp = self.next_timestamp.lock();
            let timestamp = *next_timestamp;
            *next_timestamp = next_timestamp.wrapping_add(1);
            timestamp
        };

        self.resource_queue(
            VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT,
            OUTPUT_RESOURCE_ID,
            timestamp,
            0,
        )?;
        self.resource_queue_input_async(
            VIRTIO_VIDEO_QUEUE_TYPE_INPUT,
            INPUT_RESOURCE_ID,
            timestamp,
            buffer.len() as u32,
            PendingDecodeBuffer::Owned { input, output },
            output_len,
            timestamp,
        )?;

        Ok(buffer.len())
    }

    fn decode_mapped_access_unit(
        &self,
        coded_format: u32,
        input_len: usize,
        timestamp: u64,
    ) -> Result<(), &'static str> {
        self.ensure_stream_format(coded_format)?;
        if input_len == 0 {
            return Err("VirtIO video input is empty");
        }
        if input_len > MAPPED_INPUT_BYTES {
            return Err("VirtIO video input exceeds mapped video input buffer");
        }

        let (input_paddr, output_paddr) = {
            let mapped_buffer = self.mapped_buffer.read();
            let buffer = mapped_buffer
                .as_ref()
                .ok_or("VirtIO video mmap buffer is not available")?;
            (
                buffer.as_paddr() as u64,
                (buffer.as_paddr() + MAPPED_OUTPUT_OFFSET) as u64,
            )
        };
        let timestamp = if timestamp == 0 {
            self.next_timestamp()
        } else {
            timestamp
        };

        *self.mapped_frame.lock() = None;
        self.ensure_mapped_resources(input_paddr, output_paddr)?;

        self.resource_queue_decode_pair_async(timestamp, input_len as u32)
    }

    fn next_timestamp(&self) -> u64 {
        let mut next_timestamp = self.next_timestamp.lock();
        let timestamp = *next_timestamp;
        *next_timestamp = next_timestamp.wrapping_add(1);
        timestamp
    }

    fn ensure_mapped_resources(
        &self,
        input_paddr: u64,
        output_paddr: u64,
    ) -> Result<(), &'static str> {
        let mut created = self.mapped_resources_created.lock();
        if *created {
            return Ok(());
        }

        self.resource_destroy_all(VIRTIO_VIDEO_QUEUE_TYPE_INPUT)?;
        self.resource_destroy_all(VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT)?;
        self.resource_create(
            VIRTIO_VIDEO_QUEUE_TYPE_INPUT,
            INPUT_RESOURCE_ID,
            input_paddr,
            MAPPED_INPUT_BYTES as u32,
        )?;
        self.resource_create(
            VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT,
            OUTPUT_RESOURCE_ID,
            output_paddr,
            MAPPED_OUTPUT_BYTES as u32,
        )?;
        *created = true;
        Ok(())
    }

    fn invalidate_mapped_resources(&self) {
        *self.mapped_resources_created.lock() = false;
    }

    fn resource_create(
        &self,
        queue_type: u32,
        resource_id: u32,
        paddr: u64,
        length: u32,
    ) -> Result<(), &'static str> {
        let mut request = Vec::new();
        push_le32(&mut request, VIRTIO_VIDEO_CMD_RESOURCE_CREATE);
        push_le32(&mut request, STREAM_ID);
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

    fn resource_queue(
        &self,
        queue_type: u32,
        resource_id: u32,
        timestamp: u64,
        data_size: u32,
    ) -> Result<usize, &'static str> {
        let mut request = Vec::new();
        push_le32(&mut request, VIRTIO_VIDEO_CMD_RESOURCE_QUEUE);
        push_le32(&mut request, STREAM_ID);
        push_le32(&mut request, queue_type);
        push_le32(&mut request, resource_id);
        push_le64(&mut request, timestamp);
        push_le32(&mut request, 1);
        push_le32(&mut request, data_size);
        for _ in 1..8 {
            push_le32(&mut request, 0);
        }
        push_le32(&mut request, 0);

        let response = self.command_request(&request, 24)?;
        if read_le32(&response, 0)? != VIRTIO_VIDEO_RESP_OK_RESOURCE_QUEUE {
            return Err("virtio-video RESOURCE_QUEUE failed");
        }
        Ok(read_le32(&response, 20)? as usize)
    }

    fn resource_destroy_all(&self, queue_type: u32) -> Result<(), &'static str> {
        let mut request = Vec::new();
        push_le32(&mut request, VIRTIO_VIDEO_CMD_RESOURCE_DESTROY_ALL);
        push_le32(&mut request, STREAM_ID);
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
        queue_type: u32,
        resource_id: u32,
        timestamp: u64,
        data_size: u32,
    ) -> Vec<u8> {
        let mut request = Vec::new();
        push_le32(&mut request, VIRTIO_VIDEO_CMD_RESOURCE_QUEUE);
        push_le32(&mut request, STREAM_ID);
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

    fn resource_queue_input_async(
        &self,
        queue_type: u32,
        resource_id: u32,
        timestamp: u64,
        data_size: u32,
        buffer: PendingDecodeBuffer,
        output_len: usize,
        decode_timestamp: u64,
    ) -> Result<(), &'static str> {
        if self.pending_decode.lock().is_some() {
            return Err("VirtIO video decode already pending");
        }

        let request = self.resource_queue_request(queue_type, resource_id, timestamp, data_size);
        let req_len = request.len();
        if req_len > PAGE_SIZE {
            return Err("VirtIO video async command message too large");
        }

        let async_buffers = self.async_command_buffers.lock();
        let command_buffers = async_buffers
            .as_ref()
            .ok_or("VirtIO video async command buffers are not available")?;
        let command_buffers = &command_buffers.input;
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
        queue.desc[resp_desc].len = 24;
        queue.desc[resp_desc].flags = DescriptorFlag::Write as u16;
        queue.desc[resp_desc].next = 0;

        if let Err(e) = queue.push(req_desc) {
            queue.free_desc_chain(req_desc);
            return Err(e);
        }
        drop(virtqueues);
        drop(async_buffers);

        *self.pending_decode.lock() = Some(PendingDecode {
            output_req_desc: None,
            input_req_desc: req_desc,
            input_done: false,
            buffer,
            output_len,
            timestamp: decode_timestamp,
        });
        self.notify(QUEUE_COMMAND);
        Ok(())
    }

    fn resource_queue_decode_pair_async(
        &self,
        timestamp: u64,
        input_len: u32,
    ) -> Result<(), &'static str> {
        if self.pending_decode.lock().is_some() {
            return Err("VirtIO video decode already pending");
        }

        let output_request = self.resource_queue_request(
            VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT,
            OUTPUT_RESOURCE_ID,
            timestamp,
            0,
        );
        let input_request = self.resource_queue_request(
            VIRTIO_VIDEO_QUEUE_TYPE_INPUT,
            INPUT_RESOURCE_ID,
            timestamp,
            input_len,
        );
        if output_request.len() > PAGE_SIZE || input_request.len() > PAGE_SIZE {
            return Err("VirtIO video async command message too large");
        }

        let async_buffers = self.async_command_buffers.lock();
        let command_buffers = async_buffers
            .as_ref()
            .ok_or("VirtIO video async command buffers are not available")?;

        // SAFETY: request slices and the allocated command pages are valid,
        // non-overlapping buffers.
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

        let mut virtqueues = self.virtqueues.lock();
        let queue = &mut virtqueues[QUEUE_COMMAND];
        let output_req_desc = self.queue_command_descriptors(
            queue,
            &command_buffers.output,
            output_request.len(),
            24,
        )?;
        let input_req_desc = match self.queue_command_descriptors(
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
        drop(virtqueues);
        drop(async_buffers);

        *self.pending_decode.lock() = Some(PendingDecode {
            output_req_desc: Some(output_req_desc),
            input_req_desc,
            input_done: false,
            buffer: PendingDecodeBuffer::Mapped,
            output_len: MAPPED_OUTPUT_BYTES,
            timestamp,
        });
        self.notify(QUEUE_COMMAND);
        Ok(())
    }

    fn queue_command_descriptors(
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

        if let Err(e) = queue.push(req_desc) {
            queue.free_desc_chain(req_desc);
            return Err(e);
        }
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
        while queue.is_busy() {
            if spins >= CONTROL_SPIN_LIMIT {
                queue.free_desc_chain(req_desc);
                return Err("VirtIO video command timed out");
            }
            spins += 1;
            core::hint::spin_loop();
        }

        let Some((used_desc, _used_len)) = queue.pop_used() else {
            queue.free_desc_chain(req_desc);
            return Err("VirtIO video command response missing");
        };
        if used_desc != req_desc {
            queue.free_desc_chain(used_desc);
            queue.free_desc_chain(req_desc);
            return Err("VirtIO video command response descriptor mismatch");
        }
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
        Ok(out)
    }

    fn read_stream(&self, buffer: &mut [u8]) -> usize {
        if let Err(e) = self.try_complete_pending_decode() {
            self.decoded_frame.lock().last_error = Some(e);
        }

        let mut state = self.decoded_frame.lock();
        if state.read_cursor < state.bytes.len() {
            let count = core::cmp::min(buffer.len(), state.bytes.len() - state.read_cursor);
            let start = state.read_cursor;
            buffer[..count].copy_from_slice(&state.bytes[start..start + count]);
            state.read_cursor += count;
            return count;
        }
        if !state.bytes.is_empty() {
            state.bytes.clear();
            state.read_cursor = 0;
        }
        if self.pending_decode.lock().is_some() {
            return 0;
        }

        let frame_summary = frame_header_summary(&state.bytes).unwrap_or_default();
        let last_error = state.last_error.unwrap_or("none");
        let status = format!(
            "virtio-video decoder features=0x{:x} input_caps={} output_caps={} stream_created={} coded_format={} frames={} last_error={}{}\n",
            *self.features.read(),
            *self.input_capability_descs.read(),
            *self.output_capability_descs.read(),
            *self.stream_created.read(),
            *self.stream_coded_format.read(),
            state.frame_count,
            last_error,
            frame_summary
        );
        let bytes = status.as_bytes();
        let count = core::cmp::min(buffer.len(), bytes.len());
        buffer[..count].copy_from_slice(&bytes[..count]);
        count
    }

    fn try_complete_pending_decode(&self) -> Result<bool, &'static str> {
        let mut pending_guard = self.pending_decode.lock();
        if pending_guard.is_none() {
            return Ok(false);
        }

        let mut pending = pending_guard
            .take()
            .ok_or("VirtIO video pending state missing")?;
        let mut virtqueues = self.virtqueues.lock();
        let queue = &mut virtqueues[QUEUE_COMMAND];
        while let Some((used_desc, _used_len)) = queue.pop_used() {
            if Some(used_desc) == pending.output_req_desc {
                queue.free_desc_chain(used_desc);
                pending.output_req_desc = None;
                continue;
            }
            if used_desc == pending.input_req_desc {
                queue.free_desc_chain(used_desc);
                pending.input_done = true;
                continue;
            }

            queue.free_desc_chain(used_desc);
            if let Some(output_req_desc) = pending.output_req_desc {
                queue.free_desc_chain(output_req_desc);
            }
            if !pending.input_done {
                queue.free_desc_chain(pending.input_req_desc);
            }
            return Err("VirtIO video async response descriptor mismatch");
        }
        drop(virtqueues);
        if pending.output_req_desc.is_some() || !pending.input_done {
            *pending_guard = Some(pending);
            return Ok(false);
        }

        let async_buffers = self.async_command_buffers.lock();
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
        drop(async_buffers);
        if read_le32(&response, 0)? != VIRTIO_VIDEO_RESP_OK_RESOURCE_QUEUE {
            return Err("virtio-video async RESOURCE_QUEUE failed");
        }
        let decoded_size = read_le32(&response, 20)? as usize;
        if decoded_size == 0 || decoded_size > pending.output_len {
            return Err("VirtIO video backend returned invalid frame size");
        }

        match pending.buffer {
            PendingDecodeBuffer::Owned { input, output } => {
                let _input_paddr = input.as_paddr();
                let mut frame = Vec::new();
                frame.resize(decoded_size, 0);
                // SAFETY: `output` points to live pages retained by `pending` and
                // `frame` is valid for `decoded_size` bytes. The buffers do not overlap.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        output.as_ptr() as *const u8,
                        frame.as_mut_ptr(),
                        decoded_size,
                    );
                }

                let mut state = self.decoded_frame.lock();
                state.bytes = frame;
                state.read_cursor = 0;
                state.frame_count = state.frame_count.wrapping_add(1);
                state.last_error = None;
            }
            PendingDecodeBuffer::Mapped => {
                let frame = self.read_mapped_frame_header(decoded_size, pending.timestamp)?;
                *self.mapped_frame.lock() = Some(frame);

                let mut state = self.decoded_frame.lock();
                state.bytes.clear();
                state.read_cursor = 0;
                state.frame_count = state.frame_count.wrapping_add(1);
                state.last_error = None;
            }
        }
        Ok(true)
    }

    fn read_mapped_frame_header(
        &self,
        decoded_size: usize,
        timestamp: u64,
    ) -> Result<MappedFrameInfo, &'static str> {
        if decoded_size < SCARLET_VIDEO_FRAME_HEADER_LEN {
            return Err("VirtIO video backend returned truncated mapped frame");
        }
        let mapped_buffer = self.mapped_buffer.read();
        let buffer = mapped_buffer
            .as_ref()
            .ok_or("VirtIO video mmap buffer is not available")?;
        // SAFETY: `buffer` owns `MAPPED_BUFFER_BYTES` bytes and
        // `MAPPED_OUTPUT_OFFSET..MAPPED_OUTPUT_OFFSET + decoded_size` has been
        // validated to fit inside the mapped output region by the caller.
        let frame = unsafe {
            core::slice::from_raw_parts(
                (buffer.as_ptr() as *const u8).add(MAPPED_OUTPUT_OFFSET),
                decoded_size,
            )
        };
        if frame.get(0..4) != Some(b"SVF1") {
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
        if total_len > decoded_size || total_len > MAPPED_OUTPUT_BYTES {
            return Err("VirtIO video backend returned invalid mapped frame length");
        }
        Ok(MappedFrameInfo {
            width,
            height,
            pixel_format,
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

    fn handle_get_buffer(&self, arg: usize) -> Result<i32, &'static str> {
        if self.mapped_buffer.read().is_none() {
            return Err("VirtIO video mmap buffer is not available");
        }
        let info = ScarletVideoBufferInfo {
            mmap_offset: 0,
            mmap_len: MAPPED_BUFFER_BYTES as u64,
            input_offset: 0,
            input_len: MAPPED_INPUT_BYTES as u32,
            output_offset: MAPPED_OUTPUT_OFFSET as u64,
            output_len: MAPPED_OUTPUT_BYTES as u32,
        };
        write_user_value(arg, &info)?;
        Ok(0)
    }

    fn handle_submit(&self, arg: usize) -> Result<i32, &'static str> {
        let submit: ScarletVideoSubmit = read_user_value(arg)?;
        if let Err(e) = self.try_complete_pending_decode() {
            self.decoded_frame.lock().last_error = Some(e);
            return Err(e);
        }
        match self.decode_mapped_access_unit(
            submit.coded_format,
            submit.input_len as usize,
            submit.timestamp,
        ) {
            Ok(()) => Ok(0),
            Err(e) => {
                self.decoded_frame.lock().last_error = Some(e);
                Err(e)
            }
        }
    }

    fn handle_dequeue(&self, arg: usize) -> Result<i32, &'static str> {
        if let Err(e) = self.try_complete_pending_decode() {
            self.decoded_frame.lock().last_error = Some(e);
            return Err(e);
        }
        let Some(frame) = self.mapped_frame.lock().take() else {
            return Ok(0);
        };
        let dequeued = ScarletVideoDequeuedFrame {
            width: frame.width,
            height: frame.height,
            pixel_format: frame.pixel_format,
            payload_offset: (MAPPED_OUTPUT_OFFSET + SCARLET_VIDEO_FRAME_HEADER_LEN) as u64,
            payload_len: frame.payload_len,
            flags: 0,
            timestamp: frame.timestamp,
        };
        write_user_value(arg, &dequeued)?;
        Ok(1)
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

fn read_user_value<T: Copy>(ptr: usize) -> Result<T, &'static str> {
    if ptr == 0 {
        return Err("VirtIO video ioctl pointer is null");
    }
    let task = mytask().ok_or("No current task for VirtIO video ioctl")?;
    let mut value = core::mem::MaybeUninit::<T>::uninit();
    // SAFETY: `value` is uninitialized storage for `T`; the byte slice covers
    // exactly that storage and is filled before `assume_init`.
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(value.as_mut_ptr() as *mut u8, core::mem::size_of::<T>())
    };
    copy_from_user(task, ptr, bytes).map_err(|_| "Failed to copy VirtIO video ioctl from user")?;
    // SAFETY: `copy_from_user` has initialized every byte of `value`.
    Ok(unsafe { value.assume_init() })
}

fn write_user_value<T: Copy>(ptr: usize, value: &T) -> Result<(), &'static str> {
    if ptr == 0 {
        return Err("VirtIO video ioctl pointer is null");
    }
    let task = mytask().ok_or("No current task for VirtIO video ioctl")?;
    // SAFETY: `value` is valid for `size_of::<T>()` bytes and is only read.
    let bytes = unsafe {
        core::slice::from_raw_parts(value as *const T as *const u8, core::mem::size_of::<T>())
    };
    copy_to_user(task, ptr, bytes).map_err(|_| "Failed to copy VirtIO video ioctl to user")
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

fn frame_header_summary(frame: &[u8]) -> Option<String> {
    if frame.len() < SCARLET_VIDEO_FRAME_HEADER_LEN || frame.get(0..4) != Some(b"SVF1") {
        return None;
    }
    let width = read_le32(frame, 4).ok()?;
    let height = read_le32(frame, 8).ok()?;
    let format = read_le32(frame, 12).ok()?;
    let length = read_le32(frame, 16).ok()?;
    Some(format!(
        " last_frame={}x{} format=0x{:08x} payload={} total={}",
        width,
        height,
        format,
        length,
        frame.len()
    ))
}

impl Device for VirtioVideoDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "virtio-video"
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

impl CharDevice for VirtioVideoDevice {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn write_byte(&self, _byte: u8) -> Result<(), &'static str> {
        Err("Write a complete H.264 access unit with write()")
    }

    fn read(&self, buffer: &mut [u8]) -> usize {
        self.read_stream(buffer)
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        match self.decode_h264_access_unit(buffer) {
            Ok(bytes) => Ok(bytes),
            Err(e) => {
                self.decoded_frame.lock().last_error = Some(e);
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
        Ok(self.read_stream(buffer))
    }
}

impl ControlOps for VirtioVideoDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        match command {
            VVIDEO_GET_BUFFER => self.handle_get_buffer(arg),
            VVIDEO_SUBMIT => self.handle_submit(arg),
            VVIDEO_DEQUEUE => self.handle_dequeue(arg),
            _ => Err("Unsupported VirtIO video control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        alloc::vec![
            (VVIDEO_GET_BUFFER, "Get mmap video buffer layout"),
            (VVIDEO_SUBMIT, "Submit mmap-written coded video access unit"),
            (VVIDEO_DEQUEUE, "Dequeue a decoded mmap video frame"),
        ]
    }
}

impl MemoryMappingOps for VirtioVideoDevice {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        if offset % PAGE_SIZE != 0 || length % PAGE_SIZE != 0 {
            return Err("VirtIO video mmap offset and length must be page-aligned");
        }
        if offset >= MAPPED_BUFFER_BYTES {
            return Err("VirtIO video mmap offset exceeds buffer size");
        }
        if length > MAPPED_BUFFER_BYTES - offset {
            return Err("VirtIO video mmap length exceeds buffer size");
        }
        let mapped_buffer = self.mapped_buffer.read();
        let buffer = mapped_buffer
            .as_ref()
            .ok_or("VirtIO video mmap buffer is not available")?;
        Ok((buffer.as_paddr() + offset, 0x3, true))
    }

    fn supports_mmap(&self) -> bool {
        self.mapped_buffer.read().is_some()
    }

    fn mmap_owner_name(&self) -> String {
        String::from("virtio-video")
    }
}

impl Selectable for VirtioVideoDevice {
    fn wait_until_ready(
        &self,
        _interest: ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        SelectWaitOutcome::Ready
    }
}

impl crate::device::events::InterruptCapableDevice for VirtioVideoDevice {
    fn handle_interrupt(&self) -> crate::interrupt::InterruptResult<()> {
        let isr_status = self.read32_register(Register::InterruptStatus);
        if isr_status == 0 {
            return Ok(());
        }

        self.write32_register(Register::InterruptAck, isr_status);
        if let Err(e) = self.try_complete_pending_decode() {
            self.decoded_frame.lock().last_error = Some(e);
        }
        Ok(())
    }

    fn interrupt_id(&self) -> Option<InterruptId> {
        *self.interrupt_id.lock()
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
