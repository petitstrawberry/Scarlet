#![no_std]
#![allow(dead_code)]

extern crate alloc;

mod debug;
mod firmware;
pub mod h264;
mod vvideo;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub use debug::{AvdTraceEvent, AvdTraceKind};
pub use firmware::AvdFirmwareMessage;

use debug::AvdTraceLog;
use firmware::AvdFirmwareMailbox;
use h264::H264DecodeRequest;
use scarlet::{
    arch::mmio,
    device::{
        DeviceInfo,
        iommu::{DmaContext, IommuDomainConfig, IommuDomainType},
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
        video::{
            SCARLET_VIDEO_FORMAT_H264, SCARLET_VIDEO_FRAME_HEADER_LEN,
            SCARLET_VIDEO_PIXEL_FORMAT_NV12, VideoBackendCapabilities, VideoBackendDecodeRequest,
            VideoBackendDecodedFrame, VideoDecodeBackend, register_video_backend,
        },
    },
    early_println,
    sync::Mutex,
    vm,
};

const AVD_DEFAULT_IOVA_BASE: u64 = 0x4000_0000;
const AVD_DEFAULT_IOVA_SIZE: u64 = 0x4000_0000;

const REG_STATUS: usize = 0x0000;
const REG_CONTROL: usize = 0x0004;
const REG_IRQ_STATUS: usize = 0x0010;
const REG_IRQ_MASK: usize = 0x0014;
const REG_FW_BASE_LO: usize = 0x0100;
const REG_FW_BASE_HI: usize = 0x0104;
const REG_FW_SIZE: usize = 0x0108;
const REG_MAILBOX_AP_TO_CM3: usize = 0x0200;
const REG_MAILBOX_CM3_TO_AP: usize = 0x0204;

const CONTROL_CM3_RESET: u32 = 1 << 0;
const CONTROL_CM3_RUN: u32 = 1 << 1;
const CONTROL_IRQ_ENABLE: u32 = 1 << 8;
const AVD_TRACE_CAPACITY: usize = 128;
const AVD_MAPPED_INPUT_BYTES: usize = 8 * 1024 * 1024;
const AVD_MAX_DECODED_FRAME_BYTES: usize = 16 * 1024 * 1024;
const AVD_MAPPED_OUTPUT_BYTES: usize = align_up_const(
    AVD_MAX_DECODED_FRAME_BYTES + SCARLET_VIDEO_FRAME_HEADER_LEN,
    4096,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppleAvdSoc {
    T8103,
    T6000,
    T6020,
    T8112,
    Unknown,
}

impl AppleAvdSoc {
    fn from_device(device: &PlatformDeviceInfo) -> Self {
        for compatible in device.compatible() {
            match compatible {
                "apple,t8103-avd" => return Self::T8103,
                "apple,t6000-avd" => return Self::T6000,
                "apple,t6020-avd" => return Self::T6020,
                "apple,t8112-avd" => return Self::T8112,
                _ => {}
            }
        }
        Self::Unknown
    }

    fn name(self) -> &'static str {
        match self {
            Self::T8103 => "t8103",
            Self::T6000 => "t6000",
            Self::T6020 => "t6020",
            Self::T8112 => "t8112",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AvdFirmwareState {
    Missing,
    Staged,
    Running,
    Faulted,
}

#[derive(Clone, Copy)]
struct AvdRegisters {
    base: usize,
}

impl AvdRegisters {
    fn new(base: usize) -> Self {
        Self { base }
    }

    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        // SAFETY: `base` is an ioremap'd Apple AVD MMIO base and offsets are
        // register offsets defined by this driver.
        unsafe { mmio::read32(self.base + offset) }
    }

    #[inline]
    fn write32(&self, offset: usize, value: u32) {
        // SAFETY: `base` is an ioremap'd Apple AVD MMIO base and offsets are
        // register offsets defined by this driver.
        unsafe { mmio::write32(self.base + offset, value) }
    }

    fn status(&self) -> u32 {
        self.read32(REG_STATUS)
    }

    fn irq_status(&self) -> u32 {
        self.read32(REG_IRQ_STATUS)
    }

    fn mask_irqs(&self) {
        self.write32(REG_IRQ_MASK, 0);
    }

    fn enable_irqs(&self) {
        self.write32(REG_IRQ_MASK, u32::MAX);
    }

    fn hold_cm3_in_reset(&self) {
        self.write32(REG_CONTROL, CONTROL_CM3_RESET);
    }

    fn run_cm3(&self) {
        self.write32(REG_CONTROL, CONTROL_CM3_RUN | CONTROL_IRQ_ENABLE);
    }

    fn stage_firmware_window(&self, dma_addr: u64, size: usize) {
        self.write32(REG_FW_BASE_LO, dma_addr as u32);
        self.write32(REG_FW_BASE_HI, (dma_addr >> 32) as u32);
        self.write32(REG_FW_SIZE, size as u32);
    }

    fn send_mailbox(&self, value: u32) {
        self.write32(REG_MAILBOX_AP_TO_CM3, value);
    }

    fn recv_mailbox(&self) -> u32 {
        self.read32(REG_MAILBOX_CM3_TO_AP)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AvdStatusSnapshot {
    status: u32,
    irq_status: u32,
    mailbox: u32,
}

/// Apple AVD hardware instance discovered from ADT/FDT.
pub struct AppleAvd {
    name: &'static str,
    soc: AppleAvdSoc,
    paddr: usize,
    size: usize,
    irq: Option<u32>,
    registers: AvdRegisters,
    dma: DmaContext,
    mailbox: AvdFirmwareMailbox,
    trace: AvdTraceLog,
    firmware_state: AvdFirmwareState,
}

impl AppleAvd {
    fn new(
        name: &'static str,
        soc: AppleAvdSoc,
        paddr: usize,
        size: usize,
        irq: Option<u32>,
        registers: AvdRegisters,
        dma: DmaContext,
    ) -> Self {
        Self {
            name,
            soc,
            paddr,
            size,
            irq,
            registers,
            dma,
            mailbox: AvdFirmwareMailbox::new(),
            trace: AvdTraceLog::new(AVD_TRACE_CAPACITY),
            firmware_state: AvdFirmwareState::Missing,
        }
    }

    /// Return the firmware node name used for this AVD instance.
    ///
    /// # Returns
    ///
    /// Static platform device name from discovery.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Return the physical MMIO base address.
    ///
    /// # Returns
    ///
    /// Physical address of the AVD register aperture.
    pub fn paddr(&self) -> usize {
        self.paddr
    }

    /// Return the physical MMIO aperture size.
    ///
    /// # Returns
    ///
    /// Byte length of the AVD register aperture.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Return the interrupt line discovered for the device.
    ///
    /// # Returns
    ///
    /// Interrupt number when firmware supplied one.
    pub fn irq(&self) -> Option<u32> {
        self.irq
    }

    /// Return the device DMA context.
    ///
    /// # Returns
    ///
    /// DMA context resolved through DART when firmware declares an IOMMU.
    pub fn dma_context(&self) -> &DmaContext {
        &self.dma
    }

    /// Return retained debug trace events.
    ///
    /// # Returns
    ///
    /// Trace event slice ordered from oldest to newest.
    pub fn trace_entries(&self) -> &[AvdTraceEvent] {
        self.trace.entries()
    }

    /// Clear retained debug trace events.
    pub fn clear_trace(&mut self) {
        self.trace.clear();
    }

    /// Submit a H.264 request to the firmware mailbox.
    ///
    /// # Arguments
    ///
    /// * `request` - H.264 decode request lowered by the frontend.
    ///
    /// # Returns
    ///
    /// Driver-local firmware command tag on success.
    pub fn submit_h264_request(
        &mut self,
        request: &H264DecodeRequest,
    ) -> Result<u32, &'static str> {
        if self.firmware_state != AvdFirmwareState::Running {
            return Err("apple-avd: firmware is not running");
        }

        let command = self.mailbox.encode_h264_decode(request);
        self.registers.send_mailbox(command.raw);
        self.trace.push(
            AvdTraceKind::DecodeSubmit,
            request.session_id,
            request.frame_number as u64,
        );
        self.trace.push(
            AvdTraceKind::MailboxTx,
            command.raw as u64,
            command.tag as u64,
        );
        Ok(command.tag)
    }

    /// Poll one firmware mailbox message.
    ///
    /// # Returns
    ///
    /// Classified firmware message when the mailbox contains a non-zero word.
    pub fn poll_firmware_message(&mut self) -> Option<AvdFirmwareMessage> {
        let raw = self.registers.recv_mailbox();
        if raw == 0 {
            return None;
        }

        let message = AvdFirmwareMessage::decode(raw);
        self.trace.push(AvdTraceKind::MailboxRx, raw as u64, 0);
        match message {
            AvdFirmwareMessage::Ready => {
                self.firmware_state = AvdFirmwareState::Running;
                self.trace.push(AvdTraceKind::Firmware, raw as u64, 0);
            }
            AvdFirmwareMessage::VideoProcessorDone | AvdFirmwareMessage::PostProcessorDone => {
                self.trace.push(AvdTraceKind::DecodeComplete, raw as u64, 0);
            }
            message if message.is_fault() => {
                self.mark_firmware_faulted();
                self.trace.push(AvdTraceKind::Fault, raw as u64, 0);
            }
            _ => {}
        }

        Some(message)
    }

    fn snapshot(&self) -> AvdStatusSnapshot {
        AvdStatusSnapshot {
            status: self.registers.status(),
            irq_status: self.registers.irq_status(),
            mailbox: self.registers.recv_mailbox(),
        }
    }

    fn prepare_for_firmware(&mut self, firmware_dma_addr: u64, firmware_size: usize) {
        self.registers.mask_irqs();
        self.registers.hold_cm3_in_reset();
        self.registers
            .stage_firmware_window(firmware_dma_addr, firmware_size);
        self.firmware_state = AvdFirmwareState::Staged;
        self.trace.push(
            AvdTraceKind::Firmware,
            firmware_dma_addr,
            firmware_size as u64,
        );
    }

    fn start_firmware(&mut self) {
        self.registers.enable_irqs();
        self.registers.run_cm3();
        self.firmware_state = AvdFirmwareState::Running;
        self.trace.push(AvdTraceKind::Firmware, 1, 0);
    }

    fn mark_firmware_faulted(&mut self) {
        self.firmware_state = AvdFirmwareState::Faulted;
    }
}

static AVD_REGISTRY: Mutex<Vec<Arc<Mutex<AppleAvd>>>> = Mutex::new(Vec::new());

fn register_avd(avd: AppleAvd) -> u32 {
    let mut registry = AVD_REGISTRY.lock();
    let id = registry.len() as u32;
    registry.push(Arc::new(Mutex::new(avd)));
    id
}

/// Return a registered Apple AVD instance by index.
///
/// # Arguments
///
/// * `id` - Zero-based registration index.
///
/// # Returns
///
/// Reference-counted AVD instance when present.
pub fn get_apple_avd(id: u32) -> Option<Arc<Mutex<AppleAvd>>> {
    let registry = AVD_REGISTRY.lock();
    registry.get(id as usize).cloned()
}

struct AppleAvdVideoBackend {
    avd_id: u32,
}

impl AppleAvdVideoBackend {
    fn new(avd_id: u32) -> Self {
        Self { avd_id }
    }

    fn avd(&self) -> Result<Arc<Mutex<AppleAvd>>, &'static str> {
        get_apple_avd(self.avd_id).ok_or("apple-avd: backend instance disappeared")
    }
}

impl VideoDecodeBackend for AppleAvdVideoBackend {
    fn name(&self) -> &'static str {
        "apple-avd"
    }

    fn capabilities(&self) -> VideoBackendCapabilities {
        VideoBackendCapabilities {
            max_sessions: 1,
            mapped_input_len: AVD_MAPPED_INPUT_BYTES as u32,
            mapped_output_len: AVD_MAPPED_OUTPUT_BYTES as u32,
            output_pixel_format: SCARLET_VIDEO_PIXEL_FORMAT_NV12,
            supports_h264: true,
            supports_av1: false,
        }
    }

    fn create_session(&self, coded_format: u32) -> Result<u32, &'static str> {
        if coded_format != SCARLET_VIDEO_FORMAT_H264 {
            return Err("apple-avd: only H.264 sessions are supported");
        }
        Ok(1)
    }

    fn destroy_session(&self, stream_id: u32) -> Result<(), &'static str> {
        if stream_id != 1 {
            return Err("apple-avd: invalid stream id");
        }
        Ok(())
    }

    fn submit_decode(&self, request: &VideoBackendDecodeRequest) -> Result<(), &'static str> {
        if request.stream_id != 1 {
            return Err("apple-avd: invalid stream id");
        }
        if request.coded_format != SCARLET_VIDEO_FORMAT_H264 {
            return Err("apple-avd: unsupported coded format");
        }
        if request.input_len == 0 {
            return Err("apple-avd: empty input access unit");
        }

        let avd = self.avd()?;
        let mut avd = avd.lock();
        avd.trace.push(
            AvdTraceKind::DecodeSubmit,
            request.input_dma_addr,
            request.input_len as u64,
        );
        Err("apple-avd: hardware decode submission is not wired yet")
    }

    fn dequeue_frame(
        &self,
        stream_id: u32,
    ) -> Result<Option<VideoBackendDecodedFrame>, &'static str> {
        if stream_id != 1 {
            return Err("apple-avd: invalid stream id");
        }
        Ok(None)
    }
}

fn first_mem_resource(device: &PlatformDeviceInfo) -> Option<(usize, usize)> {
    device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .map(|resource| {
            let size = resource
                .end
                .checked_sub(resource.start)
                .and_then(|span| span.checked_add(1))
                .unwrap_or(0);
            (resource.start, size)
        })
}

fn first_irq(device: &PlatformDeviceInfo) -> Option<u32> {
    device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::IRQ))
        .map(|resource| resource.start as u32)
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let (paddr, size) = first_mem_resource(device).ok_or("apple-avd: missing MMIO resource")?;
    if size == 0 {
        return Err("apple-avd: empty MMIO resource");
    }

    let vaddr = vm::ioremap(paddr, size).map_err(|_| "apple-avd: MMIO ioremap failed")?;
    let dma = DeviceManager::get_manager().resolve_platform_dma_context(
        device,
        IommuDomainConfig {
            domain_type: IommuDomainType::Dma,
            iova_base: AVD_DEFAULT_IOVA_BASE,
            iova_size: AVD_DEFAULT_IOVA_SIZE,
        },
    )?;

    let soc = AppleAvdSoc::from_device(device);
    let irq = first_irq(device);
    let mut avd = AppleAvd::new(
        device.name(),
        soc,
        paddr,
        size,
        irq,
        AvdRegisters::new(vaddr),
        dma,
    );
    let snapshot = avd.snapshot();
    avd.trace
        .push(AvdTraceKind::Probe, paddr as u64, size as u64);
    let id = register_avd(avd);
    let backend: Arc<dyn VideoDecodeBackend> = Arc::new(AppleAvdVideoBackend::new(id));
    let backend_id = register_video_backend(Arc::clone(&backend));
    vvideo::register_avd_vvideo_device(backend);

    early_println!(
        "[apple-avd] registered {} id={} backend={} soc={} mmio={:#x}+{:#x} irq={:?} status={:#x} irq_status={:#x}",
        device.name(),
        id,
        backend_id,
        soc.name(),
        paddr,
        size,
        irq,
        snapshot.status,
        snapshot.irq_status
    );

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_apple_avd_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-avd",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-avd",
            "apple,t6000-avd",
            "apple,t6020-avd",
            "apple,t8112-avd",
            "apple,avd",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_apple_avd_driver);

#[used]
static SCARLET_DRIVER_APPLE_AVD_ANCHOR: fn() = force_link;

/// Keep the Apple AVD driver crate linked into Scarlet module bundles.
pub fn force_link() {}

const fn align_up_const(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}
