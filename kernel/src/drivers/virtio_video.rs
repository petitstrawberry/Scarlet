//! VirtIO video decode prototype driver.
//!
//! This driver is intentionally small: it exists to prove that Scarlet can
//! discover and initialize a custom vhost-user-backed VirtIO PCI device before
//! the real VideoToolbox command protocol is implemented.

extern crate alloc;

use alloc::format;
use core::any::Any;

use spin::{Mutex, RwLock};

use crate::device::{Device, DeviceType, char::CharDevice};
use crate::drivers::virtio::features::VIRTIO_F_VERSION_1;
use crate::drivers::virtio::{
    device::VirtioDevice,
    pci::VirtioPciTransport,
    queue::{DescriptorFlag, VirtQueue},
};
use crate::environment::PAGE_SIZE;
use crate::mem::page::ContiguousPages;
use crate::object::capability::selectable::{ReadyInterest, SelectWaitOutcome, Selectable};
use crate::object::capability::{ControlOps, MemoryMappingOps};
use crate::vm::addr::virt_to_phys;

const QUEUE_COMMAND: usize = 0;
const QUEUE_COUNT: usize = 2;
const QUEUE_SIZE: usize = 256;
const CONTROL_SPIN_LIMIT: usize = 1_000_000;

const VIRTIO_VIDEO_F_RESOURCE_GUEST_PAGES: u32 = 0;

const VIRTIO_VIDEO_CMD_QUERY_CAPABILITY: u32 = 256;
const VIRTIO_VIDEO_CMD_STREAM_CREATE: u32 = 257;
const VIRTIO_VIDEO_RESP_OK_NODATA: u32 = 512;
const VIRTIO_VIDEO_RESP_OK_QUERY_CAPABILITY: u32 = 513;
const VIRTIO_VIDEO_QUEUE_TYPE_INPUT: u32 = 256;
const VIRTIO_VIDEO_QUEUE_TYPE_OUTPUT: u32 = 257;
const VIRTIO_VIDEO_FORMAT_H264: u32 = 4098;
const VIRTIO_VIDEO_MEM_TYPE_GUEST_PAGES: u32 = 0;

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

/// Prototype VirtIO video decode device.
pub struct VirtioVideoDevice {
    base_addr: usize,
    pci_transport: Option<VirtioPciTransport>,
    virtqueues: Mutex<[VirtQueue<'static>; QUEUE_COUNT]>,
    features: RwLock<u64>,
    input_capability_descs: RwLock<u32>,
    output_capability_descs: RwLock<u32>,
    stream_created: RwLock<bool>,
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

        self.create_h264_stream(1)?;
        *self.stream_created.write() = true;

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
        let response =
            self.command_request(&request, core::mem::size_of::<VirtioVideoCmdHdr>() + 8)?;
        let response_type = read_le32(&response, 0)?;
        if response_type != VIRTIO_VIDEO_RESP_OK_QUERY_CAPABILITY {
            return Err("virtio-video QUERY_CAPABILITY failed");
        }
        read_le32(&response, core::mem::size_of::<VirtioVideoCmdHdr>())
    }

    fn create_h264_stream(&self, stream_id: u32) -> Result<(), &'static str> {
        let mut tag = [0u8; 64];
        let name = b"scarlet-videotoolbox-h264";
        tag[..name.len()].copy_from_slice(name);

        let request = VirtioVideoStreamCreate {
            hdr: VirtioVideoCmdHdr {
                type_: VIRTIO_VIDEO_CMD_STREAM_CREATE,
                stream_id,
            },
            in_mem_type: VIRTIO_VIDEO_MEM_TYPE_GUEST_PAGES,
            out_mem_type: VIRTIO_VIDEO_MEM_TYPE_GUEST_PAGES,
            coded_format: VIRTIO_VIDEO_FORMAT_H264,
            padding: [0; 4],
            tag,
        };
        let response = self.command_request(&request, core::mem::size_of::<VirtioVideoCmdHdr>())?;
        let response_type = read_le32(&response, 0)?;
        if response_type != VIRTIO_VIDEO_RESP_OK_NODATA {
            return Err("virtio-video STREAM_CREATE failed");
        }
        Ok(())
    }

    fn command_request<T: Copy>(
        &self,
        request: &T,
        response_len: usize,
    ) -> Result<alloc::vec::Vec<u8>, &'static str> {
        let req_len = core::mem::size_of::<T>();
        if req_len > PAGE_SIZE || response_len > PAGE_SIZE {
            return Err("VirtIO video command message too large");
        }

        let req_alloc = ContiguousPages::new(1).ok_or("Failed to allocate video request")?;
        let resp_alloc = ContiguousPages::new(1).ok_or("Failed to allocate video response")?;
        // SAFETY: `request` and the allocated page are both valid for `req_len`
        // bytes, non-overlapping, and the destination page is writable.
        unsafe {
            core::ptr::copy_nonoverlapping(
                request as *const T as *const u8,
                req_alloc.as_ptr() as *mut u8,
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

        queue.desc[req_desc].addr = req_alloc.as_paddr() as u64;
        queue.desc[req_desc].len = req_len as u32;
        queue.desc[req_desc].flags = DescriptorFlag::Next as u16;
        queue.desc[req_desc].next = resp_desc as u16;

        queue.desc[resp_desc].addr = resp_alloc.as_paddr() as u64;
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
                resp_alloc.as_ptr() as *const u8,
                out.as_mut_ptr(),
                response_len,
            );
        }
        Ok(out)
    }
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
        Err("VirtIO video prototype command protocol is not implemented")
    }

    fn read(&self, buffer: &mut [u8]) -> usize {
        self.read_at(0, buffer).unwrap_or(0)
    }

    fn write(&self, _buffer: &[u8]) -> Result<usize, &'static str> {
        Err("VirtIO video prototype command protocol is not implemented")
    }

    fn can_read(&self) -> bool {
        true
    }

    fn can_write(&self) -> bool {
        false
    }

    fn read_at(&self, position: u64, buffer: &mut [u8]) -> Result<usize, &'static str> {
        let status = format!(
            "virtio-video decoder features=0x{:x} input_caps={} output_caps={} stream_created={}\n",
            *self.features.read(),
            *self.input_capability_descs.read(),
            *self.output_capability_descs.read(),
            *self.stream_created.read()
        );
        let bytes = status.as_bytes();
        let start = usize::try_from(position).map_err(|_| "Read position overflow")?;
        if start >= bytes.len() {
            return Ok(0);
        }

        let count = core::cmp::min(buffer.len(), bytes.len() - start);
        buffer[..count].copy_from_slice(&bytes[start..start + count]);
        Ok(count)
    }
}

impl ControlOps for VirtioVideoDevice {}

impl MemoryMappingOps for VirtioVideoDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("VirtIO video prototype does not expose mmap regions")
    }

    fn supports_mmap(&self) -> bool {
        false
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
