#![no_std]
#![allow(dead_code)]

extern crate alloc;

mod debug;
#[cfg(feature = "debug-device")]
mod debug_device;
mod firmware;
pub mod h264;

use alloc::{boxed::Box, collections::VecDeque, format, string::String, sync::Arc, vec::Vec};

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
        manager::{DeviceManager, DriverPriority, is_probe_defer, probe_defer},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
        reset::ResetHandle,
        video::{
            SCARLET_VIDEO_FORMAT_H264, SCARLET_VIDEO_FRAME_HEADER_LEN, SCARLET_VIDEO_FRAME_MAGIC,
            SCARLET_VIDEO_PIXEL_FORMAT_NV12, ScarletVideoDequeuedFrame, VideoBackendCapabilities,
            VideoBackendDecodeRequest, VideoBackendDecodedFrame, VideoDecodeBackend,
            register_video_backend, register_video_decode_device,
        },
    },
    environment::PAGE_SIZE,
    mem::page::ContiguousPages,
    println,
    sync::Mutex,
    time, vm,
};
use scarlet_driver_apple_pmgr::{
    PmgrDomain, pmgr_get_domain_by_label, pmgr_get_domain_by_register_paddr,
};

const AVD_DEFAULT_IOVA_BASE: u64 = 0x4000_0000;
const AVD_DEFAULT_IOVA_SIZE: u64 = 0x4000_0000;
const DEFAULT_AVD_FIRMWARE: &[u8] = include_bytes!(env!("SCARLET_APPLE_AVD_FW_BIN"));

const REG_TOP_GATE: usize = 0x1000000;
const REG_DART_TUNING0: usize = 0x1010060;
const REG_DART_TUNING1: usize = 0x1010068;
const REG_DART_TUNING2: usize = 0x101006c;
const REG_PIODMA_CONFIG: usize = 0x1070000;
const REG_PIODMA_BASE: usize = 0x1070024;
const REG_MCPU_CODE: usize = 0x1080000;
const REG_MCPU_SRAM: usize = 0x108c000;
const REG_DECODER_CONTROL_BASE: usize = 0x1100000;
const REG_MCPU_CONTROL: usize = 0x1098008;
const REG_MCPU_IRQ_ENABLE0: usize = 0x1098010;
const REG_MCPU_IRQ_ENABLE1: usize = 0x1098048;
const REG_MCPU_AP_ACK: usize = 0x1098050;
const REG_MAILBOX_AP_TO_CM3: usize = 0x1098054;
const REG_MCPU_AP_IRQ_CLEAR: usize = 0x109805c;
const REG_MAILBOX_CM3_TO_AP: usize = 0x1098064;
const REG_MCPU_CM3_ACK: usize = 0x1098068;
const REG_MCPU_CM3_IRQ_CLEAR: usize = 0x1098074;
const REG_MCPU_IRQ_ARM: usize = 0x109807c;
const REG_MCPU_IRQ_MASK: usize = 0x1098080;
const REG_MCPU_STATUS: usize = 0x1098090;
const REG_MCPU_CONTROL_98: usize = 0x1098098;
const REG_H264_INSTRUCTION: usize = 0x1104000;
const REG_H265_INSTRUCTION: usize = 0x1104004;
const REG_H264_MODE: usize = 0x110400c;
const REG_VP9_MODE: usize = 0x1104010;
const REG_H264_SUBMIT: usize = 0x1104014;
const REG_H264_COUNTER0: usize = 0x1104018;
const REG_H264_COUNTER1: usize = 0x110401c;
const REG_H264_COUNTER2: usize = 0x1104020;
const REG_H264_COUNTER3: usize = 0x1104024;
const REG_H264_COUNTER4: usize = 0x1104028;
const REG_H264_CONTROL0: usize = 0x1104034;
const REG_H264_CONTROL1: usize = 0x110403c;
const REG_H265_CONTROL: usize = 0x1104040;
const REG_H264_DMA_TRIGGER: usize = 0x1104048;
const REG_VP9_CONTROL: usize = 0x110404c;
const REG_H264_TIMEOUT: usize = 0x110405c;
const REG_H264_STATUS: usize = 0x1104060;
const REG_H264_STATUS_MASK: usize = 0x1104064;
const REG_H264_INST_FIFO_BASE: usize = 0x1104068;
const REG_H264_INST_FIFO_SIZE: usize = 0x1104084;
const REG_H264_INST_FIFO_READ: usize = 0x11040a0;
const REG_H264_INST_FIFO_WRITE: usize = 0x11040bc;
const REG_H264_PIPE_SELECT: usize = 0x11040f4;
const REG_H264_PIPE_CONTROL: usize = 0x1104110;
const REG_AVD_DMA_CONFIG_BASE: usize = 0x108ee90;
const REG_AVD_DMA_BASE: usize = 0x110c000;
const REG_AVD_DMA_CTRL0: usize = 0x110c010;
const REG_AVD_DMA_CTRL1: usize = 0x110c018;
const REG_AVD_DMA_IRQ_CLEAR0: usize = 0x110cc90;
const REG_AVD_DMA_IRQ_CLEAR1: usize = 0x110cc94;
const REG_AVD_DMA_IRQ_CLEAR2: usize = 0x110ccd0;
const REG_AVD_DMA_IRQ_CLEAR3: usize = 0x110ccd4;
const REG_AVD_DMA_IRQ_CLEAR4: usize = 0x110cac8;
const REG_WRAP_CONTROL: usize = 0x1400000;
const REG_WRAP_IDLE: usize = 0x1400014;
const REG_WRAP_INIT: usize = 0x1400018;

const MCPU_CONTROL_RESET: u32 = 0xe;
const MCPU_CONTROL_RUN: u32 = 0x1;
const H264_SUBMIT_START: u32 = 1;
const H264_SUBMIT_FRAME: u32 = 0x2b000107;
const H264_STATUS_DONE_MASK: u32 = 0x0084_2108;
const H264_STATUS_ERROR_MASK: u32 = 0x0000_0003;
const H264_STATUS_VIDEO_DONE: u32 = 0x0040_0000;
const H264_STATUS_POSTPROCESS_DONE: u32 = 0x0000_1000;
const AVD_TRACE_CAPACITY: usize = 128;
const AVD_DMA_GRANULE: usize = 0x4000;
// m1n1 clears 0xc000 bytes here. Extending the SRAM clear reaches the MCPU
// mailbox/control block at 0x1098000 and clears the run latch back to zero.
const AVD_MCPU_CODE_BYTES: usize = 0xc000;
const AVD_MCPU_SRAM_BYTES: usize = 0xc000;
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
const AVD_OUTPUT_SLOT_PAYLOAD_OFFSET: usize = AVD_DMA_GRANULE;
const AVD_MCPU_RUN_POLLS: usize = 100;
const AVD_MCPU_RUN_POLL_US: u64 = 10;
const AVD_DECODE_POLL_LIMIT: usize = 10_000;
const AVD_PMGR_CLOCK_GATE_PADDRS_PROPERTY: &str = "apple,pmgr-clock-gate-paddrs";

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

const AVD_DMA_STAGE0: &[(usize, u32)] = &[
    (0x110c044, 0x0000_0040),
    (0x110c084, 0x0040_0040),
    (0x110c244, 0x0080_0034),
    (0x110c284, 0x0000_0018),
    (0x110c2c4, 0x00b4_0020),
    (0x110c3c4, 0x00d4_0030),
    (0x110c404, 0x0018_0014),
    (0x110c444, 0x0104_001c),
    (0x110c484, 0x002c_0014),
    (0x110c4c4, 0x0120_0014),
    (0x110c504, 0x0040_0018),
    (0x110c544, 0x0134_0024),
    (0x110c584, 0x0058_0014),
    (0x110c5c4, 0x0158_0014),
    (0x110c1c4, 0x006c_0048),
    (0x110c204, 0x00b4_0048),
    (0x110c384, 0x00fc_0038),
    (0x110c604, 0x0134_0030),
    (0x110c644, 0x016c_00b0),
    (0x110c684, 0x021c_00b0),
    (0x110c844, 0x0164_001c),
    (0x110c884, 0x02cc_0028),
    (0x110c744, 0x0180_0018),
    (0x110c784, 0x02f4_0020),
    (0x110c7c4, 0x0198_0018),
    (0x110c804, 0x0314_001c),
    (0x110c8c4, 0x01b0_0024),
    (0x110c904, 0x0330_0040),
    (0x110c944, 0x01d4_001c),
    (0x110c984, 0x0370_002c),
    (0x110c9c4, 0x01f0_0030),
    (0x110ca04, 0x039c_003c),
    (0x110ca44, 0x0220_0014),
    (0x110ca84, 0x03d8_0014),
    (0x110cb04, 0x0234_0014),
    (0x110cb44, 0x03ec_0014),
    (0x110cac4, 0x0248_0080),
    (0x110cc8c, 0x02c8_0014),
    (0x110cccc, 0x02dc_0014),
    (0x110cc88, 0x02f0_0060),
    (0x110ccc8, 0x0350_0054),
    (0x110cb84, 0x03a4_001c),
    (0x110cbc4, 0x0400_0040),
    (0x110cc04, 0x03c0_0040),
    (0x110cc44, 0x0440_00c0),
];

const AVD_PMGR_BOOT_WRITES: &[(usize, u32)] = &[
    (0x000, 0x11),
    (0x00c, 0x0d),
    (0x010, 0x0c),
    (0x014, 0x01),
    (0x018, 0x01),
    (0x01c, 0x03),
    (0x020, 0x03),
    (0x024, 0x03),
    (0x028, 0x03),
    (0x02c, 0x03),
    (0x108, 0x11),
    (0x10c, 0x0d),
    (0x110, 0x0c),
    (0x114, 0x01),
    (0x118, 0x01),
    (0x11c, 0x03),
    (0x120, 0x03),
    (0x124, 0x03),
    (0x128, 0x03),
    (0x12c, 0x03),
    (0x400, 0xc0f1_0010),
    (0xa00, 0x01ff_ffff),
];

const AVD_CTRL_BOOT_TUNABLES: &[(usize, u32)] = &[
    (0x0008, 0x8000_0000),
    (0x1000, 0x8000_0000),
    (0x1100, 0x8000_0000),
    (0x1200, 0x8000_0000),
    (0x1300, 0x8000_0000),
    (0x1400, 0x8000_0000),
    (0x1500, 0x8000_0000),
    (0x1600, 0x8000_0000),
    (0x1700, 0x8000_0000),
    (0x1800, 0x8000_0000),
    (0x4000, 0x8000_0000),
    (0x4100, 0x8000_0000),
    (0x4200, 0x8000_0000),
    (0x4300, 0x8000_0000),
    (0x4400, 0x8000_0000),
    (0x4500, 0x8000_0000),
    (0x4600, 0x8000_0000),
    (0xc000, 0x0000_0001),
    (0xc080, 0x8001_07ff),
    (0xc084, 0x0000_0028),
    (0xc0c0, 0x8001_07ff),
    (0xc0c4, 0x0028_0028),
    (0xc100, 0x8001_07ff),
    (0xc104, 0x0050_0028),
    (0xc140, 0x8001_07ff),
    (0xc144, 0x0078_0028),
    (0xc180, 0x8001_07ff),
    (0xc184, 0x0052_0028),
    (0xc1c0, 0x8001_07ff),
    (0xc1c4, 0x007a_0028),
    (0xc200, 0x8001_07ff),
    (0xc204, 0x00a2_0028),
    (0xc240, 0x8001_07ff),
    (0xc244, 0x00ca_0028),
    (0xc280, 0x8001_07ff),
    (0xc284, 0x00a0_0020),
    (0xc2c0, 0x8001_07ff),
    (0xc2c4, 0x00c0_0020),
    (0xc300, 0x8001_07ff),
    (0xc304, 0x00e0_0020),
    (0xc340, 0x8001_07ff),
    (0xc344, 0x0100_0020),
    (0xc380, 0x8001_07ff),
    (0xc384, 0x0000_000a),
    (0xc3c0, 0x8001_07ff),
    (0xc3c4, 0x000a_000a),
    (0xc400, 0x8001_07ff),
    (0xc404, 0x0014_000a),
    (0xc440, 0x8001_07ff),
    (0xc444, 0x001e_000a),
    (0xc480, 0x8001_07ff),
    (0xc484, 0x0120_000c),
    (0xc4c0, 0x8001_07ff),
    (0xc4c4, 0x012c_000c),
    (0xc500, 0x8001_07ff),
    (0xc504, 0x0138_000c),
    (0xc540, 0x8001_07ff),
    (0xc544, 0x0144_000c),
    (0xc580, 0x8001_07ff),
    (0xc584, 0x00f2_0020),
    (0xc5c0, 0x8001_07ff),
    (0xc5c4, 0x0112_0020),
    (0xc600, 0x8001_07ff),
    (0xc604, 0x0132_0020),
    (0xc640, 0x8001_07ff),
    (0xc644, 0x0152_0020),
    (0xc680, 0x8001_07ff),
    (0xc684, 0x0150_0018),
    (0xc6c0, 0x8001_07ff),
    (0xc6c4, 0x0028_000a),
    (0xc700, 0x8001_07ff),
    (0xc704, 0x0168_000e),
    (0xc740, 0x8001_07ff),
    (0xc744, 0x0032_000a),
    (0xc780, 0x8001_07ff),
    (0xc784, 0x0176_000a),
    (0xc7c0, 0x8001_07ff),
    (0xc7c4, 0x003c_000c),
    (0xc800, 0x8001_07ff),
    (0xc804, 0x0180_0012),
    (0xc840, 0x8001_07ff),
    (0xc844, 0x0048_000a),
    (0xc880, 0x8001_07ff),
    (0xc884, 0x0192_000a),
    (0xc8c0, 0x8001_07ff),
    (0xc8c4, 0x0172_0018),
    (0xc900, 0x8001_13ff),
    (0xc904, 0x019c_011c),
    (0xc940, 0x8001_13ff),
    (0xc944, 0x02b8_011c),
    (0xc980, 0x8001_13ff),
    (0xc984, 0x03d4_011c),
    (0xc9c0, 0x8001_13ff),
    (0xc9c4, 0x04f0_011c),
    (0xe000, 0x0000_0001),
    (0xe080, 0x8002_07ff),
    (0xe084, 0x0000_001c),
    (0xe0c0, 0x8002_07ff),
    (0xe0c4, 0x0000_002e),
    (0xe100, 0x8002_07ff),
    (0xe104, 0x001c_0054),
    (0xe140, 0x8002_07ff),
    (0xe144, 0x002e_009e),
    (0xe180, 0x8002_07ff),
    (0xe184, 0x0070_0016),
    (0xe1c0, 0x8002_07ff),
    (0xe1c4, 0x00cc_0022),
    (0xe200, 0x8002_07ff),
    (0xe204, 0x0086_0020),
    (0xe240, 0x8002_07ff),
    (0xe244, 0x00ee_0020),
    (0xe280, 0x8002_07ff),
    (0xe284, 0x00a6_000c),
    (0xe2c0, 0x8002_07ff),
    (0xe2c4, 0x010e_000c),
    (0xe300, 0x8002_3fff),
    (0xe304, 0x00c0_00cc),
    (0xe340, 0x8002_07ff),
    (0xe344, 0x00b6_000a),
    (0xe380, 0x8002_07ff),
    (0xe384, 0x011a_000a),
    (0xe3c0, 0x8002_07ff),
    (0xe3c4, 0x018c_0012),
    (0xe400, 0x8002_07ff),
    (0xe404, 0x0124_0020),
    (0xe440, 0x8002_07ff),
    (0xe444, 0x019e_0068),
    (0xe480, 0x8002_07ff),
    (0xe484, 0x0144_0060),
    (0xe4c0, 0x8003_4072),
    (0xe4c8, 0x021e_009e),
    (0xe4cc, 0x0206_000c),
    (0xe500, 0x8005_6072),
    (0xe508, 0x02bc_009e),
    (0xe50c, 0x0212_000c),
    (0xe540, 0x8002_07ff),
    (0xe544, 0x00b2_0004),
    (0xe800, 0x0000_0001),
    (0xe880, 0x8002_07ff),
    (0xe884, 0x0000_0016),
    (0xe8c0, 0x8002_07ff),
    (0xe8c4, 0x0000_0022),
    (0xe900, 0x8002_07ff),
    (0xe904, 0x0016_0022),
    (0xe940, 0x8002_07ff),
    (0xe944, 0x0022_003a),
    (0xe980, 0x8077_0003),
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
        self.read32(REG_MCPU_STATUS)
    }

    fn irq_status(&self) -> u32 {
        self.read32(REG_MCPU_IRQ_ENABLE1)
    }

    fn mask_irqs(&self) {
        self.write32(REG_MCPU_IRQ_ENABLE0, 0);
        self.write32(REG_MCPU_IRQ_ENABLE1, 0);
    }

    fn enable_irqs(&self) {
        self.write32(REG_MCPU_IRQ_ENABLE0, 0x2);
        self.write32(REG_MCPU_IRQ_ENABLE1, 0x8);
    }

    fn hold_cm3_in_reset(&self) {
        self.write32(REG_MCPU_CONTROL, MCPU_CONTROL_RESET);
        self.mask_irqs();
    }

    fn run_cm3(&self) -> Result<(), &'static str> {
        self.write32(REG_MCPU_CONTROL, MCPU_CONTROL_RESET);
        self.mask_irqs();
        self.mask_irqs();
        self.write32(REG_MCPU_AP_ACK, 1);
        self.write32(REG_MCPU_CM3_ACK, 1);
        self.write32(REG_MCPU_AP_IRQ_CLEAR, 1);
        self.write32(REG_MCPU_CM3_IRQ_CLEAR, 1);
        self.enable_irqs();
        arch::io_wmb();
        self.write32(REG_MCPU_CONTROL, MCPU_CONTROL_RUN);
        arch::io_mb();

        for _ in 0..AVD_MCPU_RUN_POLLS {
            if self.status() == 1 {
                self.write32(REG_WRAP_IDLE, 0);
                return Ok(());
            }
            time::udelay(AVD_MCPU_RUN_POLL_US);
        }

        self.write32(REG_WRAP_IDLE, 0);
        Err("apple-avd: CM3 run status did not assert")
    }

    fn init_hardware(&self) {
        self.init_power_regs();
        self.write32(REG_TOP_GATE, 0xfff);
        self.init_dart_tuning();
        self.clear_window(REG_MCPU_CODE, AVD_MCPU_CODE_BYTES);
        self.clear_window(REG_MCPU_SRAM, AVD_MCPU_SRAM_BYTES);
        self.init_ctrl_tunables();
        self.init_wrapper();
        self.init_dma_stage0();
    }

    fn stage_firmware_image(&self, image: &[u8]) -> Result<(), &'static str> {
        if image.len() > AVD_MCPU_CODE_BYTES {
            return Err("apple-avd: firmware image exceeds CM3 code window");
        }
        self.clear_window(REG_MCPU_CODE, AVD_MCPU_CODE_BYTES);
        self.clear_window(REG_MCPU_SRAM, AVD_MCPU_SRAM_BYTES);
        self.write_buffer(REG_MCPU_CODE, image);
        arch::io_wmb();
        Ok(())
    }

    fn send_mailbox(&self, value: u32) {
        self.write32(REG_MAILBOX_AP_TO_CM3, value);
    }

    fn recv_mailbox(&self) -> u32 {
        self.read32(REG_MAILBOX_CM3_TO_AP)
    }

    fn recv_mailbox_status(&self) -> u32 {
        self.read32(REG_MCPU_AP_IRQ_CLEAR)
    }

    fn clear_recv_mailbox(&self) {
        self.write32(REG_MAILBOX_CM3_TO_AP, 0);
        self.write32(REG_MCPU_CM3_ACK, 1);
        arch::io_wmb();
    }

    fn log_boot_state(&self, label: &str) {
        println!(
            "[apple-avd] boot {} ctrl={:#x} status={:#x} irq0={:#x} irq1={:#x} irq_arm={:#x} irq_mask={:#x} mcpu98={:#x} ap_ack={:#x} cm3_ack={:#x} ap_clr={:#x} cm3_clr={:#x} ap2cm3={:#x} cm32ap_status={:#x} wrap_ctl={:#x} wrap_idle={:#x} wrap_init={:#x} top={:#x} dart=[{:#x},{:#x},{:#x}]",
            label,
            self.read32(REG_MCPU_CONTROL),
            self.read32(REG_MCPU_STATUS),
            self.read32(REG_MCPU_IRQ_ENABLE0),
            self.read32(REG_MCPU_IRQ_ENABLE1),
            self.read32(REG_MCPU_IRQ_ARM),
            self.read32(REG_MCPU_IRQ_MASK),
            self.read32(REG_MCPU_CONTROL_98),
            self.read32(REG_MCPU_AP_ACK),
            self.read32(REG_MCPU_CM3_ACK),
            self.read32(REG_MCPU_AP_IRQ_CLEAR),
            self.read32(REG_MCPU_CM3_IRQ_CLEAR),
            self.read32(REG_MAILBOX_AP_TO_CM3),
            self.recv_mailbox_status(),
            self.read32(REG_WRAP_CONTROL),
            self.read32(REG_WRAP_IDLE),
            self.read32(REG_WRAP_INIT),
            self.read32(REG_TOP_GATE),
            self.read32(REG_DART_TUNING0),
            self.read32(REG_DART_TUNING1),
            self.read32(REG_DART_TUNING2)
        );
    }

    fn log_code_window(&self, label: &str, image: &[u8]) {
        let image_sp = read_image_word(image, 0);
        let image_reset = read_image_word(image, 4);
        println!(
            "[apple-avd] boot {} image_len={} image_vec=[{:#x},{:#x}] code_vec=[{:#x},{:#x},{:#x},{:#x}] sram0={:#x}",
            label,
            image.len(),
            image_sp,
            image_reset,
            self.read32(REG_MCPU_CODE),
            self.read32(REG_MCPU_CODE + 4),
            self.read32(REG_MCPU_CODE + 8),
            self.read32(REG_MCPU_CODE + 12),
            self.read32(REG_MCPU_SRAM)
        );
    }

    fn init_h264_engine(&self) {
        self.write32(REG_H264_COUNTER0, 0x78);
        self.write32(REG_H264_COUNTER1, 0x78);
        self.write32(REG_H264_COUNTER2, 0x78);
        self.write32(REG_H264_COUNTER3, 0x78);
        self.write32(REG_H264_COUNTER4, 0x20);
        self.write32(REG_H264_CONTROL0, 0);
        self.write32(REG_H264_CONTROL1, 0);
        self.write32(REG_H265_CONTROL, 0);
        self.write32(REG_H264_DMA_TRIGGER, 0);
        self.write32(REG_VP9_CONTROL, 0);
        self.write32(
            REG_H264_TIMEOUT,
            self.read32(REG_H264_TIMEOUT) | 0x0050_0000,
        );
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
            self.write32(REG_H264_MODE, *word);
        }
    }

    fn submit_h264(&self) {
        self.write32(REG_H264_SUBMIT, H264_SUBMIT_FRAME);
    }

    fn clear_window(&self, offset: usize, len: usize) {
        for word_offset in (0..len).step_by(4) {
            self.write32(offset + word_offset, 0);
        }
    }

    fn write_buffer(&self, offset: usize, bytes: &[u8]) {
        let mut index = 0;
        while index < bytes.len() {
            let mut word = [0u8; 4];
            let end = (index + 4).min(bytes.len());
            word[..end - index].copy_from_slice(&bytes[index..end]);
            self.write32(offset + index, u32::from_le_bytes(word));
            index += 4;
        }
    }

    fn init_wrapper(&self) {
        self.write32(REG_WRAP_IDLE, 1);
        self.write32(REG_WRAP_INIT, 1);
        self.write32(REG_DECODER_CONTROL_BASE + 0x14c, 0x14);
        self.write32(REG_DECODER_CONTROL_BASE + 0xe4d0, 0);
        self.write32(REG_DECODER_CONTROL_BASE + 0xe4d4, 0);
        self.write32(REG_DECODER_CONTROL_BASE + 0xe510, 0);
        self.write32(REG_DECODER_CONTROL_BASE + 0xe514, 0);
        self.write32(REG_DECODER_CONTROL_BASE + 0xe308, u32::MAX);
        self.write32(REG_DECODER_CONTROL_BASE + 0xe300, 0x8002_3fff);
        self.write32(REG_DECODER_CONTROL_BASE + 0xc900, 0x8001_13ff);
        self.write32(REG_DECODER_CONTROL_BASE + 0xc940, 0x8001_13ff);
        self.write32(REG_DECODER_CONTROL_BASE + 0xc980, 0x8001_13ff);
        self.write32(REG_DECODER_CONTROL_BASE + 0xc9c0, 0x8001_13ff);
        self.write32(REG_PIODMA_CONFIG, 0);
        self.write32(REG_H264_STATUS_MASK, 0x3);
        self.write32(REG_AVD_DMA_IRQ_CLEAR0, u32::MAX);
        self.write32(REG_AVD_DMA_IRQ_CLEAR1, u32::MAX);
        self.write32(REG_AVD_DMA_IRQ_CLEAR2, u32::MAX);
        self.write32(REG_AVD_DMA_IRQ_CLEAR3, u32::MAX);
        self.write32(REG_AVD_DMA_IRQ_CLEAR4, u32::MAX);
        self.write32(REG_PIODMA_BASE, 0x26907000);
        self.write32(REG_WRAP_IDLE, 0);
    }

    fn init_power_regs(&self) {
        for (offset, value) in AVD_PMGR_BOOT_WRITES.iter().copied() {
            self.write32(offset, value);
        }
    }

    fn init_dart_tuning(&self) {
        self.write32(REG_DART_TUNING0, 0x8001_6100);
        self.write32(REG_DART_TUNING1, 0x000f_0f0f);
        self.write32(REG_DART_TUNING2, 0x0008_0808);
    }

    fn init_ctrl_tunables(&self) {
        for (offset, value) in AVD_CTRL_BOOT_TUNABLES.iter().copied() {
            self.write32(REG_DECODER_CONTROL_BASE + offset, value);
        }
    }

    fn init_dma_stage0(&self) {
        self.write32(REG_PIODMA_BASE, 0x26907000);
        self.write32(REG_WRAP_CONTROL, 0x3);
        self.write32(REG_H264_INSTRUCTION, 0);
        self.write32(REG_H264_TIMEOUT, 0);
        self.write32(REG_H264_PIPE_CONTROL, 0);
        self.write32(REG_H264_PIPE_SELECT, 0x1555);

        for offset in (0x1100000..=0x110b000).step_by(0x1000) {
            self.write32(offset, 0xc000_0000);
        }
        self.write32(REG_AVD_DMA_CTRL0, 1);
        self.write32(REG_AVD_DMA_CTRL1, 1);
        for offset in (0x40..=0xc80).step_by(0x40) {
            let reg = REG_AVD_DMA_BASE + offset;
            self.write32(reg, self.read32(reg) | 0xc000_0000);
        }
        let last_dma_reg = REG_AVD_DMA_BASE + 0xd00;
        self.write32(last_dma_reg, self.read32(last_dma_reg) | 0xc000_0003);

        for (offset, value) in AVD_DMA_STAGE0.iter().copied() {
            self.write32(offset, value);
        }

        self.write32(
            REG_H264_TIMEOUT,
            self.read32(REG_H264_TIMEOUT) | 0x0050_0000,
        );
        self.write32(REG_MCPU_IRQ_ARM, 1);
        self.write32(REG_MCPU_IRQ_MASK, u32::MAX);
    }
}

/// Snapshot of Apple AVD debug status registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvdStatusSnapshot {
    /// Top-level AVD status register.
    pub status: u32,
    /// Top-level AVD IRQ status register.
    pub irq_status: u32,
    /// CM3-to-AP mailbox status bits.
    pub mailbox: u32,
}

struct AvdFirmwareImage {
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
    power: Option<PmgrDomain>,
    reset: Option<ResetHandle>,
    mailbox: AvdFirmwareMailbox,
    trace: AvdTraceLog,
    firmware_state: AvdFirmwareState,
    firmware_image: Option<AvdFirmwareImage>,
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
        power: Option<PmgrDomain>,
        reset: Option<ResetHandle>,
    ) -> Self {
        Self {
            name,
            soc,
            paddr,
            size,
            irq,
            registers,
            dma,
            power,
            reset,
            mailbox: AvdFirmwareMailbox::new(),
            trace: AvdTraceLog::new(AVD_TRACE_CAPACITY),
            firmware_state: AvdFirmwareState::Missing,
            firmware_image: None,
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
        None
    }

    /// Return the staged firmware image size.
    ///
    /// # Returns
    ///
    /// Firmware image byte length when an image is currently mapped.
    pub fn firmware_image_size(&self) -> Option<usize> {
        self.firmware_image.as_ref().map(|image| image.size)
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

    /// Ensure the bundled firmware is running before a hardware decode submit.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the firmware is already running or was started
    /// successfully.
    pub fn ensure_firmware_running(&mut self) -> Result<(), &'static str> {
        if self.firmware_state == AvdFirmwareState::Running {
            return Ok(());
        }
        if self.firmware_state == AvdFirmwareState::Faulted {
            return Err("apple-avd: firmware is faulted");
        }

        self.init_h264_engine();
        self.boot_firmware(DEFAULT_AVD_FIRMWARE)
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
        validate_firmware_image(image)?;
        println!(
            "[apple-avd] boot begin firmware_len={} power_on={} reset_source={}",
            image.len(),
            self.power
                .as_ref()
                .map(|power| power.is_on())
                .unwrap_or(true),
            self.reset_source()
        );
        self.prepare_for_firmware(image)?;
        self.start_firmware()?;
        self.firmware_image = Some(AvdFirmwareImage { size: image.len() });

        if let Some(message) = self.poll_firmware_message() {
            if message.is_fault() {
                return Err("apple-avd: firmware faulted");
            }
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
        self.registers
            .clear_h264_status(H264_STATUS_DONE_MASK | H264_STATUS_ERROR_MASK);
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
            mailbox: self.registers.recv_mailbox_status(),
        }
    }

    fn prepare_for_firmware(&mut self, image: &[u8]) -> Result<(), &'static str> {
        self.reset_hardware()?;
        self.registers.log_boot_state("before-init");
        self.registers.init_hardware();
        self.registers.log_boot_state("after-init");
        self.registers.hold_cm3_in_reset();
        self.registers.log_boot_state("after-hold-reset");
        self.registers.stage_firmware_image(image)?;
        self.registers.log_code_window("after-stage", image);
        self.registers.clear_recv_mailbox();
        self.registers.log_boot_state("after-clear-mailbox");
        self.firmware_state = AvdFirmwareState::Staged;
        self.trace.push(
            AvdTraceKind::Firmware,
            REG_MCPU_CODE as u64,
            image.len() as u64,
        );
        Ok(())
    }

    fn start_firmware(&mut self) -> Result<(), &'static str> {
        self.registers.enable_irqs();
        self.registers.log_boot_state("after-enable-irqs");
        if let Err(e) = self.registers.run_cm3() {
            self.registers.log_boot_state("after-run-timeout");
            self.mark_firmware_faulted();
            self.trace.push(AvdTraceKind::Fault, 0x1098_0090, 0);
            println!("[apple-avd] boot {}", e);
            return Err(e);
        }
        self.registers.log_boot_state("after-run");
        self.firmware_state = AvdFirmwareState::Running;
        self.trace.push(AvdTraceKind::Firmware, 1, 0);
        Ok(())
    }

    fn reset_hardware(&mut self) -> Result<(), &'static str> {
        if let Some(power) = &self.power {
            if !power.is_on() {
                power.enable()?;
                time::udelay(10);
            }
        }

        if let Some(reset) = &self.reset {
            reset.reset()?;
            time::udelay(10);
        } else {
            println!("[apple-avd] reset unavailable; skipping PMGR reset fallback");
        }

        self.firmware_state = AvdFirmwareState::Missing;
        self.trace.push(AvdTraceKind::Firmware, 0, 0);
        Ok(())
    }

    fn reset_source(&self) -> &'static str {
        if self.reset.is_some() { "fdt" } else { "none" }
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
    output_pool: Option<AvdMappedOutputPool>,
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
            output_pool: None,
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
        self.output_pool = None;
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

    fn prepare_mapped_output(
        &mut self,
        avd: &AppleAvd,
        request: &VideoBackendDecodeRequest,
        layout: h264::AvdFrameLayout,
    ) -> Result<AvdReferenceOutput, &'static str> {
        let payload_len = layout.output_len();
        let slot_span = AVD_OUTPUT_SLOT_PAYLOAD_OFFSET
            .checked_add(payload_len)
            .map(|len| align_up(len, AVD_DMA_GRANULE))
            .ok_or("apple-avd: output slot size overflow")?;
        let required = slot_span
            .checked_mul(AVD_REFERENCE_SLOT_COUNT)
            .ok_or("apple-avd: reference slot size overflow")?;
        if payload_len == 0 || required > request.output_len as usize {
            return Err("apple-avd: decoded frame exceeds mapped output slot pool");
        }

        let granule = avd.dma_context().mapping_granule().max(AVD_DMA_GRANULE);
        let output_map_len = align_up(request.output_len as usize, granule);
        let remap_output = self.output_pool.as_ref().is_none_or(|pool| {
            pool.paddr != request.output_paddr
                || pool.vaddr != request.output_vaddr
                || pool.len != output_map_len
        });
        if remap_output {
            let mapping = avd
                .dma_context()
                .map_phys_owned(
                    request.output_paddr,
                    output_map_len,
                    IommuMapFlags::READ | IommuMapFlags::WRITE | IommuMapFlags::COHERENT,
                )
                .map_err(|_| "apple-avd: output DMA map failed")?;
            self.output_pool = Some(AvdMappedOutputPool {
                paddr: request.output_paddr,
                vaddr: request.output_vaddr,
                len: output_map_len,
                mapping,
            });
            self.reference_frames.clear();
            self.next_reference_slot = 0;
            self.reference_slot_len = 0;
        }

        if self.reference_slot_len != slot_span {
            self.reference_frames.clear();
            self.next_reference_slot = 0;
            self.reference_slot_len = slot_span;
        }

        let slot = self.next_reference_slot % AVD_REFERENCE_SLOT_COUNT;
        self.next_reference_slot = (slot + 1) % AVD_REFERENCE_SLOT_COUNT;
        let slot_offset = slot
            .checked_mul(slot_span)
            .ok_or("apple-avd: output slot offset overflow")?;
        let payload_offset_in_output = slot_offset
            .checked_add(AVD_OUTPUT_SLOT_PAYLOAD_OFFSET)
            .ok_or("apple-avd: output payload offset overflow")?;
        let payload_end = payload_offset_in_output
            .checked_add(payload_len)
            .ok_or("apple-avd: output payload end overflow")?;
        if payload_end > request.output_len as usize {
            return Err("apple-avd: output payload exceeds mapped output slot pool");
        }
        let header_offset_in_output = payload_offset_in_output
            .checked_sub(SCARLET_VIDEO_FRAME_HEADER_LEN)
            .ok_or("apple-avd: output header offset underflow")?;
        let output_pool = self
            .output_pool
            .as_ref()
            .ok_or("apple-avd: mapped output pool unavailable")?;
        Ok(AvdReferenceOutput {
            slot,
            dma_addr: output_pool.mapping.dma_addr() + payload_offset_in_output as u64,
            vaddr: output_pool.vaddr + payload_offset_in_output,
            header_vaddr: output_pool.vaddr + header_offset_in_output,
            payload_offset: request.output_offset as usize + payload_offset_in_output,
            len: payload_len,
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

struct AvdMappedOutputPool {
    paddr: usize,
    vaddr: usize,
    len: usize,
    mapping: DmaMapping,
}

#[derive(Clone, Copy)]
struct AvdReferenceOutput {
    slot: usize,
    dma_addr: u64,
    vaddr: usize,
    header_vaddr: usize,
    payload_offset: usize,
    len: usize,
}

struct AvdPendingDecode {
    stream_id: u32,
    frame_number: u32,
    timestamp: u64,
    layout: h264::AvdFrameLayout,
    payload_len: usize,
    output_header_vaddr: usize,
    output_payload_vaddr: usize,
    output_payload_offset: usize,
    output_payload_len: usize,
    reference_slot: usize,
    store_reference: bool,
    is_idr: bool,
    status_before: u32,
    command_tag: u32,
    input_mapping: DmaMapping,
    poll_count: usize,
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
        let Some(front) = state.pending.front_mut() else {
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
        } else {
            front.poll_count = front.poll_count.saturating_add(1);
            if front.poll_count > AVD_DECODE_POLL_LIMIT {
                let _ = state.pending.pop_front();
                avd.recover_h264_engine(status as u64);
                return Err("apple-avd: decode timed out");
            }
        }
        Ok(())
    }
}

impl VideoDecodeBackend for AppleAvdVideoBackend {
    fn name(&self) -> &'static str {
        "apple-avd"
    }

    fn debug_status(&self) -> Option<String> {
        let avd = self.avd().ok()?;
        let avd = avd.lock();
        let snapshot = avd.debug_snapshot();
        let h264_status = avd.h264_status();
        let firmware = avd.firmware_state_name();
        drop(avd);

        let state = self.state.lock();
        Some(format!(
            " fw={} pending={} completed={} h264_status={:#x} status={:#x} irq_status={:#x} mailbox={:#x}",
            firmware,
            state.pending.len(),
            state.completed.len(),
            h264_status,
            snapshot.status,
            snapshot.irq_status,
            snapshot.mailbox
        ))
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
        avd.ensure_firmware_running()?;

        let granule = avd.dma_context().mapping_granule().max(PAGE_SIZE);
        let input_vaddr = request.input_vaddr;
        let input_len = request.input_len as usize;
        let input_map_len = align_up(input_len, granule);
        arch::clean_invalidate_dcache_to_poc_range(input_vaddr, input_map_len);

        // SAFETY: `/dev/videoN` passes a PMM-backed kernel mapping for the
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
        let input_mapping = avd
            .dma_context()
            .map_phys_owned(
                request.input_paddr,
                input_map_len,
                IommuMapFlags::READ | IommuMapFlags::COHERENT,
            )
            .map_err(|_| "apple-avd: input DMA map failed")?;

        let frame_number = session.next_frame;
        session.next_frame = session.next_frame.wrapping_add(1);
        let reference_output = session.prepare_mapped_output(&avd, request, layout)?;
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
                len: payload_len,
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

        arch::clean_invalidate_dcache_to_poc_range(reference_output.vaddr, reference_output.len);

        let status_before = avd.submit_h264_mmio(&decode_request, &instructions)?;
        let command_tag = avd.submit_h264_request(&decode_request)?;
        avd.trace.push(
            AvdTraceKind::DecodeSubmit,
            input_mapping.dma_addr(),
            inst_len as u64,
        );
        state.pending.push_back(AvdPendingDecode {
            stream_id: request.stream_id,
            frame_number,
            timestamp: request.timestamp,
            layout,
            payload_len,
            output_header_vaddr: reference_output.header_vaddr,
            output_payload_vaddr: reference_output.vaddr,
            output_payload_offset: reference_output.payload_offset,
            output_payload_len: reference_output.len,
            reference_slot: reference_output.slot,
            store_reference: decode_request.slice.is_reference(),
            is_idr: decode_request.slice.is_idr(),
            status_before,
            command_tag,
            input_mapping,
            poll_count: 0,
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
    arch::invalidate_dcache_to_poc_range(pending.output_payload_vaddr, pending.output_payload_len);
    write_frame_header(
        pending.output_header_vaddr,
        pending.layout.width,
        pending.layout.height,
        pending.layout.pixel_format,
        pending.payload_len as u32,
    )?;
    arch::clean_dcache_to_poc_range(pending.output_header_vaddr, SCARLET_VIDEO_FRAME_HEADER_LEN);

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
            payload_offset: pending.output_payload_offset as u64,
            payload_len: pending.payload_len as u32,
            flags: pending.command_tag,
            timestamp: pending.timestamp,
        },
    });
    let _ = pending.input_mapping.dma_addr();
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

fn resolve_avd_reset(device: &PlatformDeviceInfo) -> Result<Option<ResetHandle>, &'static str> {
    match DeviceManager::get_manager().resolve_reset_by_index(device, 0) {
        Ok(reset) => Ok(Some(reset)),
        Err(e) if is_probe_defer(e) => probe_defer(),
        Err(e @ ("reset: resets missing" | "reset: index out of range")) => {
            println!("[apple-avd] reset unavailable: {}", e);
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

fn resolve_avd_power_domain() -> Result<Option<PmgrDomain>, &'static str> {
    match pmgr_get_domain_by_label("avd_sys") {
        Ok(power) => {
            let was_on = power.is_on();
            power.enable()?;
            println!(
                "[apple-avd] power domain '{}' enabled before={} after={}",
                power.label(),
                was_on,
                power.is_on()
            );
            Ok(Some(power))
        }
        Err(e @ "pmgr: registry not initialized") => {
            println!("[apple-avd] PMGR not ready for avd_sys: {}", e);
            probe_defer()
        }
        Err(e @ "pmgr: domain label not found") => {
            println!("[apple-avd] power domain avd_sys unavailable: {}", e);
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

fn read_be_u64_cells(bytes: &[u8]) -> Result<Vec<usize>, &'static str> {
    if bytes.len() % 8 != 0 {
        return Err("apple-avd: malformed PMGR clock gate paddr property");
    }

    let mut out = Vec::new();
    for chunk in bytes.chunks_exact(8) {
        let word: [u8; 8] = chunk
            .try_into()
            .map_err(|_| "apple-avd: malformed PMGR clock gate paddr property")?;
        let raw = u64::from_be_bytes(word);
        out.push(
            usize::try_from(raw).map_err(|_| "apple-avd: PMGR clock gate paddr out of range")?,
        );
    }
    Ok(out)
}

fn enable_adt_pmgr_clock_gates(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let Some(property) = device.property(AVD_PMGR_CLOCK_GATE_PADDRS_PROPERTY) else {
        println!(
            "[apple-avd] PMGR ADT clock gate property '{}' missing",
            AVD_PMGR_CLOCK_GATE_PADDRS_PROPERTY
        );
        return Ok(());
    };
    let paddrs = read_be_u64_cells(property.value())?;
    if paddrs.is_empty() {
        println!("[apple-avd] PMGR ADT clock gate property is empty");
        return Ok(());
    }

    for paddr in paddrs {
        match pmgr_get_domain_by_register_paddr(paddr) {
            Ok(domain) => {
                let was_on = domain.is_on();
                domain.enable()?;
                println!(
                    "[apple-avd] PMGR ADT clock gate '{}' paddr={:#x} enabled before={} after={}",
                    domain.label(),
                    paddr,
                    was_on,
                    domain.is_on()
                );
            }
            Err(e @ "pmgr: registry not initialized") => {
                println!(
                    "[apple-avd] PMGR not ready for ADT clock gate {:#x}: {}",
                    paddr, e
                );
                return probe_defer();
            }
            Err(e @ "pmgr: register paddr not found") => {
                println!(
                    "[apple-avd] PMGR ADT clock gate paddr={:#x} unavailable: {}",
                    paddr, e
                );
                return Err("apple-avd: PMGR ADT clock gate missing");
            }
            Err(e) => return Err(e),
        }
    }

    Ok(())
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
    enable_adt_pmgr_clock_gates(device)?;
    let power = resolve_avd_power_domain()?;
    let reset = resolve_avd_reset(device)?;
    let has_reset = reset.is_some();
    let mut avd = AppleAvd::new(
        device.name(),
        soc,
        paddr,
        size,
        irq,
        AvdRegisters::new(vaddr),
        dma,
        power,
        reset,
    );
    let snapshot = avd.snapshot();
    avd.trace
        .push(AvdTraceKind::Probe, paddr as u64, size as u64);
    avd.registers.mask_irqs();
    avd.registers.hold_cm3_in_reset();
    match avd.boot_firmware(DEFAULT_AVD_FIRMWARE) {
        Ok(()) => println!("[apple-avd] probe firmware boot succeeded"),
        Err(e) => {
            println!("[apple-avd] probe firmware boot failed: {}", e);
            return Err(e);
        }
    }
    let id = register_avd(avd);
    let backend: Arc<dyn VideoDecodeBackend> = Arc::new(AppleAvdVideoBackend::new(id));
    let backend_id = register_video_backend(Arc::clone(&backend));
    let video_name = register_video_decode_device(Arc::clone(&backend));
    #[cfg(feature = "debug-device")]
    debug_device::register_avd_debug_device(id, Arc::clone(&backend));

    println!(
        "[apple-avd] registered {} id={} backend={} video={} soc={} mmio={:#x}+{:#x} irq={:?} reset={} status={:#x} irq_status={:#x}",
        device.name(),
        id,
        backend_id,
        video_name,
        soc.name(),
        paddr,
        size,
        irq,
        has_reset,
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

fn read_image_word(image: &[u8], offset: usize) -> u32 {
    let Some(bytes) = image.get(offset..offset + 4) else {
        return 0;
    };
    u32::from_le_bytes(bytes.try_into().expect("firmware word bytes"))
}

fn validate_firmware_image(image: &[u8]) -> Result<(), &'static str> {
    if image.len() < 8 {
        return Err("apple-avd: firmware image is too small");
    }
    if image.get(0..4) == Some(b"\x7fELF") {
        return Err("apple-avd: firmware image is still an ELF file");
    }
    if image.len() > AVD_MCPU_CODE_BYTES {
        return Err("apple-avd: firmware image exceeds CM3 code window");
    }

    let stack_pointer = u32::from_le_bytes(image[0..4].try_into().expect("stack pointer bytes"));
    if (stack_pointer & 0xff00_0000) != 0x1000_0000 {
        return Err("apple-avd: firmware image has invalid stack pointer");
    }

    let reset_vector = u32::from_le_bytes(image[4..8].try_into().expect("reset vector bytes"));
    let reset_addr = (reset_vector & !1) as usize;
    if (reset_vector & 1) == 0 || reset_addr >= image.len() {
        return Err("apple-avd: firmware image has invalid reset vector");
    }

    Ok(())
}
