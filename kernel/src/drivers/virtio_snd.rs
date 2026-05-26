//! VirtIO sound playback driver.
//!
//! This driver implements a small playback-only subset of the VirtIO sound
//! specification. The native Scarlet audio layer provides the mmap ring buffer;
//! this driver consumes complete periods and sends them to the device tx queue.

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, RwLock};

use crate::device::audio::{
    AUDIO_PCM_FORMAT_S16LE, AudioPcmCapabilities, AudioPcmParams, AudioPlaybackDevice,
    register_playback_device,
};
use crate::device::{Device, DeviceType};
use crate::drivers::virtio::features::{
    VIRTIO_F_VERSION_1, VIRTIO_RING_F_EVENT_IDX, VIRTIO_RING_F_INDIRECT_DESC,
};
use crate::drivers::virtio::{
    device::VirtioDevice,
    pci::VirtioPciTransport,
    queue::{DescriptorFlag, VirtQueue},
};
use crate::environment::PAGE_SIZE;
use crate::mem::page::ContiguousPages;
use crate::object::capability::selectable::Selectable;
use crate::object::capability::{ControlOps, MemoryMappingOps};
use crate::vm::addr::virt_to_phys;

const QUEUE_CONTROL: usize = 0;
const QUEUE_EVENT: usize = 1;
const QUEUE_TX: usize = 2;
const QUEUE_RX: usize = 3;
const QUEUE_CONTROL_SIZE: usize = 8;
const QUEUE_EVENT_SIZE: usize = 8;
const QUEUE_TX_SIZE: usize = 32;
const QUEUE_RX_SIZE: usize = 8;
const TX_DESCRIPTORS_PER_PERIOD: usize = 3;
const TX_DRAIN_SPIN_LIMIT: usize = 1_000_000;

const VIRTIO_SND_R_PCM_INFO: u32 = 0x0100;
const VIRTIO_SND_R_PCM_SET_PARAMS: u32 = 0x0101;
const VIRTIO_SND_R_PCM_PREPARE: u32 = 0x0102;
const VIRTIO_SND_R_PCM_RELEASE: u32 = 0x0103;
const VIRTIO_SND_R_PCM_START: u32 = 0x0104;
const VIRTIO_SND_R_PCM_STOP: u32 = 0x0105;

const VIRTIO_SND_S_OK: u32 = 0x8000;
const VIRTIO_SND_S_IO_ERR: u32 = 0x8003;

const VIRTIO_SND_D_OUTPUT: u8 = 0;
const VIRTIO_SND_PCM_FMT_S16: u8 = 5;
const VIRTIO_SND_PCM_RATE_48000: u8 = 7;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndHdr {
    code: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndQueryInfo {
    hdr: VirtioSndHdr,
    start_id: u32,
    count: u32,
    size: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndPcmHdr {
    hdr: VirtioSndHdr,
    stream_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndPcmSetParams {
    hdr: VirtioSndPcmHdr,
    buffer_bytes: u32,
    period_bytes: u32,
    features: u32,
    channels: u8,
    format: u8,
    rate: u8,
    padding: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndInfo {
    hda_fn_nid: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndPcmInfo {
    hdr: VirtioSndInfo,
    features: u32,
    formats: u64,
    rates: u64,
    direction: u8,
    channels_min: u8,
    channels_max: u8,
    padding: [u8; 5],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndPcmXfer {
    stream_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioSndPcmStatus {
    status: u32,
    latency_bytes: u32,
}

struct TxPeriod {
    desc_idx: usize,
    header: ContiguousPages,
    data: ContiguousPages,
    status: ContiguousPages,
}

/// VirtIO sound device.
pub struct VirtioSndDevice {
    base_addr: usize,
    pci_transport: Option<VirtioPciTransport>,
    virtqueues: Mutex<[VirtQueue<'static>; QUEUE_RX + 1]>,
    features: RwLock<u64>,
    streams: RwLock<u32>,
    stream_id: RwLock<Option<u32>>,
    capabilities: RwLock<AudioPcmCapabilities>,
    configured_periods: RwLock<usize>,
    in_flight: Mutex<Vec<TxPeriod>>,
    event_buffers: Mutex<Vec<(usize, ContiguousPages)>>,
}

impl VirtioSndDevice {
    /// Create a VirtIO sound device backed by MMIO transport.
    ///
    /// # Arguments
    ///
    /// * `base_addr` - Mapped VirtIO MMIO base address.
    ///
    /// # Returns
    ///
    /// A new initialized VirtIO sound device.
    pub fn new(base_addr: usize) -> Self {
        Self::new_with_transport(base_addr, None)
    }

    /// Create a VirtIO sound device backed by PCI transport.
    ///
    /// # Arguments
    ///
    /// * `transport` - Mapped VirtIO PCI transport regions.
    ///
    /// # Returns
    ///
    /// A new initialized VirtIO sound device.
    pub fn new_pci(transport: VirtioPciTransport) -> Self {
        Self::new_with_transport(transport.common_cfg, Some(transport))
    }

    fn new_with_transport(base_addr: usize, pci_transport: Option<VirtioPciTransport>) -> Self {
        let mut device = Self {
            base_addr,
            pci_transport,
            virtqueues: Mutex::new([
                VirtQueue::new(QUEUE_CONTROL_SIZE),
                VirtQueue::new(QUEUE_EVENT_SIZE),
                VirtQueue::new(QUEUE_TX_SIZE),
                VirtQueue::new(QUEUE_RX_SIZE),
            ]),
            features: RwLock::new(0),
            streams: RwLock::new(0),
            stream_id: RwLock::new(None),
            capabilities: RwLock::new(default_capabilities()),
            configured_periods: RwLock::new(4),
            in_flight: Mutex::new(Vec::new()),
            event_buffers: Mutex::new(Vec::new()),
        };

        match device.init() {
            Ok(features) => {
                *device.features.write() = features;
                device.populate_event_queue();
                device.discover_streams();
            }
            Err(e) => {
                crate::println!("[virtio-snd] initialization failed: {}", e);
            }
        }

        device
    }

    fn populate_event_queue(&self) {
        let mut virtqueues = self.virtqueues.lock();
        let queue = &mut virtqueues[QUEUE_EVENT];
        let mut buffers = self.event_buffers.lock();
        for _ in 0..4 {
            let Some(event) = ContiguousPages::new(1) else {
                break;
            };
            let Some(desc_idx) = queue.alloc_desc() else {
                break;
            };
            queue.desc[desc_idx].addr = event.as_paddr() as u64;
            queue.desc[desc_idx].len = core::mem::size_of::<VirtioSndHdr>() as u32 + 4;
            queue.desc[desc_idx].flags = DescriptorFlag::Write as u16;
            queue.desc[desc_idx].next = 0;
            if queue.push(desc_idx).is_ok() {
                buffers.push((desc_idx, event));
            } else {
                queue.free_desc(desc_idx);
                break;
            }
        }
        drop(virtqueues);
        self.notify(QUEUE_EVENT);
    }

    fn discover_streams(&self) {
        let streams = self.read_config::<u32>(4);
        *self.streams.write() = streams;
        if streams == 0 {
            crate::println!("[virtio-snd] no PCM streams reported");
            return;
        }

        for stream_id in 0..streams {
            match self.query_pcm_info(stream_id) {
                Ok(info) if info.direction == VIRTIO_SND_D_OUTPUT => {
                    *self.stream_id.write() = Some(stream_id);
                    *self.capabilities.write() = capabilities_from_pcm_info(&info);
                    crate::println!(
                        "[virtio-snd] selected playback stream {} channels {}..{}",
                        stream_id,
                        info.channels_min,
                        info.channels_max
                    );
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    crate::println!("[virtio-snd] failed to query stream {}: {}", stream_id, e);
                }
            }
        }

        crate::println!("[virtio-snd] no output PCM stream found");
    }

    fn query_pcm_info(&self, stream_id: u32) -> Result<VirtioSndPcmInfo, &'static str> {
        let req = VirtioSndQueryInfo {
            hdr: VirtioSndHdr {
                code: VIRTIO_SND_R_PCM_INFO,
            },
            start_id: stream_id,
            count: 1,
            size: core::mem::size_of::<VirtioSndPcmInfo>() as u32,
        };
        let response_len =
            core::mem::size_of::<VirtioSndHdr>() + core::mem::size_of::<VirtioSndPcmInfo>();
        let response = self.control_request_bytes(&req, response_len)?;
        let status =
            read_unaligned::<VirtioSndHdr>(&response[..core::mem::size_of::<VirtioSndHdr>()]);
        if status.code != VIRTIO_SND_S_OK {
            return Err("VirtIO sound PCM info request failed");
        }
        Ok(read_unaligned::<VirtioSndPcmInfo>(
            &response[core::mem::size_of::<VirtioSndHdr>()..],
        ))
    }

    fn pcm_command(&self, code: u32) -> Result<(), &'static str> {
        let stream_id = (*self.stream_id.read()).ok_or("VirtIO sound has no playback stream")?;
        let req = VirtioSndPcmHdr {
            hdr: VirtioSndHdr { code },
            stream_id,
        };
        let status: VirtioSndHdr = self.control_request(&req)?;
        if status.code == VIRTIO_SND_S_OK {
            Ok(())
        } else {
            Err("VirtIO sound PCM command failed")
        }
    }

    fn control_request<T: Copy, R: Copy>(&self, request: &T) -> Result<R, &'static str> {
        let bytes = self.control_request_bytes(request, core::mem::size_of::<R>())?;
        Ok(read_unaligned::<R>(&bytes))
    }

    fn control_request_bytes<T: Copy>(
        &self,
        request: &T,
        response_len: usize,
    ) -> Result<Vec<u8>, &'static str> {
        let req_len = core::mem::size_of::<T>();
        if req_len > PAGE_SIZE || response_len > PAGE_SIZE {
            return Err("VirtIO sound control message too large");
        }

        let req_alloc = ContiguousPages::new(1).ok_or("Failed to allocate control request")?;
        let resp_alloc = ContiguousPages::new(1).ok_or("Failed to allocate control response")?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                request as *const T as *const u8,
                req_alloc.as_ptr() as *mut u8,
                req_len,
            );
        }

        let mut virtqueues = self.virtqueues.lock();
        let queue = &mut virtqueues[QUEUE_CONTROL];
        let req_desc = queue.alloc_desc().ok_or("No control request descriptor")?;
        let resp_desc = match queue.alloc_desc() {
            Some(desc) => desc,
            None => {
                queue.free_desc(req_desc);
                return Err("No control response descriptor");
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
        self.notify(QUEUE_CONTROL);

        let mut virtqueues = self.virtqueues.lock();
        let queue = &mut virtqueues[QUEUE_CONTROL];
        while queue.is_busy() {
            core::hint::spin_loop();
        }
        let Some((used_desc, _used_len)) = queue.pop_used() else {
            queue.free_desc_chain(req_desc);
            return Err("VirtIO sound control response missing");
        };
        if used_desc != req_desc {
            queue.free_desc_chain(used_desc);
            queue.free_desc_chain(req_desc);
            return Err("VirtIO sound control response descriptor mismatch");
        }
        queue.free_desc_chain(req_desc);
        drop(virtqueues);

        let mut out = alloc::vec![0u8; response_len];
        unsafe {
            core::ptr::copy_nonoverlapping(
                resp_alloc.as_ptr() as *const u8,
                out.as_mut_ptr(),
                response_len,
            );
        }
        Ok(out)
    }

    fn tx_in_flight_count(&self) -> usize {
        self.in_flight.lock().len()
    }

    fn drain_tx_queue(&self) -> Result<(), &'static str> {
        for _ in 0..TX_DRAIN_SPIN_LIMIT {
            self.process_completions();
            if self.tx_in_flight_count() == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }

        self.process_completions();
        if self.tx_in_flight_count() == 0 {
            Ok(())
        } else {
            Err("VirtIO sound TX drain timed out")
        }
    }
}

impl AudioPlaybackDevice for VirtioSndDevice {
    fn capabilities(&self) -> AudioPcmCapabilities {
        *self.capabilities.read()
    }

    fn configure(&self, params: &AudioPcmParams) -> Result<(), &'static str> {
        if params.format != AUDIO_PCM_FORMAT_S16LE || params.rate != 48_000 {
            return Err("VirtIO sound MVP supports only S16LE 48000 Hz");
        }
        let stream_id = (*self.stream_id.read()).ok_or("VirtIO sound has no playback stream")?;
        let configured_periods = (params.buffer_frames / params.period_frames) as usize;
        let req = VirtioSndPcmSetParams {
            hdr: VirtioSndPcmHdr {
                hdr: VirtioSndHdr {
                    code: VIRTIO_SND_R_PCM_SET_PARAMS,
                },
                stream_id,
            },
            buffer_bytes: params.buffer_bytes().ok_or("PCM buffer size overflow")? as u32,
            period_bytes: params.period_bytes().ok_or("PCM period size overflow")? as u32,
            features: 0,
            channels: params.channels as u8,
            format: VIRTIO_SND_PCM_FMT_S16,
            rate: VIRTIO_SND_PCM_RATE_48000,
            padding: 0,
        };
        let status: VirtioSndHdr = self.control_request(&req)?;
        if status.code != VIRTIO_SND_S_OK {
            return Err("VirtIO sound SET_PARAMS failed");
        }
        *self.configured_periods.write() = configured_periods;
        self.pcm_command(VIRTIO_SND_R_PCM_PREPARE)
    }

    fn start(&self) -> Result<(), &'static str> {
        self.pcm_command(VIRTIO_SND_R_PCM_START)
    }

    fn stop(&self) -> Result<(), &'static str> {
        self.pcm_command(VIRTIO_SND_R_PCM_STOP)?;
        self.drain_tx_queue()
    }

    fn release(&self) -> Result<(), &'static str> {
        self.pcm_command(VIRTIO_SND_R_PCM_RELEASE)?;
        self.drain_tx_queue()
    }

    fn submit_period(&self, pcm: &[u8]) -> Result<(), &'static str> {
        let stream_id = (*self.stream_id.read()).ok_or("VirtIO sound has no playback stream")?;
        if pcm.is_empty() || pcm.len() > PAGE_SIZE {
            return Err("Invalid VirtIO sound period size");
        }

        let header = ContiguousPages::new(1).ok_or("Failed to allocate audio tx header")?;
        let data = ContiguousPages::new(1).ok_or("Failed to allocate audio tx data")?;
        let status = ContiguousPages::new(1).ok_or("Failed to allocate audio tx status")?;
        unsafe {
            let hdr = header.as_ptr() as *mut VirtioSndPcmXfer;
            core::ptr::write(hdr, VirtioSndPcmXfer { stream_id });
            core::ptr::copy_nonoverlapping(pcm.as_ptr(), data.as_ptr() as *mut u8, pcm.len());
            core::ptr::write(
                status.as_ptr() as *mut VirtioSndPcmStatus,
                VirtioSndPcmStatus::default(),
            );
        }

        let mut virtqueues = self.virtqueues.lock();
        let queue = &mut virtqueues[QUEUE_TX];
        let header_desc = queue.alloc_desc().ok_or("No audio tx header descriptor")?;
        let data_desc = match queue.alloc_desc() {
            Some(desc) => desc,
            None => {
                queue.free_desc(header_desc);
                return Err("No audio tx data descriptor");
            }
        };
        let status_desc = match queue.alloc_desc() {
            Some(desc) => desc,
            None => {
                queue.free_desc(data_desc);
                queue.free_desc(header_desc);
                return Err("No audio tx status descriptor");
            }
        };

        queue.desc[header_desc].addr = header.as_paddr() as u64;
        queue.desc[header_desc].len = core::mem::size_of::<VirtioSndPcmXfer>() as u32;
        queue.desc[header_desc].flags = DescriptorFlag::Next as u16;
        queue.desc[header_desc].next = data_desc as u16;

        queue.desc[data_desc].addr = data.as_paddr() as u64;
        queue.desc[data_desc].len = pcm.len() as u32;
        queue.desc[data_desc].flags = DescriptorFlag::Next as u16;
        queue.desc[data_desc].next = status_desc as u16;

        queue.desc[status_desc].addr = status.as_paddr() as u64;
        queue.desc[status_desc].len = core::mem::size_of::<VirtioSndPcmStatus>() as u32;
        queue.desc[status_desc].flags = DescriptorFlag::Write as u16;
        queue.desc[status_desc].next = 0;

        if let Err(e) = queue.push(header_desc) {
            queue.free_desc_chain(header_desc);
            return Err(e);
        }
        self.in_flight.lock().push(TxPeriod {
            desc_idx: header_desc,
            header,
            data,
            status,
        });
        drop(virtqueues);
        self.notify(QUEUE_TX);
        Ok(())
    }

    fn process_completions(&self) -> usize {
        let mut completed = 0usize;
        let mut virtqueues = self.virtqueues.lock();
        let queue = &mut virtqueues[QUEUE_TX];
        let mut in_flight = self.in_flight.lock();
        while let Some((desc_idx, _used_len)) = queue.pop_used() {
            if let Some(index) = in_flight
                .iter()
                .position(|period| period.desc_idx == desc_idx)
            {
                let period = in_flight.remove(index);
                let status = unsafe {
                    core::ptr::read_volatile(period.status.as_ptr() as *const VirtioSndPcmStatus)
                };
                if status.status != VIRTIO_SND_S_OK && status.status != VIRTIO_SND_S_IO_ERR {
                    crate::println!("[virtio-snd] unexpected tx status {:#x}", status.status);
                }
                let _ = (period.header.as_paddr(), period.data.as_paddr());
                queue.free_desc_chain(desc_idx);
                completed += 1;
            } else {
                queue.free_desc_chain(desc_idx);
            }
        }
        completed
    }

    fn max_in_flight_periods(&self) -> usize {
        let configured_periods = *self.configured_periods.read();
        let tx_queue_period_capacity = QUEUE_TX_SIZE / TX_DESCRIPTORS_PER_PERIOD;
        configured_periods.clamp(1, tx_queue_period_capacity)
    }
}

impl Device for VirtioSndDevice {
    fn device_type(&self) -> DeviceType {
        DeviceType::Generic
    }

    fn name(&self) -> &'static str {
        "virtio-snd"
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn core::any::Any {
        self
    }
}

impl ControlOps for VirtioSndDevice {}

impl MemoryMappingOps for VirtioSndDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        Err("VirtIO sound backend does not support direct mmap")
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        false
    }
}

impl Selectable for VirtioSndDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

impl VirtioDevice for VirtioSndDevice {
    fn pci_transport(&self) -> Option<VirtioPciTransport> {
        self.pci_transport
    }

    fn get_base_addr(&self) -> usize {
        self.base_addr
    }

    fn get_virtqueue_count(&self) -> usize {
        QUEUE_RX + 1
    }

    fn get_virtqueue_size(&self, queue_idx: usize) -> usize {
        if queue_idx >= self.get_virtqueue_count() {
            panic!("Invalid queue index for VirtIO sound device: {}", queue_idx);
        }
        let virtqueues = self.virtqueues.lock();
        virtqueues[queue_idx].get_queue_size()
    }

    fn get_supported_features(&self, device_features: u64) -> u64 {
        let mut result = 0;
        if self.allow_ring_features() {
            result |= device_features & (1u64 << VIRTIO_RING_F_INDIRECT_DESC);
            result &= !(1u64 << VIRTIO_RING_F_EVENT_IDX);
        }
        if self.pci_transport().is_some() {
            result |= device_features & (1u64 << VIRTIO_F_VERSION_1);
        }
        result
    }

    fn get_queue_desc_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }
        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].get_raw_ptr() as usize) as u64)
    }

    fn get_queue_driver_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }
        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].avail.flags as *const _ as usize) as u64)
    }

    fn get_queue_device_addr(&self, queue_idx: usize) -> Option<u64> {
        if queue_idx >= self.get_virtqueue_count() {
            return None;
        }
        let virtqueues = self.virtqueues.lock();
        Some(virt_to_phys(virtqueues[queue_idx].used.flags as *const _ as usize) as u64)
    }
}

/// Register a VirtIO sound backend with the native audio subsystem.
///
/// # Arguments
///
/// * `backend` - VirtIO sound playback backend.
///
/// # Returns
///
/// The native audio character device name.
pub fn register_audio_device(backend: Arc<VirtioSndDevice>) -> alloc::string::String {
    let audio_backend: Arc<dyn AudioPlaybackDevice> = backend;
    register_playback_device(audio_backend)
}

fn default_capabilities() -> AudioPcmCapabilities {
    AudioPcmCapabilities {
        formats: 1 << AUDIO_PCM_FORMAT_S16LE,
        min_rate: 48_000,
        max_rate: 48_000,
        min_channels: 2,
        max_channels: 2,
        min_period_frames: 64,
        max_period_frames: 4_096,
        min_buffer_frames: 128,
        max_buffer_frames: 65_536,
    }
}

fn capabilities_from_pcm_info(info: &VirtioSndPcmInfo) -> AudioPcmCapabilities {
    let mut caps = default_capabilities();
    caps.min_channels = info.channels_min as u16;
    caps.max_channels = info.channels_max as u16;
    if (info.formats & (1u64 << VIRTIO_SND_PCM_FMT_S16)) == 0 {
        caps.formats = 0;
    }
    caps
}

fn read_unaligned<T: Copy>(bytes: &[u8]) -> T {
    assert!(bytes.len() >= core::mem::size_of::<T>());
    unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const T) }
}
