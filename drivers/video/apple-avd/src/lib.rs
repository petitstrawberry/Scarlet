#![no_std]
#![allow(dead_code)]

extern crate alloc;

mod debug;
mod debug_device;
mod firmware;
pub mod h264;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub use debug::{AvdTraceEvent, AvdTraceKind};
pub use firmware::AvdFirmwareMessage;

use debug::AvdTraceLog;
use firmware::AvdFirmwareMailbox;
use h264::{
    AnnexBAccessUnit, AvdDmaRange, AvdH264InstructionStream, AvdH264Workspace, H264DecodeRequest,
    H264FrontendError, H264StreamParameters,
};
use scarlet::{
    arch::{self, mmio},
    device::{
        DeviceInfo,
        iommu::{DmaContext, DmaMapping, IommuDomainConfig, IommuDomainType, IommuMapFlags},
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
        video::{
            SCARLET_VIDEO_FORMAT_H264, SCARLET_VIDEO_FRAME_HEADER_LEN, SCARLET_VIDEO_FRAME_MAGIC,
            SCARLET_VIDEO_PIXEL_FORMAT_NV12, ScarletVideoDequeuedFrame, VideoBackendCapabilities,
            VideoBackendDecodeRequest, VideoBackendDecodedFrame, VideoDecodeBackend,
            register_video_backend, register_video_decode_device,
        },
    },
    early_println,
    environment::PAGE_SIZE,
    mem::page::ContiguousPages,
    sync::Mutex,
    vm,
};

const AVD_DEFAULT_IOVA_BASE: u64 = 0x4000_0000;
const AVD_DEFAULT_IOVA_SIZE: u64 = 0x4000_0000;
const DEFAULT_AVD_FIRMWARE: &[u8] = include_bytes!(env!("SCARLET_APPLE_AVD_FW_BIN"));

const REG_STATUS: usize = 0x0000;
const REG_CONTROL: usize = 0x0004;
const REG_IRQ_STATUS: usize = 0x0010;
const REG_IRQ_MASK: usize = 0x0014;
const REG_FW_BASE_LO: usize = 0x0100;
const REG_FW_BASE_HI: usize = 0x0104;
const REG_FW_SIZE: usize = 0x0108;
const REG_MAILBOX_AP_TO_CM3: usize = 0x0200;
const REG_MAILBOX_CM3_TO_AP: usize = 0x0204;
const REG_H264_INSTRUCTION: usize = 0x1104000;
const REG_H264_SUBMIT: usize = 0x1104014;
const REG_H264_COUNTER0: usize = 0x1104018;
const REG_H264_COUNTER1: usize = 0x110401c;
const REG_H264_COUNTER2: usize = 0x1104020;
const REG_H264_COUNTER3: usize = 0x1104024;
const REG_H264_COUNTER4: usize = 0x1104028;
const REG_H264_CONTROL0: usize = 0x1104034;
const REG_H264_CONTROL1: usize = 0x110403c;
const REG_H264_TIMEOUT: usize = 0x110405c;
const REG_H264_STATUS: usize = 0x1104060;
const REG_H264_STATUS_MASK: usize = 0x1104064;
const REG_AVD_DMA_CONFIG_BASE: usize = 0x108ee90;

const CONTROL_CM3_RESET: u32 = 1 << 0;
const CONTROL_CM3_RUN: u32 = 1 << 1;
const CONTROL_IRQ_ENABLE: u32 = 1 << 8;
const H264_SUBMIT_START: u32 = 1;
const H264_STATUS_DONE_MASK: u32 = 0x0084_2108;
const H264_STATUS_ERROR_MASK: u32 = 0x0000_0003;
const AVD_TRACE_CAPACITY: usize = 128;
const AVD_DMA_GRANULE: usize = 0x4000;
const AVD_MAPPED_INPUT_BYTES: usize = 8 * 1024 * 1024;
const AVD_MAX_DECODED_FRAME_BYTES: usize = 16 * 1024 * 1024;
const AVD_MAPPED_OUTPUT_BYTES: usize = align_up_const(
    AVD_MAX_DECODED_FRAME_BYTES + SCARLET_VIDEO_FRAME_HEADER_LEN,
    AVD_DMA_GRANULE,
);
const AVD_MAX_SESSIONS: usize = 4;
const AVD_WORKSPACE_BYTES: usize = 16 * 1024 * 1024;
const AVD_WORKSPACE_ALIGN: usize = AVD_DMA_GRANULE;
const AVD_WORKSPACE_INST_FIFO_OFFSET: usize = 0x4000;
const AVD_WORKSPACE_INST_FIFO_BYTES: usize = 0x100000;
const AVD_WORKSPACE_PPS_TILE_OFFSET: usize = 0x140000;
const AVD_WORKSPACE_SPS_TILE_OFFSET: usize = 0x200000;
const AVD_WORKSPACE_REFERENCE_OFFSET: usize = 0x400000;
const AVD_REFERENCE_SLOT_COUNT: usize = 4;

const AVD_H264_DMA_CONFIG: [u32; 30] = [
    0x0402_0002,
    0x0002_0002,
    0x0402_0002,
    0x0402_0002,
    0x0402_0002,
    0x0007_0007,
    0x0007_0007,
    0x0007_0007,
    0x0007_0007,
    0x0007_0007,
    0x0402_0002,
    0x0002_0002,
    0x0402_0002,
    0x0402_0002,
    0x0402_0002,
    0x0007_0007,
    0x0007_0007,
    0x0007_0007,
    0x0007_0007,
    0x0007_0007,
    0x0402_0002,
    0x0202_0202,
    0x0402_0002,
    0x0402_0002,
    0x0402_0202,
    0x0007_0007,
    0x0007_0007,
    0x0007_0007,
    0x0007_0007,
    0x0007_0007,
];

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

    fn clear_recv_mailbox(&self) {
        self.write32(REG_MAILBOX_CM3_TO_AP, 0);
    }

    fn init_h264_engine(&self) {
        self.write32(REG_H264_COUNTER0, 0x78);
        self.write32(REG_H264_COUNTER1, 0x78);
        self.write32(REG_H264_COUNTER2, 0x78);
        self.write32(REG_H264_COUNTER3, 0x78);
        self.write32(REG_H264_COUNTER4, 0x20);
        self.write32(REG_H264_CONTROL0, 0);
        self.write32(REG_H264_CONTROL1, 0);
        self.write32(REG_H264_TIMEOUT, 0x0050_0000);
        self.write32(REG_H264_STATUS_MASK, 0x3);

        for (index, value) in AVD_H264_DMA_CONFIG.iter().copied().enumerate() {
            self.write32(REG_AVD_DMA_CONFIG_BASE + index * 4, value);
        }
    }

    fn clear_h264_status(&self, mask: u32) {
        self.write32(REG_H264_STATUS, mask);
    }

    fn h264_status(&self) -> u32 {
        self.read32(REG_H264_STATUS)
    }

    fn write_h264_instructions(&self, words: &[u32]) {
        for word in words {
            self.write32(REG_H264_INSTRUCTION, *word);
        }
    }

    fn submit_h264(&self) {
        self.write32(REG_H264_SUBMIT, H264_SUBMIT_START);
    }
}

/// Snapshot of Apple AVD debug status registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdStatusSnapshot {
    /// Top-level AVD status register.
    pub status: u32,
    /// Top-level AVD IRQ status register.
    pub irq_status: u32,
    /// Raw CM3-to-AP mailbox value.
    pub mailbox: u32,
}

struct AvdFirmwareAllocation {
    pages: ContiguousPages,
    mapping: DmaMapping,
    size: usize,
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
    firmware_allocation: Option<AvdFirmwareAllocation>,
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
            firmware_allocation: None,
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

    /// Return the detected Apple Silicon SoC name.
    ///
    /// # Returns
    ///
    /// Static SoC identifier used by driver diagnostics.
    pub fn soc_name(&self) -> &'static str {
        self.soc.name()
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

    /// Return the current firmware lifecycle state.
    ///
    /// # Returns
    ///
    /// Static state name for debug output.
    pub fn firmware_state_name(&self) -> &'static str {
        match self.firmware_state {
            AvdFirmwareState::Missing => "missing",
            AvdFirmwareState::Staged => "staged",
            AvdFirmwareState::Running => "running",
            AvdFirmwareState::Faulted => "faulted",
        }
    }

    /// Return the device-visible address of the staged firmware image.
    ///
    /// # Returns
    ///
    /// Firmware DMA address when an image is currently mapped.
    pub fn firmware_dma_addr(&self) -> Option<u64> {
        self.firmware_allocation
            .as_ref()
            .map(|allocation| allocation.mapping.dma_addr())
    }

    /// Return the staged firmware image size.
    ///
    /// # Returns
    ///
    /// Firmware image byte length when an image is currently mapped.
    pub fn firmware_image_size(&self) -> Option<usize> {
        self.firmware_allocation
            .as_ref()
            .map(|allocation| allocation.size)
    }

    /// Return a snapshot of the top-level debug registers.
    ///
    /// # Returns
    ///
    /// Status, IRQ status, and firmware mailbox values captured together.
    pub fn debug_snapshot(&self) -> AvdStatusSnapshot {
        self.snapshot()
    }

    /// Initialize the H.264 engine registers with the v3-class defaults.
    pub fn init_h264_engine(&mut self) {
        self.registers.init_h264_engine();
        self.trace.push(AvdTraceKind::Firmware, 0x1104_0000, 0);
    }

    /// Stage and start the bundled CM3 firmware image.
    ///
    /// # Arguments
    ///
    /// * `image` - Raw Cortex-M3 firmware image bytes.
    ///
    /// # Returns
    ///
    /// `Ok(())` once the image is copied, mapped, and CM3 run has been
    /// requested.
    pub fn boot_firmware(&mut self, image: &[u8]) -> Result<(), &'static str> {
        if image.is_empty() {
            return Err("apple-avd: empty firmware image");
        }
        let granule = self.dma.mapping_granule().max(PAGE_SIZE);
        let byte_len = align_up(image.len(), granule);
        let page_count = byte_len.div_ceil(PAGE_SIZE);
        let pages = ContiguousPages::new_aligned(page_count, granule)
            .ok_or("apple-avd: firmware allocation failed")?;

        // SAFETY: `pages` owns at least `byte_len` bytes and `image.len()` was
        // used to size-check the copy. The source firmware image is immutable
        // static data and cannot overlap with PMM pages.
        unsafe {
            core::ptr::copy_nonoverlapping(image.as_ptr(), pages.as_ptr() as *mut u8, image.len());
        }
        arch::clean_dcache_to_poc_range(pages.as_vaddr(), byte_len);

        let mapping = self
            .dma
            .map_phys_owned(
                pages.as_paddr(),
                byte_len,
                IommuMapFlags::READ | IommuMapFlags::EXECUTE | IommuMapFlags::COHERENT,
            )
            .map_err(|_| "apple-avd: firmware DMA map failed")?;
        let dma_addr = mapping.dma_addr();
        self.prepare_for_firmware(dma_addr, image.len());
        self.start_firmware();
        self.firmware_allocation = Some(AvdFirmwareAllocation {
            pages,
            mapping,
            size: image.len(),
        });

        for _ in 0..1024 {
            if matches!(
                self.poll_firmware_message(),
                Some(AvdFirmwareMessage::Ready)
            ) {
                break;
            }
            core::hint::spin_loop();
        }
        Ok(())
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

    /// Submit a generated H.264 instruction stream to the MMIO command path.
    ///
    /// # Arguments
    ///
    /// * `request` - H.264 decode request metadata.
    /// * `instructions` - AVD H.264 instruction stream.
    ///
    /// # Returns
    ///
    /// Status register value observed immediately before submit.
    pub fn submit_h264_mmio(
        &mut self,
        request: &H264DecodeRequest,
        instructions: &AvdH264InstructionStream,
    ) -> Result<u32, &'static str> {
        if instructions.words().is_empty() {
            return Err("apple-avd: empty H.264 instruction stream");
        }
        self.registers.write_h264_instructions(instructions.words());
        let status_before = self.registers.h264_status();
        self.registers.submit_h264();
        self.trace.push(
            AvdTraceKind::DecodeSubmit,
            request.input.dma_addr,
            instructions.words().len() as u64,
        );
        Ok(status_before)
    }

    /// Return the current H.264 status register.
    ///
    /// # Returns
    ///
    /// Raw H.264 engine status.
    pub fn h264_status(&self) -> u32 {
        self.registers.h264_status()
    }

    /// Clear H.264 status and replay engine initialization after an error.
    ///
    /// # Arguments
    ///
    /// * `reason` - Driver-local reason or status value recorded in the trace.
    pub fn recover_h264_engine(&mut self, reason: u64) {
        self.trace.push(AvdTraceKind::Fault, reason, 0);
        self.registers
            .clear_h264_status(H264_STATUS_DONE_MASK | H264_STATUS_ERROR_MASK);
        self.registers.init_h264_engine();
        self.registers.clear_recv_mailbox();
        self.trace.push(AvdTraceKind::Firmware, 0x1104_0000, 1);
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
        self.registers.clear_recv_mailbox();
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

struct AvdSessionWorkspace {
    pages: ContiguousPages,
    mapping: DmaMapping,
    byte_len: usize,
}

impl AvdSessionWorkspace {
    fn new(avd: &AppleAvd) -> Result<Self, &'static str> {
        let granule = avd.dma_context().mapping_granule().max(AVD_WORKSPACE_ALIGN);
        let byte_len = align_up(AVD_WORKSPACE_BYTES, granule);
        let page_count = byte_len.div_ceil(PAGE_SIZE);
        let pages = ContiguousPages::new_aligned(page_count, granule)
            .ok_or("apple-avd: workspace allocation failed")?;
        let mapping = avd
            .dma_context()
            .map_phys_owned(
                pages.as_paddr(),
                byte_len,
                IommuMapFlags::READ | IommuMapFlags::WRITE | IommuMapFlags::COHERENT,
            )
            .map_err(|_| "apple-avd: workspace DMA map failed")?;
        Ok(Self {
            pages,
            mapping,
            byte_len,
        })
    }

    fn addresses(&self) -> AvdH264Workspace {
        let base = self.mapping.dma_addr();
        AvdH264Workspace {
            instruction_fifo_dma_addr: base + AVD_WORKSPACE_INST_FIFO_OFFSET as u64,
            pps_tile_dma_addr: base + AVD_WORKSPACE_PPS_TILE_OFFSET as u64,
            sps_tile_dma_addr: base + AVD_WORKSPACE_SPS_TILE_OFFSET as u64,
            reference_dma_addr: base + AVD_WORKSPACE_REFERENCE_OFFSET as u64,
        }
    }

    fn instruction_fifo_vaddr(&self) -> usize {
        self.pages.as_vaddr() + AVD_WORKSPACE_INST_FIFO_OFFSET
    }

    fn instruction_fifo_mut(&mut self) -> &mut [u8] {
        let ptr = (self.pages.as_vaddr() + AVD_WORKSPACE_INST_FIFO_OFFSET) as *mut u8;
        // SAFETY: `pages` owns `byte_len` bytes, and the instruction FIFO
        // window is inside the workspace constants checked at compile time.
        unsafe { core::slice::from_raw_parts_mut(ptr, AVD_WORKSPACE_INST_FIFO_BYTES) }
    }
}

struct AvdBackendSession {
    stream_id: u32,
    active: bool,
    coded_format: u32,
    next_frame: u32,
    stream_parameters: Option<H264StreamParameters>,
    workspace: Option<AvdSessionWorkspace>,
    reference_frames: Vec<AvdReferenceFrame>,
    next_reference_slot: usize,
    reference_slot_len: usize,
}

impl AvdBackendSession {
    fn new(index: usize) -> Self {
        Self {
            stream_id: (index + 1) as u32,
            active: false,
            coded_format: 0,
            next_frame: 0,
            stream_parameters: None,
            workspace: None,
            reference_frames: Vec::new(),
            next_reference_slot: 0,
            reference_slot_len: 0,
        }
    }

    fn reset(&mut self) {
        self.active = false;
        self.coded_format = 0;
        self.next_frame = 0;
        self.stream_parameters = None;
        self.workspace = None;
        self.reference_frames.clear();
        self.next_reference_slot = 0;
        self.reference_slot_len = 0;
    }

    fn ensure_workspace(
        &mut self,
        avd: &AppleAvd,
    ) -> Result<&mut AvdSessionWorkspace, &'static str> {
        if self.workspace.is_none() {
            self.workspace = Some(AvdSessionWorkspace::new(avd)?);
        }
        self.workspace
            .as_mut()
            .ok_or("apple-avd: workspace unavailable")
    }

    fn prepare_reference_output(
        &mut self,
        avd: &AppleAvd,
        layout: h264::AvdFrameLayout,
    ) -> Result<AvdReferenceOutput, &'static str> {
        if self.workspace.is_none() {
            self.workspace = Some(AvdSessionWorkspace::new(avd)?);
        }
        let payload_len = layout.output_len();
        let slot_len = align_up(payload_len, AVD_DMA_GRANULE);
        let reference_capacity = self
            .workspace
            .as_ref()
            .and_then(|workspace| {
                workspace
                    .byte_len
                    .checked_sub(AVD_WORKSPACE_REFERENCE_OFFSET)
            })
            .ok_or("apple-avd: workspace reference region is unavailable")?;
        let required = slot_len
            .checked_mul(AVD_REFERENCE_SLOT_COUNT)
            .ok_or("apple-avd: reference slot size overflow")?;
        if payload_len == 0 || required > reference_capacity {
            return Err("apple-avd: decoded frame exceeds AVD reference workspace");
        }
        if self.reference_slot_len != slot_len {
            self.reference_frames.clear();
            self.next_reference_slot = 0;
            self.reference_slot_len = slot_len;
        }

        let slot = self.next_reference_slot % AVD_REFERENCE_SLOT_COUNT;
        self.next_reference_slot = (slot + 1) % AVD_REFERENCE_SLOT_COUNT;
        let slot_offset = AVD_WORKSPACE_REFERENCE_OFFSET + slot * slot_len;
        let workspace = self
            .workspace
            .as_ref()
            .ok_or("apple-avd: workspace unavailable")?;
        Ok(AvdReferenceOutput {
            slot,
            dma_addr: workspace.mapping.dma_addr() + slot_offset as u64,
            vaddr: workspace.pages.as_vaddr() + slot_offset,
            len: slot_len,
        })
    }
}

#[derive(Clone, Copy)]
struct AvdReferenceFrame {
    slot: usize,
    frame_number: u32,
    timestamp: u64,
    layout: h264::AvdFrameLayout,
}

#[derive(Clone, Copy)]
struct AvdReferenceOutput {
    slot: usize,
    dma_addr: u64,
    vaddr: usize,
    len: usize,
}

struct AvdPendingDecode {
    stream_id: u32,
    frame_number: u32,
    timestamp: u64,
    layout: h264::AvdFrameLayout,
    payload_len: usize,
    output_base_paddr: usize,
    hardware_output_vaddr: usize,
    hardware_output_len: usize,
    reference_slot: usize,
    store_reference: bool,
    is_idr: bool,
    status_before: u32,
    command_tag: u32,
    input_mapping: DmaMapping,
    output_mapping: DmaMapping,
}

struct AvdBackendState {
    sessions: Vec<AvdBackendSession>,
    pending: VecDeque<AvdPendingDecode>,
    completed: VecDeque<VideoBackendDecodedFrame>,
}

impl AvdBackendState {
    fn new() -> Self {
        let mut sessions = Vec::new();
        for index in 0..AVD_MAX_SESSIONS {
            sessions.push(AvdBackendSession::new(index));
        }
        Self {
            sessions,
            pending: VecDeque::new(),
            completed: VecDeque::new(),
        }
    }

    fn allocate_session(&mut self, coded_format: u32) -> Result<u32, &'static str> {
        let session = self
            .sessions
            .iter_mut()
            .find(|session| !session.active)
            .ok_or("apple-avd: no free video sessions")?;
        session.active = true;
        session.coded_format = coded_format;
        session.next_frame = 0;
        session.stream_parameters = None;
        Ok(session.stream_id)
    }

    fn session_mut(&mut self, stream_id: u32) -> Result<&mut AvdBackendSession, &'static str> {
        self.sessions
            .iter_mut()
            .find(|session| session.stream_id == stream_id)
            .ok_or("apple-avd: invalid stream id")
    }

    fn active_session_mut(
        &mut self,
        stream_id: u32,
    ) -> Result<&mut AvdBackendSession, &'static str> {
        let session = self.session_mut(stream_id)?;
        if !session.active {
            return Err("apple-avd: inactive stream id");
        }
        Ok(session)
    }

    fn session_for_submit(
        &mut self,
        stream_id: u32,
        coded_format: u32,
    ) -> Result<&mut AvdBackendSession, &'static str> {
        let session = self.session_mut(stream_id)?;
        if !session.active {
            session.active = true;
            session.coded_format = coded_format;
        }
        if session.coded_format != coded_format {
            return Err("apple-avd: stream format mismatch");
        }
        Ok(session)
    }

    fn destroy_session(&mut self, stream_id: u32) -> Result<(), &'static str> {
        let session = self.active_session_mut(stream_id)?;
        session.reset();
        self.pending
            .retain(|pending| pending.stream_id != stream_id);
        self.completed.retain(|frame| frame.stream_id != stream_id);
        Ok(())
    }

    fn has_pending_for_stream(&self, stream_id: u32) -> bool {
        self.pending
            .iter()
            .any(|pending| pending.stream_id == stream_id)
    }
}

struct AppleAvdVideoBackend {
    avd_id: u32,
    state: Mutex<AvdBackendState>,
}

impl AppleAvdVideoBackend {
    fn new(avd_id: u32) -> Self {
        Self {
            avd_id,
            state: Mutex::new(AvdBackendState::new()),
        }
    }

    fn avd(&self) -> Result<Arc<Mutex<AppleAvd>>, &'static str> {
        get_apple_avd(self.avd_id).ok_or("apple-avd: backend instance disappeared")
    }

    fn service_completions(&self) -> Result<(), &'static str> {
        let avd = self.avd()?;
        let mut avd = avd.lock();
        let mut state = self.state.lock();
        let message = avd.poll_firmware_message();
        let status = avd.h264_status();
        let Some(front) = state.pending.front() else {
            return Ok(());
        };

        if status != front.status_before && (status & H264_STATUS_ERROR_MASK) != 0 {
            let _ = state.pending.pop_front();
            avd.recover_h264_engine(status as u64);
            return Err("apple-avd: H.264 engine reported an error");
        }

        let completed_by_mailbox = matches!(
            message,
            Some(AvdFirmwareMessage::VideoProcessorDone)
                | Some(AvdFirmwareMessage::PostProcessorDone)
        );
        let completed_by_status =
            status != front.status_before && (status & H264_STATUS_DONE_MASK) != 0;
        if completed_by_mailbox || completed_by_status {
            let pending = state
                .pending
                .pop_front()
                .ok_or("apple-avd: pending queue changed under completion")?;
            finish_pending_decode(&mut state, pending)?;
            avd.trace
                .push(AvdTraceKind::DecodeComplete, status as u64, 0);
        }
        Ok(())
    }
}

impl VideoDecodeBackend for AppleAvdVideoBackend {
    fn name(&self) -> &'static str {
        "apple-avd"
    }

    fn capabilities(&self) -> VideoBackendCapabilities {
        VideoBackendCapabilities {
            max_sessions: AVD_MAX_SESSIONS as u32,
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
        self.state.lock().allocate_session(coded_format)
    }

    fn destroy_session(&self, stream_id: u32) -> Result<(), &'static str> {
        let had_pending = {
            let mut state = self.state.lock();
            let had_pending = state.has_pending_for_stream(stream_id);
            state.destroy_session(stream_id)?;
            had_pending
        };
        if had_pending {
            self.avd()?.lock().recover_h264_engine(stream_id as u64);
        }
        Ok(())
    }

    fn submit_decode(&self, request: &VideoBackendDecodeRequest) -> Result<(), &'static str> {
        if request.coded_format != SCARLET_VIDEO_FORMAT_H264 {
            return Err("apple-avd: unsupported coded format");
        }
        if request.input_len == 0 {
            return Err("apple-avd: empty input access unit");
        }
        if request.output_len as usize <= SCARLET_VIDEO_FRAME_HEADER_LEN {
            return Err("apple-avd: output buffer is too small");
        }
        if request.input_len as usize > AVD_MAPPED_INPUT_BYTES {
            return Err("apple-avd: input exceeds mapped input buffer");
        }

        self.service_completions()?;
        let avd = self.avd()?;
        let mut avd = avd.lock();
        let mut state = self.state.lock();
        if !state.pending.is_empty() {
            return Err("apple-avd: decode already pending");
        }

        let input_vaddr = vm::phys_to_virt(request.input_dma_addr as usize);
        let input_len = request.input_len as usize;
        // SAFETY: `/dev/videoN` passes a PMM-backed physical address for the
        // mapped input range and keeps the pages alive for the lifetime of the
        // device. `input_len` was checked against the mapped capacity above.
        let input_bytes =
            unsafe { core::slice::from_raw_parts(input_vaddr as *const u8, input_len) };
        let access_unit = AnnexBAccessUnit::new(input_bytes);
        let parsed_stream_parameters =
            access_unit.stream_parameters().map_err(h264_error_to_str)?;

        let session = state.session_for_submit(request.stream_id, request.coded_format)?;
        let stream_parameters = if let Some(parameters) = parsed_stream_parameters {
            session.stream_parameters = Some(parameters);
            parameters
        } else {
            session
                .stream_parameters
                .ok_or("apple-avd: missing H.264 SPS before first slice")?
        };
        let layout = stream_parameters.nv12_layout();
        let payload_len = layout.output_len();
        let total_output_len = payload_len
            .checked_add(SCARLET_VIDEO_FRAME_HEADER_LEN)
            .ok_or("apple-avd: output length overflow")?;
        if total_output_len > request.output_len as usize {
            return Err("apple-avd: decoded frame exceeds mapped output buffer");
        }

        let granule = avd.dma_context().mapping_granule().max(PAGE_SIZE);
        let input_map_len = align_up(input_len, granule);
        let output_map_len = align_up(request.output_len as usize, granule);
        let input_mapping = avd
            .dma_context()
            .map_phys_owned(
                request.input_dma_addr as usize,
                input_map_len,
                IommuMapFlags::READ | IommuMapFlags::COHERENT,
            )
            .map_err(|_| "apple-avd: input DMA map failed")?;
        let output_mapping = avd
            .dma_context()
            .map_phys_owned(
                request.output_dma_addr as usize,
                output_map_len,
                IommuMapFlags::READ | IommuMapFlags::WRITE | IommuMapFlags::COHERENT,
            )
            .map_err(|_| "apple-avd: output DMA map failed")?;

        let frame_number = session.next_frame;
        session.next_frame = session.next_frame.wrapping_add(1);
        let reference_output = session.prepare_reference_output(&avd, layout)?;
        let decode_request = H264DecodeRequest::from_access_unit(
            session.stream_id as u64,
            frame_number,
            &access_unit,
            AvdDmaRange {
                dma_addr: input_mapping.dma_addr(),
                len: input_len,
            },
            AvdDmaRange {
                dma_addr: reference_output.dma_addr,
                len: reference_output.len,
            },
            layout,
        )
        .map_err(h264_error_to_str)?;

        let (instructions, inst_len) = {
            let workspace = session.ensure_workspace(&avd)?;
            let mut workspace_addresses = workspace.addresses();
            workspace_addresses.reference_dma_addr = reference_output.dma_addr;
            let instructions = AvdH264InstructionStream::build(
                &decode_request,
                &stream_parameters,
                &decode_request.slice,
                &workspace_addresses,
            );
            let inst_len = instructions
                .write_le_bytes(workspace.instruction_fifo_mut())
                .map_err(h264_error_to_str)?;
            arch::clean_dcache_to_poc_range(workspace.instruction_fifo_vaddr(), inst_len);
            arch::clean_dcache_to_poc_range(workspace.pages.as_vaddr(), workspace.byte_len);
            (instructions, inst_len)
        };

        arch::clean_dcache_to_poc_range(input_vaddr, input_map_len);
        arch::clean_invalidate_dcache_to_poc_range(
            vm::phys_to_virt(request.output_dma_addr as usize),
            output_map_len,
        );

        let status_before = avd.submit_h264_mmio(&decode_request, &instructions)?;
        let command_tag = avd.submit_h264_request(&decode_request)?;
        avd.trace.push(
            AvdTraceKind::DecodeSubmit,
            request.input_dma_addr,
            inst_len as u64,
        );
        state.pending.push_back(AvdPendingDecode {
            stream_id: request.stream_id,
            frame_number,
            timestamp: request.timestamp,
            layout,
            payload_len,
            output_base_paddr: request.output_dma_addr as usize,
            hardware_output_vaddr: reference_output.vaddr,
            hardware_output_len: reference_output.len,
            reference_slot: reference_output.slot,
            store_reference: decode_request.slice.is_reference(),
            is_idr: decode_request.slice.is_idr(),
            status_before,
            command_tag,
            input_mapping,
            output_mapping,
        });
        Ok(())
    }

    fn dequeue_frame(
        &self,
        stream_id: u32,
    ) -> Result<Option<VideoBackendDecodedFrame>, &'static str> {
        self.service_completions()?;
        let mut state = self.state.lock();
        let Some(index) = state
            .completed
            .iter()
            .position(|frame| frame.stream_id == stream_id)
        else {
            return Ok(None);
        };
        Ok(state.completed.remove(index))
    }
}

fn finish_pending_decode(
    state: &mut AvdBackendState,
    pending: AvdPendingDecode,
) -> Result<(), &'static str> {
    let output_vaddr = vm::phys_to_virt(pending.output_base_paddr);
    let total_output_len = pending
        .payload_len
        .checked_add(SCARLET_VIDEO_FRAME_HEADER_LEN)
        .ok_or("apple-avd: completed frame length overflow")?;

    arch::invalidate_dcache_to_poc_range(
        pending.hardware_output_vaddr,
        pending.hardware_output_len,
    );
    // SAFETY: The pending request allocated a reference slot with at least
    // `payload_len` bytes and the frontend checked that the user output buffer
    // can hold the same payload after the SVF1 header.
    unsafe {
        core::ptr::copy_nonoverlapping(
            pending.hardware_output_vaddr as *const u8,
            (output_vaddr + SCARLET_VIDEO_FRAME_HEADER_LEN) as *mut u8,
            pending.payload_len,
        );
    }
    arch::clean_dcache_to_poc_range(
        output_vaddr + SCARLET_VIDEO_FRAME_HEADER_LEN,
        pending.payload_len,
    );
    write_frame_header(
        output_vaddr,
        pending.layout.width,
        pending.layout.height,
        pending.layout.pixel_format,
        pending.payload_len as u32,
    )?;
    arch::clean_dcache_to_poc_range(output_vaddr, SCARLET_VIDEO_FRAME_HEADER_LEN);

    let session = state.session_mut(pending.stream_id)?;
    if pending.is_idr {
        session.reference_frames.clear();
    }
    if pending.store_reference {
        session
            .reference_frames
            .retain(|frame| frame.slot != pending.reference_slot);
        session.reference_frames.push(AvdReferenceFrame {
            slot: pending.reference_slot,
            frame_number: pending.frame_number,
            timestamp: pending.timestamp,
            layout: pending.layout,
        });
        while session.reference_frames.len() > AVD_REFERENCE_SLOT_COUNT {
            session.reference_frames.remove(0);
        }
    }

    state.completed.push_back(VideoBackendDecodedFrame {
        stream_id: pending.stream_id,
        frame: ScarletVideoDequeuedFrame {
            width: pending.layout.width,
            height: pending.layout.height,
            pixel_format: pending.layout.pixel_format,
            payload_offset: (AVD_MAPPED_INPUT_BYTES + SCARLET_VIDEO_FRAME_HEADER_LEN) as u64,
            payload_len: pending.payload_len as u32,
            flags: pending.command_tag,
            timestamp: pending.timestamp,
        },
    });
    let _ = pending.input_mapping.dma_addr();
    let _ = pending.output_mapping.dma_addr();
    let _ = total_output_len;
    Ok(())
}

fn write_frame_header(
    output_vaddr: usize,
    width: u32,
    height: u32,
    pixel_format: u32,
    payload_len: u32,
) -> Result<(), &'static str> {
    let header = output_vaddr as *mut u8;
    // SAFETY: `output_vaddr` points at the beginning of the mapped output
    // buffer. The caller verified the buffer has at least
    // `SCARLET_VIDEO_FRAME_HEADER_LEN` bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(
            SCARLET_VIDEO_FRAME_MAGIC.as_ptr(),
            header,
            SCARLET_VIDEO_FRAME_MAGIC.len(),
        );
        core::ptr::copy_nonoverlapping(width.to_le_bytes().as_ptr(), header.add(4), 4);
        core::ptr::copy_nonoverlapping(height.to_le_bytes().as_ptr(), header.add(8), 4);
        core::ptr::copy_nonoverlapping(pixel_format.to_le_bytes().as_ptr(), header.add(12), 4);
        core::ptr::copy_nonoverlapping(payload_len.to_le_bytes().as_ptr(), header.add(16), 4);
    }
    Ok(())
}

fn h264_error_to_str(error: H264FrontendError) -> &'static str {
    match error {
        H264FrontendError::MissingStartCode => "apple-avd: H.264 access unit missing start code",
        H264FrontendError::EmptyNalUnit => "apple-avd: H.264 access unit contains an empty NAL",
        H264FrontendError::MissingParameterSet => "apple-avd: H.264 parameter set is missing",
        H264FrontendError::InvalidDimensions => "apple-avd: H.264 dimensions are invalid",
        H264FrontendError::MalformedSps => "apple-avd: H.264 SPS is malformed",
        H264FrontendError::UnsupportedSps => "apple-avd: H.264 SPS uses unsupported features",
        H264FrontendError::MalformedSlice => "apple-avd: H.264 slice header is malformed",
        H264FrontendError::InstructionStreamTooLarge => {
            "apple-avd: generated H.264 instruction stream is too large"
        }
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
    avd.init_h264_engine();
    avd.boot_firmware(DEFAULT_AVD_FIRMWARE)?;
    let id = register_avd(avd);
    let backend: Arc<dyn VideoDecodeBackend> = Arc::new(AppleAvdVideoBackend::new(id));
    let backend_id = register_video_backend(Arc::clone(&backend));
    let video_name = register_video_decode_device(Arc::clone(&backend));
    debug_device::register_avd_debug_device(id, Arc::clone(&backend));

    early_println!(
        "[apple-avd] registered {} id={} backend={} video={} soc={} mmio={:#x}+{:#x} irq={:?} status={:#x} irq_status={:#x}",
        device.name(),
        id,
        backend_id,
        video_name,
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

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}
