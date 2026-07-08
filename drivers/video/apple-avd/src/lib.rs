#![no_std]
#![allow(dead_code)]

extern crate alloc;

mod common;
mod debug;
mod debug_device;
mod firmware;
pub mod h264;
pub mod vp9;

use alloc::{
    boxed::Box,
    format,
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};

pub use common::AvdDmaRange;
pub use debug::{AvdTraceEvent, AvdTraceKind};
pub use firmware::AvdFirmwareMessage;

use debug::AvdTraceLog;
use firmware::AvdFirmwareMailbox;
use h264::{
    AvdH264InstructionStream, AvdH264ReferencePicture, AvdH264Workspace, H264DecodeRequest,
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
            SCARLET_VIDEO_FORMAT_H264, SCARLET_VIDEO_FORMAT_VP9, SCARLET_VIDEO_FRAME_HEADER_LEN,
            SCARLET_VIDEO_FRAME_MAGIC, SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR,
            SCARLET_VIDEO_H264_DPB_FLAG_LONG_TERM, SCARLET_VIDEO_H264_DPB_FLAG_VALID,
            SCARLET_VIDEO_PIXEL_FORMAT_NV12, SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME,
            SCARLET_VIDEO_VP9_PROBABILITY_BYTES, ScarletVideoDequeuedFrame,
            ScarletVideoH264DpbEntry, ScarletVideoH264StatelessParams,
            ScarletVideoVp9StatelessParams, VideoBackendCapabilities, VideoBackendDecodeRequest,
            VideoBackendDecodedFrame, VideoBackendH264StatelessRequest,
            VideoBackendVp9StatelessRequest, VideoCompletionNotifier, VideoDecodeBackend,
            register_video_backend, register_video_decode_device,
        },
    },
    environment::PAGE_SIZE,
    interrupt::{
        InterruptClaim, InterruptError, InterruptId, InterruptResult, InterruptSource,
        MaskableInterruptSource,
    },
    mem::page::ContiguousPages,
    println,
    sync::{IrqGuard, Mutex},
    time, vm,
};
use scarlet_driver_apple_pmgr::{
    PmgrDomain, pmgr_get_domain_by_label, pmgr_get_domain_by_register_paddr,
};
use vp9::{
    AvdVp9InstructionStream, AvdVp9ReferencePicture, AvdVp9Workspace, Vp9DecodeRequest,
    Vp9FrontendError, Vp9StreamParameters,
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
const REG_MCPU_IRQ_CLEAR: usize = 0x109804c;
const REG_MCPU_AP_ACK: usize = 0x1098050;
const REG_MCPU_DECODE_DMA_CONFIG: usize = 0x1098054;
const REG_MAILBOX_AP_TO_CM3: usize = 0x1098058;
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
const REG_DECODE_SUBMIT: usize = 0x1104014;
const REG_DECODE_COUNTER0: usize = 0x1104018;
const REG_DECODE_COUNTER1: usize = 0x110401c;
const REG_DECODE_COUNTER2: usize = 0x1104020;
const REG_DECODE_COUNTER3: usize = 0x1104024;
const REG_DECODE_COUNTER4: usize = 0x1104028;
const REG_DECODE_CONTROL0: usize = 0x1104034;
const REG_DECODE_CONTROL1: usize = 0x110403c;
const REG_H265_CONTROL: usize = 0x1104040;
const REG_DECODE_DMA_TRIGGER: usize = 0x1104048;
const REG_VP9_CONTROL: usize = 0x110404c;
const REG_DECODE_TIMEOUT: usize = 0x110405c;
const REG_DECODE_STATUS: usize = 0x1104060;
const REG_DECODE_STATUS_MASK: usize = 0x1104064;
const REG_DECODE_INST_FIFO_BASE: usize = 0x1104068;
const REG_DECODE_INST_FIFO_SIZE: usize = 0x1104084;
const REG_DECODE_INST_FIFO_READ: usize = 0x11040a0;
const REG_DECODE_INST_FIFO_WRITE: usize = 0x11040bc;
const REG_DECODE_PIPE_SELECT: usize = 0x11040f4;
const REG_DECODE_PIPE_CONTROL: usize = 0x1104110;
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
const MCPU_DECODE_DMA_CONFIG: u32 = 0x108e_b30;
const MCPU_IRQ_ENABLE1_PENDING: u32 = 0x1;
const MCPU_MAILBOX1_NOT_EMPTY: u32 = 0x8;
const H264_SUBMIT_START: u32 = 1;
const H264_SUBMIT_FRAME: u32 = 0x2b000107;
const VP9_SUBMIT_FRAME: u32 = 0x2bfff107;
const VP9_SUBMIT_TILE: u32 = 0x2bfff007;
const DECODE_STATUS_DONE_MASK: u32 = 0x0084_2108;
const DECODE_STATUS_ERROR_MASK: u32 = 0x0000_0003;
const DECODE_STATUS_ACCEPTED: u32 = 0x0000_0800;
const DECODE_STATUS_VIDEO_PHASE_MASK: u32 = 0x00c0_0000;
const DECODE_STATUS_POSTPROCESS_PHASE_MASK: u32 = 0x0000_3000;
const DECODE_STATUS_POSTPROCESS_DONE: u32 = 0x0000_2000;
const DECODE_STATUS_VP9_VIDEO_DONE_CLEAR: u32 = 0x0002_0000;
const DECODE_STATUS_POSTPROCESS_DONE_CLEAR: u32 = 0x0040_0000;
const DECODE_STATUS_RECOVERY_CLEAR_MASK: u32 = DECODE_STATUS_DONE_MASK
    | DECODE_STATUS_ERROR_MASK
    | DECODE_STATUS_ACCEPTED
    | DECODE_STATUS_VIDEO_PHASE_MASK
    | DECODE_STATUS_POSTPROCESS_PHASE_MASK
    | 0x0200_0000;
const DECODE_VP_CM3_MASK: u32 = 0x7;
const DECODE_PP_CM3_MASK: u32 = 0x5;
const DECODE_T8103_H264_VP_SLOT: u32 = 2;
const DECODE_T8103_VP9_VP_SLOT: u32 = 3;
const DECODE_T8103_FIFO_SLOT: u32 = 0;
const DECODE_T8103_FIFO_COUNT: u32 = 7;
const AVD_TRACE_CAPACITY: usize = 128;
const AVD_DECODE_TRACE_FRAMES: u32 = 4;
const AVD_DECODE_PROGRESS_INTERVAL: u32 = 0;
const AVD_OUTPUT_SAMPLE_BYTES: usize = 4096;
const AVD_OUTPUT_UV_SAMPLE_BYTES: usize = 256;
const AVD_DMA_GRANULE: usize = 0x4000;
// m1n1 clears 0xc000 bytes here. Extending the SRAM clear reaches the MCPU
// mailbox/control block at 0x1098000 and clears the run latch back to zero.
const AVD_MCPU_CODE_BYTES: usize = 0xc000;
const AVD_MCPU_SRAM_BYTES: usize = 0xc000;
const AVD_MAPPED_INPUT_BYTES: usize = 8 * 1024 * 1024;
const AVD_MAPPED_OUTPUT_BYTES: usize = 64 * 1024 * 1024;
const AVD_MAX_SESSIONS: usize = 4;
const AVD_WORKSPACE_ALIGN: usize = AVD_DMA_GRANULE;
const AVD_WORKSPACE_INST_FIFO_OFFSET: usize = 0x4000;
const AVD_WORKSPACE_INST_FIFO_BYTES: usize = 0x100000;
const AVD_WORKSPACE_PPS_TILE_OFFSET: usize = 0x140000;
const AVD_WORKSPACE_VP9_PROBS_OFFSET: usize = 0x180000;
const AVD_WORKSPACE_VP9_PPS0_OFFSET: usize = 0x190000;
const AVD_WORKSPACE_VP9_PPS1_OFFSET: usize = 0x198000;
const AVD_WORKSPACE_VP9_PPS2_OFFSET: usize = 0x1d8000;
const AVD_WORKSPACE_REFERENCE_OFFSET: usize = 0x400000;
const AVD_REFERENCE_FRAME_TABLE_LEN: usize = 16;
const AVD_H264_EXTRA_DECODE_SLOTS: usize = 1;
const AVD_H264_OUTPUT_SLOTS: usize = AVD_REFERENCE_FRAME_TABLE_LEN + AVD_H264_EXTRA_DECODE_SLOTS;
const AVD_VP9_MAX_REFERENCE_FRAMES: usize = 8;
const AVD_VP9_REFERENCE_SLOTS: usize = 4;
const AVD_MAX_OUTPUT_SLOTS: usize = if AVD_H264_OUTPUT_SLOTS > AVD_VP9_REFERENCE_SLOTS {
    AVD_H264_OUTPUT_SLOTS
} else {
    AVD_VP9_REFERENCE_SLOTS
};
const AVD_COMPLETED_FRAME_QUEUE_LEN: usize = AVD_MAX_SESSIONS * AVD_MAX_OUTPUT_SLOTS;
const AVD_OUTPUT_SLOT_PAYLOAD_OFFSET: usize = AVD_DMA_GRANULE;
const AVD_MCPU_RUN_POLLS: usize = 100;
const AVD_MCPU_RUN_POLL_US: u64 = 10;
const AVD_DECODE_POLL_LIMIT: usize = 10_000;
const AVD_MAILBOX_DRAIN_POLLS: usize = 64;
const AVD_MAILBOX_DRAIN_IDLE_POLLS: usize = 4;
const AVD_MAILBOX_DRAIN_POLL_US: u64 = 10;
const AVD_PMGR_CLOCK_GATE_PADDRS_PROPERTY: &str = "apple,pmgr-clock-gate-paddrs";

const AVD_DECODE_DMA_CONFIG: [u32; 30] = [
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AvdDecodePipe {
    H264,
    Vp9,
}

impl AvdDecodePipe {
    fn control_register(self) -> usize {
        match self {
            Self::H264 => REG_DECODE_DMA_TRIGGER,
            Self::Vp9 => REG_VP9_CONTROL,
        }
    }

    fn vp_slot(self) -> u32 {
        match self {
            Self::H264 => DECODE_T8103_H264_VP_SLOT,
            Self::Vp9 => DECODE_T8103_VP9_VP_SLOT,
        }
    }
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

    fn irq_enable_status1(&self) -> u32 {
        self.read32(REG_MCPU_IRQ_ENABLE1)
    }

    fn mask_irqs(&self) {
        self.write32(REG_MCPU_IRQ_ARM, 0);
        self.write32(REG_MCPU_IRQ_ENABLE0, 0);
        self.write32(REG_MCPU_IRQ_ENABLE1, 0);
        arch::io_wmb();
    }

    fn enable_irqs(&self) {
        self.write32(REG_MCPU_IRQ_ARM, 1);
        self.write32(REG_MCPU_IRQ_ENABLE0, 0x2);
        self.write32(REG_MCPU_IRQ_ENABLE1, 0x8);
        arch::io_wmb();
    }

    fn clear_irq_latches(&self) {
        self.write32(REG_MCPU_AP_IRQ_CLEAR, 1);
        self.write32(REG_MCPU_CM3_IRQ_CLEAR, 1);
        self.write32(REG_MCPU_IRQ_CLEAR, MCPU_MAILBOX1_NOT_EMPTY);
        arch::io_wmb();
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
        self.mask_irqs();
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
        arch::io_wmb();
    }

    fn recv_mailbox(&self) -> u32 {
        self.read32(REG_MAILBOX_CM3_TO_AP)
    }

    fn recv_mailbox_status(&self) -> u32 {
        self.read32(REG_MCPU_AP_IRQ_CLEAR)
    }

    fn clear_recv_mailbox(&self) {
        self.write32(REG_MCPU_IRQ_CLEAR, MCPU_MAILBOX1_NOT_EMPTY);
        arch::io_wmb();
    }

    fn drain_recv_mailbox(&self) -> usize {
        let mut drained = 0usize;
        let mut idle_polls = 0usize;
        for _ in 0..AVD_MAILBOX_DRAIN_POLLS {
            let raw = self.recv_mailbox();
            if raw == 0 {
                idle_polls += 1;
                if idle_polls >= AVD_MAILBOX_DRAIN_IDLE_POLLS {
                    break;
                }
                time::udelay(AVD_MAILBOX_DRAIN_POLL_US);
                continue;
            }

            idle_polls = 0;
            drained += 1;
            self.clear_recv_mailbox();
            arch::io_mb();
            time::udelay(AVD_MAILBOX_DRAIN_POLL_US);
        }
        drained
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

    fn init_decode_engine(&self) {
        self.write32(REG_DECODE_COUNTER0, 0x78);
        self.write32(REG_DECODE_COUNTER1, 0x78);
        self.write32(REG_DECODE_COUNTER2, 0x78);
        self.write32(REG_DECODE_COUNTER3, 0x78);
        self.write32(REG_DECODE_COUNTER4, 0x20);
        self.write32(REG_DECODE_CONTROL0, 0);
        self.write32(REG_DECODE_CONTROL1, 0);
        self.write32(REG_H265_CONTROL, 0);
        self.write32(REG_DECODE_DMA_TRIGGER, 0);
        self.write32(REG_VP9_CONTROL, 0);
        self.write32(
            REG_DECODE_TIMEOUT,
            self.read32(REG_DECODE_TIMEOUT) | 0x0050_0000,
        );
        self.write32(REG_DECODE_STATUS_MASK, 0x3);

        for (index, value) in AVD_DECODE_DMA_CONFIG.iter().copied().enumerate() {
            self.write32(REG_AVD_DMA_CONFIG_BASE + index * 4, value);
        }
    }

    fn clear_decode_status(&self, mask: u32) {
        self.write32(REG_DECODE_STATUS, mask);
    }

    fn decode_status(&self) -> u32 {
        self.read32(REG_DECODE_STATUS)
    }

    fn clear_decode_latched_status(&self) -> u32 {
        let mut last = 0;
        for _ in 0..8 {
            let status = self.decode_status();
            if status == 0 {
                break;
            }
            last = status;
            let clear = status | DECODE_STATUS_RECOVERY_CLEAR_MASK;
            self.clear_decode_status(clear);
            arch::io_mb();
        }
        last
    }

    fn write_h264_instructions(&self, words: &[u32]) {
        for word in words {
            self.write32(REG_H264_MODE, *word);
        }
    }

    fn write_vp9_instructions(&self, words: &[u32]) {
        for word in words {
            self.write32(REG_VP9_MODE, *word);
        }
    }

    fn configure_decode_stream(&self, instruction_fifo_dma: u64, pipe: AvdDecodePipe) {
        let fifo_offset = DECODE_T8103_FIFO_SLOT as usize * 4;
        let vp_slot = pipe.vp_slot();
        self.write32(
            REG_DECODE_INST_FIFO_BASE + fifo_offset,
            (instruction_fifo_dma >> 8) as u32,
        );
        self.write32(
            REG_DECODE_INST_FIFO_SIZE + fifo_offset,
            AVD_WORKSPACE_INST_FIFO_BYTES as u32,
        );
        self.write32(REG_DECODE_INST_FIFO_READ + fifo_offset, 0);
        self.write32(REG_DECODE_INST_FIFO_WRITE + fifo_offset, 0);
        self.write32(pipe.control_register(), 0);
        self.write32(
            REG_DECODE_TIMEOUT,
            self.read32(REG_DECODE_TIMEOUT)
                | (DECODE_VP_CM3_MASK << (vp_slot * 5))
                | (DECODE_PP_CM3_MASK << 20),
        );
    }

    fn submit_h264(&self) {
        self.write32(REG_DECODE_SUBMIT, H264_SUBMIT_FRAME);
    }

    fn submit_vp9(&self, tile_count: u32) -> u32 {
        self.write32(REG_DECODE_SUBMIT, VP9_SUBMIT_FRAME);
        for _ in 1..tile_count {
            self.write32(REG_DECODE_SUBMIT, VP9_SUBMIT_TILE);
        }
        arch::io_mb();
        self.decode_status()
    }

    fn submit_decode_postprocess(&self) {
        self.write32(
            REG_DECODE_SUBMIT,
            0x2b00_0000 | 0x100 | (DECODE_T8103_FIFO_SLOT << 4) | DECODE_T8103_FIFO_COUNT,
        );
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
        self.write32(REG_DECODE_STATUS_MASK, 0x3);
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
        self.write32(REG_DECODE_TIMEOUT, 0);
        self.write32(REG_DECODE_PIPE_CONTROL, 0);
        self.write32(REG_DECODE_PIPE_SELECT, 0x1555);

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
        self.write32(REG_MCPU_DECODE_DMA_CONFIG, MCPU_DECODE_DMA_CONFIG);

        self.write32(
            REG_DECODE_TIMEOUT,
            self.read32(REG_DECODE_TIMEOUT) | 0x0050_0000,
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
    /// MCPU IRQ enable/status register.
    pub irq_enable_status1: u32,
    /// CM3-to-AP mailbox status bits.
    pub mailbox_status: u32,
    /// Raw CM3-to-AP mailbox word.
    pub mailbox_raw: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AvdInterruptCause {
    irq_enable_status1: u32,
    mailbox_raw: u32,
}

impl AvdInterruptCause {
    fn is_asserted(self) -> bool {
        self.mailbox_raw != 0 || (self.irq_enable_status1 & MCPU_IRQ_ENABLE1_PENDING) != 0
    }
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

    /// Initialize the decode engine registers with the v3-class defaults.
    pub fn init_decode_engine(&mut self) {
        self.registers.init_decode_engine();
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

        self.init_decode_engine();
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
        self.mailbox = AvdFirmwareMailbox::new();
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
        instruction_fifo_dma: u64,
    ) -> Result<u32, &'static str> {
        if instructions.words().is_empty() {
            return Err("apple-avd: empty H.264 instruction stream");
        }
        self.registers.mask_irqs();
        self.registers.clear_irq_latches();
        self.registers.drain_recv_mailbox();
        self.registers.clear_decode_latched_status();
        self.registers
            .configure_decode_stream(instruction_fifo_dma, AvdDecodePipe::H264);
        self.registers.write_h264_instructions(instructions.words());
        self.registers.clear_decode_status(
            DECODE_STATUS_DONE_MASK | DECODE_STATUS_ERROR_MASK | DECODE_STATUS_ACCEPTED,
        );
        let status_before = self.registers.decode_status();
        self.trace.push(
            AvdTraceKind::DecodeSubmit,
            request.input.dma_addr,
            instructions.words().len() as u64,
        );
        Ok(status_before)
    }

    /// Submit a generated VP9 instruction stream to the MMIO command path.
    ///
    /// # Arguments
    ///
    /// * `request` - VP9 decode request metadata.
    /// * `instructions` - AVD VP9 instruction stream.
    /// * `instruction_fifo_dma` - Device-visible instruction FIFO base.
    ///
    /// # Returns
    ///
    /// Status register value observed immediately before submit.
    pub fn submit_vp9_mmio(
        &mut self,
        request: &Vp9DecodeRequest,
        instructions: &AvdVp9InstructionStream,
        instruction_fifo_dma: u64,
    ) -> Result<u32, &'static str> {
        if instructions.words().is_empty() {
            return Err("apple-avd: empty VP9 instruction stream");
        }
        self.registers.mask_irqs();
        self.registers.clear_irq_latches();
        self.registers.drain_recv_mailbox();
        self.registers
            .configure_decode_stream(instruction_fifo_dma, AvdDecodePipe::Vp9);
        for (index, word) in instructions.words().iter().enumerate() {
            self.trace.push(
                AvdTraceKind::InstructionWord,
                index as u64,
                u64::from(*word),
            );
        }
        self.registers.write_vp9_instructions(instructions.words());
        arch::io_wmb();
        let status_before = self.registers.decode_status();
        self.trace.push(
            AvdTraceKind::DecodeSubmit,
            request.input.dma_addr,
            instructions.words().len() as u64,
        );
        Ok(status_before)
    }

    /// Submit the post-process stage after the video pipe reports completion.
    pub fn submit_decode_postprocess(&mut self) {
        self.registers.submit_decode_postprocess();
    }

    /// Start a staged VP9 decode on the decode submit queue.
    ///
    /// # Arguments
    ///
    /// * `tile_count` - Number of VP9 tiles in the staged frame.
    pub fn start_vp9_decode(&mut self, tile_count: u32) -> u32 {
        self.registers.submit_vp9(tile_count)
    }

    /// Return the current decode status register.
    ///
    /// # Returns
    ///
    /// Raw decode engine status.
    pub fn decode_status(&self) -> u32 {
        self.registers.decode_status()
    }

    /// Clear selected decode status bits.
    ///
    /// # Arguments
    ///
    /// * `mask` - Status bits to acknowledge by writing them back to the
    ///   hardware status register.
    pub fn clear_decode_status(&mut self, mask: u32) {
        self.registers.clear_decode_status(mask);
    }

    /// Clear decode status and replay engine initialization after an error.
    ///
    /// # Arguments
    ///
    /// * `reason` - Driver-local reason or status value recorded in the trace.
    pub fn recover_decode_engine(&mut self, reason: u64) {
        self.trace.push(AvdTraceKind::Fault, reason, 0);
        self.registers.mask_irqs();
        let drained_before = self.registers.drain_recv_mailbox();
        let status_before = self.registers.clear_decode_latched_status();
        self.registers.init_decode_engine();
        let status_after_init = self.registers.clear_decode_latched_status();
        let drained_after = self.registers.drain_recv_mailbox();
        self.registers.clear_irq_latches();
        self.registers.mask_irqs();
        if status_before != 0 || status_after_init != 0 || drained_before != 0 || drained_after != 0
        {
            println!(
                "[apple-avd] recovered decode engine reason={:#x} status_before={:#x} status_after_init={:#x} drained_before={} drained_after={}",
                reason, status_before, status_after_init, drained_before, drained_after
            );
        }
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
            AvdFirmwareMessage::Panic => {
                self.mark_firmware_faulted();
                self.trace.push(AvdTraceKind::Fault, raw as u64, 0);
            }
            AvdFirmwareMessage::VideoProcessorError => {
                self.trace.push(AvdTraceKind::Fault, raw as u64, 0);
            }
            _ => {}
        }

        Some(message)
    }

    fn snapshot(&self) -> AvdStatusSnapshot {
        AvdStatusSnapshot {
            status: self.registers.status(),
            irq_enable_status1: self.registers.irq_enable_status1(),
            mailbox_status: self.registers.recv_mailbox_status(),
            mailbox_raw: self.registers.recv_mailbox(),
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
        let drained = self.registers.drain_recv_mailbox();
        if drained != 0 {
            println!(
                "[apple-avd] boot drained stale mailbox messages={}",
                drained
            );
        }
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
        self.registers.mask_irqs();
        self.registers.log_boot_state("before-run-masked");
        if let Err(e) = self.registers.run_cm3() {
            self.registers.log_boot_state("after-run-timeout");
            self.mark_firmware_faulted();
            self.trace.push(AvdTraceKind::Fault, 0x1098_0090, 0);
            println!("[apple-avd] boot {}", e);
            return Err(e);
        }
        self.registers.log_boot_state("after-run");
        self.registers.mask_irqs();
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
    reference_slot_span: usize,
    slot_count: usize,
}

impl AvdSessionWorkspace {
    fn new_h264(
        avd: &AppleAvd,
        layout: h264::AvdFrameLayout,
        slot_count: usize,
    ) -> Result<Self, &'static str> {
        if slot_count == 0 || slot_count > AVD_H264_OUTPUT_SLOTS {
            return Err("apple-avd: invalid workspace slot count");
        }
        let reference_slot_span = Self::h264_reference_slot_span(layout)?;
        let required = AVD_WORKSPACE_REFERENCE_OFFSET
            .checked_add(
                reference_slot_span
                    .checked_mul(slot_count)
                    .ok_or("apple-avd: workspace size overflow")?,
            )
            .ok_or("apple-avd: workspace size overflow")?;
        let granule = avd.dma_context().mapping_granule().max(AVD_WORKSPACE_ALIGN);
        let byte_len = align_up(required, granule);
        let page_count = byte_len.div_ceil(PAGE_SIZE);
        let pages = ContiguousPages::new_aligned(page_count, granule)
            .ok_or("apple-avd: workspace allocation failed")?;
        // The large reference workspace is device-owned after setup. Drop any
        // stale CPU cache lines once, then only clean CPU-written subranges.
        arch::clean_invalidate_dcache_to_poc_range(pages.as_vaddr(), byte_len);
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
            reference_slot_span,
            slot_count,
        })
    }

    fn new_vp9(
        avd: &AppleAvd,
        layout: vp9::AvdVp9FrameLayout,
        slot_count: usize,
    ) -> Result<Self, &'static str> {
        if slot_count == 0 || slot_count > AVD_VP9_REFERENCE_SLOTS {
            return Err("apple-avd: invalid VP9 workspace slot count");
        }
        let reference_slot_span = Self::vp9_reference_slot_span(layout)?;
        let required = AVD_WORKSPACE_REFERENCE_OFFSET
            .checked_add(
                reference_slot_span
                    .checked_mul(slot_count)
                    .ok_or("apple-avd: VP9 workspace size overflow")?,
            )
            .ok_or("apple-avd: VP9 workspace size overflow")?;
        let granule = avd.dma_context().mapping_granule().max(AVD_WORKSPACE_ALIGN);
        let byte_len = align_up(required, granule);
        let page_count = byte_len.div_ceil(PAGE_SIZE);
        Self::validate_vp9_workspace(layout, slot_count, reference_slot_span, byte_len)?;
        println!(
            "[apple-avd] vp9 workspace alloc begin layout={}x{} y_stride={} slot_count={} slot_span={} required={} byte_len={} pages={} granule={}",
            layout.width,
            layout.height,
            layout.y_stride,
            slot_count,
            reference_slot_span,
            required,
            byte_len,
            page_count,
            granule
        );
        let pages = ContiguousPages::new_aligned(page_count, granule)
            .ok_or("apple-avd: VP9 workspace allocation failed")?;
        println!(
            "[apple-avd] vp9 workspace alloc ok vaddr={:#x} paddr={:#x}",
            pages.as_vaddr(),
            pages.as_paddr()
        );
        println!(
            "[apple-avd] vp9 workspace cache clean begin len={}",
            byte_len
        );
        arch::clean_invalidate_dcache_to_poc_range(pages.as_vaddr(), byte_len);
        println!("[apple-avd] vp9 workspace cache clean ok");
        println!(
            "[apple-avd] vp9 workspace dma map begin paddr={:#x} len={}",
            pages.as_paddr(),
            byte_len
        );
        let mapping = avd
            .dma_context()
            .map_phys_owned(
                pages.as_paddr(),
                byte_len,
                IommuMapFlags::READ | IommuMapFlags::WRITE | IommuMapFlags::COHERENT,
            )
            .map_err(|_| "apple-avd: VP9 workspace DMA map failed")?;
        println!(
            "[apple-avd] vp9 workspace dma map ok dma={:#x}",
            mapping.dma_addr()
        );
        Ok(Self {
            pages,
            mapping,
            byte_len,
            reference_slot_span,
            slot_count,
        })
    }

    fn h264_reference_slot_span(layout: h264::AvdFrameLayout) -> Result<usize, &'static str> {
        let rvra_len = align_up(layout.rvra_len(), AVD_DMA_GRANULE);
        let sps_len = align_up(layout.sps_scratch_len(), AVD_DMA_GRANULE);
        rvra_len
            .checked_add(sps_len)
            .map(|len| align_up(len, AVD_DMA_GRANULE))
            .ok_or("apple-avd: reference workspace slot size overflow")
    }

    fn vp9_reference_slot_span(layout: vp9::AvdVp9FrameLayout) -> Result<usize, &'static str> {
        let rvra_len = align_up(layout.rvra_len(), AVD_DMA_GRANULE);
        let sps_len = align_up(layout.sps_scratch_len(), AVD_DMA_GRANULE);
        rvra_len
            .checked_add(sps_len)
            .map(|len| align_up(len, AVD_DMA_GRANULE))
            .ok_or("apple-avd: VP9 reference workspace slot size overflow")
    }

    fn validate_vp9_workspace(
        layout: vp9::AvdVp9FrameLayout,
        slot_count: usize,
        reference_slot_span: usize,
        byte_len: usize,
    ) -> Result<(), &'static str> {
        check_workspace_window(
            AVD_WORKSPACE_INST_FIFO_OFFSET,
            AVD_WORKSPACE_INST_FIFO_BYTES,
            byte_len,
            "apple-avd: VP9 instruction FIFO exceeds workspace",
        )?;
        check_workspace_window(
            AVD_WORKSPACE_VP9_PROBS_OFFSET,
            SCARLET_VIDEO_VP9_PROBABILITY_BYTES,
            AVD_WORKSPACE_REFERENCE_OFFSET,
            "apple-avd: VP9 probability table overlaps reference workspace",
        )?;
        check_workspace_window(
            AVD_WORKSPACE_VP9_PPS0_OFFSET,
            0x8000,
            AVD_WORKSPACE_REFERENCE_OFFSET,
            "apple-avd: VP9 pps0 table overlaps reference workspace",
        )?;
        check_workspace_window(
            AVD_WORKSPACE_VP9_PPS1_OFFSET,
            8 * 0x8000,
            AVD_WORKSPACE_REFERENCE_OFFSET,
            "apple-avd: VP9 pps1 table overlaps reference workspace",
        )?;
        check_workspace_window(
            AVD_WORKSPACE_VP9_PPS2_OFFSET,
            2 * 0x8000,
            AVD_WORKSPACE_REFERENCE_OFFSET,
            "apple-avd: VP9 pps2 table overlaps reference workspace",
        )?;

        let rvra_len = align_up(layout.rvra_len(), AVD_DMA_GRANULE);
        let sps_len = align_up(layout.sps_scratch_len(), AVD_DMA_GRANULE);
        let slot_payload_len = rvra_len
            .checked_add(sps_len)
            .ok_or("apple-avd: VP9 workspace slot size overflow")?;
        if slot_payload_len > reference_slot_span {
            return Err("apple-avd: VP9 reference workspace slot is undersized");
        }
        for offset in layout.rvra_offsets() {
            if offset as usize >= rvra_len {
                return Err("apple-avd: VP9 RVRA offset exceeds RVRA workspace");
            }
        }
        for slot in 0..slot_count {
            let slot_base = AVD_WORKSPACE_REFERENCE_OFFSET
                .checked_add(
                    slot.checked_mul(reference_slot_span)
                        .ok_or("apple-avd: VP9 workspace slot offset overflow")?,
                )
                .ok_or("apple-avd: VP9 workspace slot offset overflow")?;
            check_workspace_window(
                slot_base,
                slot_payload_len,
                byte_len,
                "apple-avd: VP9 reference slot exceeds workspace allocation",
            )?;
        }
        Ok(())
    }

    fn is_compatible_h264(&self, layout: h264::AvdFrameLayout, slot_count: usize) -> bool {
        slot_count <= self.slot_count
            && Self::h264_reference_slot_span(layout)
                .is_ok_and(|slot_span| slot_span == self.reference_slot_span)
    }

    fn is_compatible_vp9(&self, layout: vp9::AvdVp9FrameLayout, slot_count: usize) -> bool {
        slot_count <= self.slot_count
            && Self::vp9_reference_slot_span(layout)
                .is_ok_and(|slot_span| slot_span == self.reference_slot_span)
    }

    fn addresses_for_h264_slot(
        &self,
        slot: usize,
        layout: h264::AvdFrameLayout,
    ) -> Result<AvdH264Workspace, &'static str> {
        if slot >= self.slot_count {
            return Err("apple-avd: invalid reference slot");
        }
        let rvra_len = align_up(layout.rvra_len(), AVD_DMA_GRANULE);
        let base = self.mapping.dma_addr();
        let reference_offset = AVD_WORKSPACE_REFERENCE_OFFSET
            + slot
                .checked_mul(self.reference_slot_span)
                .ok_or("apple-avd: reference workspace offset overflow")?;
        let reference_dma_addr = base + reference_offset as u64;
        Ok(AvdH264Workspace {
            instruction_fifo_dma_addr: base + AVD_WORKSPACE_INST_FIFO_OFFSET as u64,
            pps_tile_dma_addr: base + AVD_WORKSPACE_PPS_TILE_OFFSET as u64,
            sps_tile_dma_addr: reference_dma_addr + rvra_len as u64,
            reference_dma_addr,
            reference_offsets: layout.rvra_offsets(),
        })
    }

    fn addresses_for_vp9_slot(
        &self,
        slot: usize,
        layout: vp9::AvdVp9FrameLayout,
        references: [Option<usize>; 3],
    ) -> Result<AvdVp9Workspace, &'static str> {
        if slot >= self.slot_count {
            return Err("apple-avd: invalid VP9 reference slot");
        }
        let base = self.mapping.dma_addr();
        let rvra_len = align_up(layout.rvra_len(), AVD_DMA_GRANULE);
        let current_rvra = self.vp9_rvra_addrs_for_slot(slot, layout)?;
        let mut reference_rvra = [[0u64; 4]; 3];
        for (index, reference_slot) in references.iter().copied().enumerate() {
            if let Some(reference_slot) = reference_slot {
                reference_rvra[index] = self.vp9_rvra_addrs_for_slot(reference_slot, layout)?;
            }
        }
        let mut pps1 = [0u64; 8];
        for (index, addr) in pps1.iter_mut().enumerate() {
            *addr = base + AVD_WORKSPACE_VP9_PPS1_OFFSET as u64 + index as u64 * 0x8000;
        }
        Ok(AvdVp9Workspace {
            instruction_fifo_dma_addr: base + AVD_WORKSPACE_INST_FIFO_OFFSET as u64,
            probabilities_dma_addr: base + AVD_WORKSPACE_VP9_PROBS_OFFSET as u64,
            pps0_tile_dma_addr: base + AVD_WORKSPACE_VP9_PPS0_OFFSET as u64,
            pps1_tile_dma_addrs: pps1,
            pps2_tile_dma_addrs: [
                base + AVD_WORKSPACE_VP9_PPS2_OFFSET as u64,
                base + AVD_WORKSPACE_VP9_PPS2_OFFSET as u64 + 0x8000,
            ],
            sps_tile_dma_addr: base
                + AVD_WORKSPACE_REFERENCE_OFFSET as u64
                + slot as u64 * self.reference_slot_span as u64
                + rvra_len as u64,
            current_rvra_dma_addrs: current_rvra,
            reference_rvra_dma_addrs: reference_rvra,
        })
    }

    fn vp9_rvra_addrs_for_slot(
        &self,
        slot: usize,
        layout: vp9::AvdVp9FrameLayout,
    ) -> Result<[u64; 4], &'static str> {
        if slot >= self.slot_count {
            return Err("apple-avd: invalid VP9 RVRA slot");
        }
        let base = self.mapping.dma_addr();
        let reference_offset = AVD_WORKSPACE_REFERENCE_OFFSET
            + slot
                .checked_mul(self.reference_slot_span)
                .ok_or("apple-avd: VP9 reference workspace offset overflow")?;
        let reference_dma_addr = base + reference_offset as u64;
        let offsets = layout.rvra_offsets();
        Ok([
            reference_dma_addr + offsets[0] as u64,
            reference_dma_addr + offsets[1] as u64,
            reference_dma_addr + offsets[2] as u64,
            reference_dma_addr + offsets[3] as u64,
        ])
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

    fn vp9_probabilities_vaddr(&self) -> usize {
        self.pages.as_vaddr() + AVD_WORKSPACE_VP9_PROBS_OFFSET
    }

    fn vp9_probabilities_mut(&mut self) -> &mut [u8] {
        let ptr = self.vp9_probabilities_vaddr() as *mut u8;
        // SAFETY: `pages` owns `byte_len` bytes, and the VP9 probability table
        // window sits below `AVD_WORKSPACE_REFERENCE_OFFSET`.
        unsafe { core::slice::from_raw_parts_mut(ptr, SCARLET_VIDEO_VP9_PROBABILITY_BYTES) }
    }
}

struct AvdBackendSession {
    stream_id: u32,
    active: bool,
    coded_format: u32,
    next_frame: u32,
    vp9_frame_state: AvdVp9FrameState,
    stream_parameters: Option<H264StreamParameters>,
    workspace: Option<AvdSessionWorkspace>,
    input_pool: Option<AvdMappedInputPool>,
    output_pool: Option<AvdMappedOutputPool>,
    reference_frames: [Option<AvdReferenceFrame>; AVD_REFERENCE_FRAME_TABLE_LEN],
    reference_frame_count: usize,
    next_reference_slot: usize,
    reference_slot_len: usize,
    active_slot_count: usize,
    dpb_capacity: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AvdVp9FrameState {
    access_index: u32,
    keyframe_count: u32,
    keyframe_index: u32,
    last_was_keyframe: bool,
    last_refresh_flags: u8,
    accumulated_refresh_mask: u8,
}

impl AvdVp9FrameState {
    const fn new() -> Self {
        Self {
            access_index: 0,
            keyframe_count: 0,
            keyframe_index: 0,
            last_was_keyframe: false,
            last_refresh_flags: 0xff,
            accumulated_refresh_mask: 0,
        }
    }

    fn begin_picture_state(self, params: &ScarletVideoVp9StatelessParams) -> Self {
        let key_frame = params.frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME != 0;
        if !key_frame {
            return self;
        }
        Self {
            keyframe_count: self.keyframe_count.wrapping_add(1),
            keyframe_index: 0,
            accumulated_refresh_mask: 0,
            ..self
        }
    }

    fn to_picture_state(self) -> vp9::AvdVp9PictureState {
        vp9::AvdVp9PictureState {
            access_index: self.access_index,
            keyframe_count: self.keyframe_count,
            keyframe_index: self.keyframe_index,
            last_was_keyframe: self.last_was_keyframe,
            last_refresh_flags: self.last_refresh_flags,
            accumulated_refresh_mask: self.accumulated_refresh_mask,
        }
    }

    fn finish_picture(mut self, params: &ScarletVideoVp9StatelessParams) -> Self {
        let key_frame = params.frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME != 0;
        let refresh_flags = if key_frame {
            0xff
        } else {
            params.frame.refresh_frame_flags
        };
        if !key_frame {
            self.keyframe_index = self.keyframe_index.wrapping_add(1);
        }
        if self.keyframe_index == 1 {
            self.accumulated_refresh_mask |= refresh_flags & 0b1;
        }
        if self.keyframe_index >= 2 {
            self.accumulated_refresh_mask |= refresh_flags;
        }
        self.last_was_keyframe = key_frame;
        self.last_refresh_flags = refresh_flags;
        self.access_index = self.access_index.wrapping_add(1);
        self
    }
}

impl AvdBackendSession {
    fn new(index: usize) -> Self {
        Self {
            stream_id: (index + 1) as u32,
            active: false,
            coded_format: 0,
            next_frame: 0,
            vp9_frame_state: AvdVp9FrameState::new(),
            stream_parameters: None,
            workspace: None,
            input_pool: None,
            output_pool: None,
            reference_frames: [None; AVD_REFERENCE_FRAME_TABLE_LEN],
            reference_frame_count: 0,
            next_reference_slot: 0,
            reference_slot_len: 0,
            active_slot_count: 0,
            dpb_capacity: 0,
        }
    }

    fn reset(&mut self) {
        self.active = false;
        self.coded_format = 0;
        self.next_frame = 0;
        self.vp9_frame_state = AvdVp9FrameState::new();
        self.stream_parameters = None;
        self.workspace = None;
        self.input_pool = None;
        self.output_pool = None;
        self.clear_reference_frames();
        self.next_reference_slot = 0;
        self.reference_slot_len = 0;
        self.active_slot_count = 0;
        self.dpb_capacity = 0;
    }

    fn clear_reference_frames(&mut self) {
        for frame in &mut self.reference_frames {
            *frame = None;
        }
        self.reference_frame_count = 0;
    }

    fn reference_frames(&self) -> impl Iterator<Item = &AvdReferenceFrame> {
        self.reference_frames.iter().filter_map(Option::as_ref)
    }

    fn retain_reference_frames<F>(&mut self, mut keep: F)
    where
        F: FnMut(&AvdReferenceFrame) -> bool,
    {
        for entry in &mut self.reference_frames {
            let remove = entry.as_ref().is_some_and(|frame| !keep(frame));
            if remove {
                *entry = None;
                self.reference_frame_count = self.reference_frame_count.saturating_sub(1);
            }
        }
    }

    fn remove_reference_slot(&mut self, slot: usize) {
        for entry in &mut self.reference_frames {
            let remove = entry.as_ref().is_some_and(|frame| frame.slot == slot);
            if remove {
                *entry = None;
                self.reference_frame_count = self.reference_frame_count.saturating_sub(1);
            }
        }
    }

    fn remove_oldest_reference_frame(&mut self) {
        let mut oldest_index = None;
        let mut oldest_frame_number = u32::MAX;
        for (index, entry) in self.reference_frames.iter().enumerate() {
            let Some(frame) = entry else {
                continue;
            };
            if oldest_index.is_none() || frame.frame_number < oldest_frame_number {
                oldest_index = Some(index);
                oldest_frame_number = frame.frame_number;
            }
        }
        if let Some(index) = oldest_index {
            self.reference_frames[index] = None;
            self.reference_frame_count = self.reference_frame_count.saturating_sub(1);
        }
    }

    fn trim_reference_frames(&mut self, target_count: usize) {
        while self.reference_frame_count > target_count {
            self.remove_oldest_reference_frame();
        }
    }

    fn insert_reference_frame(&mut self, frame: AvdReferenceFrame) -> Result<(), &'static str> {
        self.remove_reference_slot(frame.slot);
        if self.reference_frame_count >= AVD_REFERENCE_FRAME_TABLE_LEN {
            self.remove_oldest_reference_frame();
        }
        let Some(entry) = self
            .reference_frames
            .iter_mut()
            .find(|entry| entry.is_none())
        else {
            return Err("apple-avd: reference frame table is full");
        };
        *entry = Some(frame);
        self.reference_frame_count += 1;
        Ok(())
    }

    fn ensure_workspace(&mut self) -> Result<&mut AvdSessionWorkspace, &'static str> {
        self.workspace
            .as_mut()
            .ok_or("apple-avd: workspace unavailable")
    }

    fn ensure_mapped_input(
        &mut self,
        avd: &AppleAvd,
        paddr: usize,
        vaddr: usize,
        len: usize,
    ) -> Result<u64, &'static str> {
        let remap_input = self
            .input_pool
            .as_ref()
            .is_none_or(|pool| pool.paddr != paddr || pool.vaddr != vaddr || pool.len != len);
        if remap_input {
            let mapping = avd
                .dma_context()
                .map_phys_owned(paddr, len, IommuMapFlags::READ | IommuMapFlags::COHERENT)
                .map_err(|_| "apple-avd: input DMA map failed")?;
            self.input_pool = Some(AvdMappedInputPool {
                paddr,
                vaddr,
                len,
                mapping,
            });
        }
        Ok(self
            .input_pool
            .as_ref()
            .ok_or("apple-avd: mapped input pool unavailable")?
            .mapping
            .dma_addr())
    }

    fn prune_reference_frames_for_dpb(
        &mut self,
        dpb: &[ScarletVideoH264DpbEntry; 16],
        max_references: usize,
    ) -> usize {
        let valid_entries = dpb
            .iter()
            .filter(|entry| entry.flags & SCARLET_VIDEO_H264_DPB_FLAG_VALID != 0)
            .count();
        let target_references = valid_entries
            .min(max_references)
            .min(AVD_REFERENCE_FRAME_TABLE_LEN);
        if target_references == 0 {
            self.clear_reference_frames();
            return valid_entries;
        }
        self.retain_reference_frames(|frame| {
            dpb.iter()
                .any(|entry| dpb_entry_matches_reference(entry, frame))
        });
        self.trim_reference_frames(target_references);
        valid_entries
    }

    fn prune_vp9_reference_frames(&mut self, params: &ScarletVideoVp9StatelessParams) -> usize {
        let key_frame = params.frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME != 0;
        if key_frame {
            self.clear_reference_frames();
            return 0;
        }
        let refs = [
            params.frame.last_frame_ts,
            params.frame.golden_frame_ts,
            params.frame.alt_frame_ts,
        ];
        self.retain_reference_frames(|frame| {
            refs.iter().any(|timestamp| *timestamp == frame.timestamp)
        });
        self.reference_frame_count
    }

    fn prepare_mapped_output(
        &mut self,
        avd: &AppleAvd,
        request: &VideoBackendDecodeRequest,
        layout: h264::AvdFrameLayout,
        stream_parameters: H264StreamParameters,
        store_reference: bool,
        is_idr: bool,
    ) -> Result<AvdReferenceOutput, &'static str> {
        let payload_len = layout.output_len();
        let slot_span = AVD_OUTPUT_SLOT_PAYLOAD_OFFSET
            .checked_add(payload_len)
            .map(|len| align_up(len, AVD_DMA_GRANULE))
            .ok_or("apple-avd: output slot size overflow")?;
        let mut dpb_capacity =
            (stream_parameters.max_num_ref_frames as usize).min(AVD_REFERENCE_FRAME_TABLE_LEN);
        if store_reference && dpb_capacity == 0 {
            dpb_capacity = 1;
        }
        let minimum_slot_count = dpb_capacity
            .checked_add(AVD_H264_EXTRA_DECODE_SLOTS)
            .ok_or("apple-avd: output slot count overflow")?
            .max(1)
            .min(AVD_H264_OUTPUT_SLOTS);
        if payload_len == 0 {
            return Err("apple-avd: decoded frame exceeds mapped output slot pool");
        }
        let max_slot_count_by_output = (request.output_len as usize / slot_span)
            .max(1)
            .min(AVD_H264_OUTPUT_SLOTS);
        if max_slot_count_by_output < minimum_slot_count {
            return Err("apple-avd: decoded frame exceeds mapped output slot pool");
        }
        let slot_count = max_slot_count_by_output;
        if is_idr {
            self.clear_reference_frames();
            self.next_reference_slot = 0;
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
            self.clear_reference_frames();
            self.next_reference_slot = 0;
            self.reference_slot_len = 0;
            self.active_slot_count = 0;
            self.dpb_capacity = 0;
        }

        let remap_workspace = self
            .workspace
            .as_ref()
            .is_none_or(|workspace| !workspace.is_compatible_h264(layout, slot_count));
        if remap_workspace {
            self.workspace = Some(AvdSessionWorkspace::new_h264(avd, layout, slot_count)?);
            self.clear_reference_frames();
            self.next_reference_slot = 0;
        }

        if self.reference_slot_len != slot_span
            || self.active_slot_count != slot_count
            || self.dpb_capacity != dpb_capacity
        {
            self.clear_reference_frames();
            self.next_reference_slot = 0;
            self.reference_slot_len = slot_span;
            self.active_slot_count = slot_count;
            self.dpb_capacity = dpb_capacity;
        }

        let slot = self
            .select_free_output_slot(slot_count)
            .ok_or("apple-avd: no free output reference slot")?;
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
            slot_count,
            dpb_capacity,
            dma_addr: output_pool.mapping.dma_addr() + payload_offset_in_output as u64,
            vaddr: output_pool.vaddr + payload_offset_in_output,
            header_vaddr: output_pool.vaddr + header_offset_in_output,
            payload_offset: request.output_offset as usize + payload_offset_in_output,
            len: payload_len,
        })
    }

    fn prepare_mapped_output_vp9(
        &mut self,
        avd: &AppleAvd,
        request: &VideoBackendDecodeRequest,
        layout: vp9::AvdVp9FrameLayout,
        store_reference: bool,
        key_frame: bool,
        log_decode: bool,
    ) -> Result<AvdReferenceOutput, &'static str> {
        let payload_len = layout.output_len();
        let slot_span = AVD_OUTPUT_SLOT_PAYLOAD_OFFSET
            .checked_add(payload_len)
            .map(|len| align_up(len, AVD_DMA_GRANULE))
            .ok_or("apple-avd: VP9 output slot size overflow")?;
        let dpb_capacity = if store_reference {
            AVD_VP9_REFERENCE_SLOTS
        } else {
            1
        };
        if payload_len == 0 {
            return Err("apple-avd: decoded VP9 frame exceeds mapped output slot pool");
        }
        let max_slot_count_by_output = (request.output_len as usize / slot_span)
            .max(1)
            .min(AVD_VP9_REFERENCE_SLOTS);
        if max_slot_count_by_output < dpb_capacity.min(AVD_VP9_REFERENCE_SLOTS) {
            return Err("apple-avd: decoded VP9 frame exceeds mapped output slot pool");
        }
        let slot_count = max_slot_count_by_output;
        if key_frame {
            self.clear_reference_frames();
            self.next_reference_slot = 0;
        }

        let granule = avd.dma_context().mapping_granule().max(AVD_DMA_GRANULE);
        let output_map_len = align_up(request.output_len as usize, granule);
        let remap_output = self.output_pool.as_ref().is_none_or(|pool| {
            pool.paddr != request.output_paddr
                || pool.vaddr != request.output_vaddr
                || pool.len != output_map_len
        });
        if remap_output {
            println!(
                "[apple-avd] vp9 output map begin layout={}x{} payload={} slot_span={} slot_count={} dpb={} paddr={:#x} vaddr={:#x} len={}",
                layout.width,
                layout.height,
                payload_len,
                slot_span,
                slot_count,
                dpb_capacity,
                request.output_paddr,
                request.output_vaddr,
                output_map_len
            );
            let mapping = avd
                .dma_context()
                .map_phys_owned(
                    request.output_paddr,
                    output_map_len,
                    IommuMapFlags::READ | IommuMapFlags::WRITE | IommuMapFlags::COHERENT,
                )
                .map_err(|_| "apple-avd: VP9 output DMA map failed")?;
            println!(
                "[apple-avd] vp9 output map ok dma={:#x}",
                mapping.dma_addr()
            );
            self.output_pool = Some(AvdMappedOutputPool {
                paddr: request.output_paddr,
                vaddr: request.output_vaddr,
                len: output_map_len,
                mapping,
            });
            self.clear_reference_frames();
            self.next_reference_slot = 0;
            self.reference_slot_len = 0;
            self.active_slot_count = 0;
            self.dpb_capacity = 0;
        }

        let remap_workspace = self
            .workspace
            .as_ref()
            .is_none_or(|workspace| !workspace.is_compatible_vp9(layout, slot_count));
        if remap_workspace {
            println!(
                "[apple-avd] vp9 workspace remap layout={}x{} slot_count={} rvra_len={} sps_len={}",
                layout.width,
                layout.height,
                slot_count,
                layout.rvra_len(),
                layout.sps_scratch_len()
            );
            self.workspace = Some(AvdSessionWorkspace::new_vp9(avd, layout, slot_count)?);
            self.clear_reference_frames();
            self.next_reference_slot = 0;
        }

        if self.reference_slot_len != slot_span
            || self.active_slot_count != slot_count
            || self.dpb_capacity != dpb_capacity
        {
            self.clear_reference_frames();
            self.next_reference_slot = 0;
            self.reference_slot_len = slot_span;
            self.active_slot_count = slot_count;
            self.dpb_capacity = dpb_capacity;
        }

        let mut slot = self.select_free_output_slot(slot_count);
        if slot.is_none() {
            self.remove_oldest_reference_frame();
            slot = self.select_free_output_slot(slot_count);
        }
        let slot = slot.ok_or("apple-avd: no free VP9 output reference slot")?;
        if log_decode {
            println!(
                "[apple-avd] vp9 output slot={} slot_count={} slot_span={} payload_len={} store_ref={} key={}",
                slot, slot_count, slot_span, payload_len, store_reference, key_frame
            );
        }
        let slot_offset = slot
            .checked_mul(slot_span)
            .ok_or("apple-avd: VP9 output slot offset overflow")?;
        let payload_offset_in_output = slot_offset
            .checked_add(AVD_OUTPUT_SLOT_PAYLOAD_OFFSET)
            .ok_or("apple-avd: VP9 output payload offset overflow")?;
        let payload_end = payload_offset_in_output
            .checked_add(payload_len)
            .ok_or("apple-avd: VP9 output payload end overflow")?;
        if payload_end > request.output_len as usize {
            return Err("apple-avd: VP9 output payload exceeds mapped output slot pool");
        }
        let header_offset_in_output = payload_offset_in_output
            .checked_sub(SCARLET_VIDEO_FRAME_HEADER_LEN)
            .ok_or("apple-avd: VP9 output header offset underflow")?;
        let output_pool = self
            .output_pool
            .as_ref()
            .ok_or("apple-avd: mapped VP9 output pool unavailable")?;
        Ok(AvdReferenceOutput {
            slot,
            slot_count,
            dpb_capacity,
            dma_addr: output_pool.mapping.dma_addr() + payload_offset_in_output as u64,
            vaddr: output_pool.vaddr + payload_offset_in_output,
            header_vaddr: output_pool.vaddr + header_offset_in_output,
            payload_offset: request.output_offset as usize + payload_offset_in_output,
            len: payload_len,
        })
    }

    fn select_free_output_slot(&mut self, slot_count: usize) -> Option<usize> {
        for offset in 0..slot_count {
            let candidate = (self.next_reference_slot + offset) % slot_count;
            if self.reference_frames().all(|frame| frame.slot != candidate) {
                self.next_reference_slot = (candidate + 1) % slot_count;
                return Some(candidate);
            }
        }
        None
    }
}

fn dpb_entry_matches_reference(
    entry: &ScarletVideoH264DpbEntry,
    frame: &AvdReferenceFrame,
) -> bool {
    if entry.flags & SCARLET_VIDEO_H264_DPB_FLAG_VALID == 0 {
        return false;
    }
    let entry_long_term = entry.flags & SCARLET_VIDEO_H264_DPB_FLAG_LONG_TERM != 0;
    if frame.timestamp != 0 && entry.reference_ts == frame.timestamp {
        return true;
    }
    if frame.frame_num == entry.frame_num
        && frame.top_field_order_cnt == entry.top_field_order_cnt
        && frame.long_term == entry_long_term
    {
        return true;
    }
    frame.top_field_order_cnt == entry.top_field_order_cnt && frame.pic_num == entry.pic_num
}

fn h264_reference_pictures_from_dpb(
    dpb: &[ScarletVideoH264DpbEntry; 16],
    frames: &[Option<AvdReferenceFrame>; AVD_REFERENCE_FRAME_TABLE_LEN],
    out: &mut [AvdH264ReferencePicture; AVD_REFERENCE_FRAME_TABLE_LEN],
) -> usize {
    let mut count = 0usize;
    for entry in dpb
        .iter()
        .filter(|entry| entry.flags & SCARLET_VIDEO_H264_DPB_FLAG_VALID != 0)
    {
        if count >= out.len() {
            break;
        }
        let Some(frame) = frames
            .iter()
            .filter_map(Option::as_ref)
            .find(|frame| dpb_entry_matches_reference(entry, frame))
        else {
            continue;
        };
        out[count] = AvdH264ReferencePicture {
            timestamp: frame.timestamp,
            reference_dma_addr: frame.reference_dma_addr,
            sps_tile_dma_addr: frame.sps_tile_dma_addr,
            frame_num: frame.frame_num,
            pic_num: frame.pic_num,
            top_field_order_cnt: frame.top_field_order_cnt,
            long_term: frame.long_term,
        };
        count += 1;
    }
    count
}

fn vp9_reference_pictures_from_frame(
    params: &ScarletVideoVp9StatelessParams,
    frames: &[Option<AvdReferenceFrame>; AVD_REFERENCE_FRAME_TABLE_LEN],
) -> [Option<AvdVp9ReferencePicture>; 3] {
    let timestamps = [
        params.frame.last_frame_ts,
        params.frame.golden_frame_ts,
        params.frame.alt_frame_ts,
    ];
    let mut references = [None; 3];
    for (index, timestamp) in timestamps.iter().copied().enumerate() {
        if timestamp == 0 {
            continue;
        }
        references[index] = frames
            .iter()
            .filter_map(Option::as_ref)
            .find(|frame| frame.timestamp == timestamp)
            .map(|frame| AvdVp9ReferencePicture {
                timestamp: frame.timestamp,
                rvra_dma_addrs: frame.vp9_rvra_dma_addrs,
            });
    }
    references
}

fn vp9_reference_slots_from_frame(
    params: &ScarletVideoVp9StatelessParams,
    frames: &[Option<AvdReferenceFrame>; AVD_REFERENCE_FRAME_TABLE_LEN],
) -> [Option<usize>; 3] {
    let timestamps = [
        params.frame.last_frame_ts,
        params.frame.golden_frame_ts,
        params.frame.alt_frame_ts,
    ];
    let mut slots = [None; 3];
    for (index, timestamp) in timestamps.iter().copied().enumerate() {
        if timestamp == 0 {
            continue;
        }
        slots[index] = frames
            .iter()
            .filter_map(Option::as_ref)
            .find(|frame| frame.timestamp == timestamp)
            .map(|frame| frame.slot);
    }
    slots
}

#[derive(Clone, Copy)]
struct AvdReferenceFrame {
    slot: usize,
    frame_number: u32,
    timestamp: u64,
    layout: AvdDecodedLayout,
    reference_dma_addr: u64,
    sps_tile_dma_addr: u64,
    vp9_rvra_dma_addrs: [u64; 4],
    frame_num: u16,
    pic_num: i32,
    top_field_order_cnt: i32,
    long_term: bool,
}

struct AvdMappedInputPool {
    paddr: usize,
    vaddr: usize,
    len: usize,
    mapping: DmaMapping,
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
    slot_count: usize,
    dpb_capacity: usize,
    dma_addr: u64,
    vaddr: usize,
    header_vaddr: usize,
    payload_offset: usize,
    len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AvdDecodedLayout {
    width: u32,
    height: u32,
    y_stride: u32,
    uv_stride: u32,
    pixel_format: u32,
}

impl From<h264::AvdFrameLayout> for AvdDecodedLayout {
    fn from(layout: h264::AvdFrameLayout) -> Self {
        Self {
            width: layout.width,
            height: layout.height,
            y_stride: layout.y_stride,
            uv_stride: layout.uv_stride,
            pixel_format: layout.pixel_format,
        }
    }
}

impl From<vp9::AvdVp9FrameLayout> for AvdDecodedLayout {
    fn from(layout: vp9::AvdVp9FrameLayout) -> Self {
        Self {
            width: layout.width,
            height: layout.height,
            y_stride: layout.y_stride,
            uv_stride: layout.uv_stride,
            pixel_format: layout.pixel_format,
        }
    }
}

struct AvdPendingDecode {
    kind: AvdPendingDecodeKind,
    stream_id: u32,
    frame_number: u32,
    timestamp: u64,
    layout: AvdDecodedLayout,
    display_x: u32,
    display_y: u32,
    display_width: u32,
    display_height: u32,
    payload_len: usize,
    output_header_vaddr: usize,
    output_payload_vaddr: usize,
    output_payload_dma: u64,
    output_payload_offset: usize,
    output_payload_len: usize,
    reference_slot: usize,
    reference_slot_count: usize,
    dpb_capacity: usize,
    reference_dma_addr: u64,
    sps_tile_dma_addr: u64,
    vp9_rvra_dma_addrs: [u64; 4],
    frame_num: u16,
    pic_num: i32,
    top_field_order_cnt: i32,
    long_term: bool,
    store_reference: bool,
    is_idr: bool,
    status_before: u32,
    command_tag: u32,
    poll_count: usize,
    completion_phase: AvdDecodeCompletionPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AvdPendingDecodeKind {
    H264,
    Vp9,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AvdDecodeCompletionPhase {
    WaitingVideo,
    WaitingPostprocess,
}

#[derive(Clone, Copy)]
struct AvdCompletionInfo {
    status_before: u32,
    status: u32,
    message_raw: u32,
    by_mailbox: bool,
    by_status: bool,
}

#[derive(Clone, Copy)]
struct AvdOutputSample {
    y_first: [u32; 4],
    y_checksum: u32,
    y_nonzero: usize,
    uv_first: [u32; 2],
    uv_checksum: u32,
    uv_non_neutral: usize,
    y0: u8,
    u0: u8,
    v0: u8,
}

struct AvdCompletedQueue {
    frames: [Option<VideoBackendDecodedFrame>; AVD_COMPLETED_FRAME_QUEUE_LEN],
    head: usize,
    len: usize,
}

impl AvdCompletedQueue {
    fn new() -> Self {
        Self {
            frames: [None; AVD_COMPLETED_FRAME_QUEUE_LEN],
            head: 0,
            len: 0,
        }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn clear(&mut self) {
        for offset in 0..self.len {
            let index = self.index(offset);
            self.frames[index] = None;
        }
        self.head = 0;
        self.len = 0;
    }

    fn retain<F>(&mut self, mut keep: F)
    where
        F: FnMut(&VideoBackendDecodedFrame) -> bool,
    {
        let old_len = self.len;
        let mut write = 0;
        for read in 0..old_len {
            let read_index = self.index(read);
            let Some(frame) = self.frames[read_index].take() else {
                continue;
            };
            if keep(&frame) {
                self.frames[write] = Some(frame);
                write += 1;
            }
        }
        for index in write..old_len {
            self.frames[index] = None;
        }
        self.head = 0;
        self.len = write;
    }

    fn push_back(&mut self, frame: VideoBackendDecodedFrame) -> Result<(), &'static str> {
        if self.len >= AVD_COMPLETED_FRAME_QUEUE_LEN {
            return Err("apple-avd: decoded frame queue is full");
        }
        let index = self.index(self.len);
        self.frames[index] = Some(frame);
        self.len += 1;
        Ok(())
    }

    fn position<F>(&self, mut predicate: F) -> Option<usize>
    where
        F: FnMut(&VideoBackendDecodedFrame) -> bool,
    {
        for offset in 0..self.len {
            let index = self.index(offset);
            let Some(frame) = self.frames[index].as_ref() else {
                continue;
            };
            if predicate(frame) {
                return Some(offset);
            }
        }
        None
    }

    fn remove(&mut self, offset: usize) -> Option<VideoBackendDecodedFrame> {
        if offset >= self.len {
            return None;
        }
        let removed_index = self.index(offset);
        let removed = self.frames[removed_index].take();
        for read in (offset + 1)..self.len {
            let read_index = self.index(read);
            let write_index = self.index(read - 1);
            self.frames[write_index] = self.frames[read_index].take();
        }
        let tail_index = self.index(self.len - 1);
        self.frames[tail_index] = None;
        self.len -= 1;
        if self.len == 0 {
            self.head = 0;
        }
        removed
    }

    fn index(&self, offset: usize) -> usize {
        (self.head + offset) % AVD_COMPLETED_FRAME_QUEUE_LEN
    }
}

struct AvdBackendState {
    sessions: [AvdBackendSession; AVD_MAX_SESSIONS],
    pending: Option<AvdPendingDecode>,
    completed: AvdCompletedQueue,
}

impl AvdBackendState {
    fn new() -> Self {
        Self {
            sessions: core::array::from_fn(AvdBackendSession::new),
            pending: None,
            completed: AvdCompletedQueue::new(),
        }
    }

    fn allocate_session(&mut self, coded_format: u32) -> Result<u32, &'static str> {
        let index = self
            .sessions
            .iter()
            .position(|session| !session.active)
            .ok_or("apple-avd: no free video sessions")?;
        self.reset_session_state(index);
        let session = &mut self.sessions[index];
        session.active = true;
        session.coded_format = coded_format;
        session.next_frame = 0;
        session.stream_parameters = None;
        Ok(session.stream_id)
    }

    fn reset_all_sessions(&mut self) {
        for session in &mut self.sessions {
            session.reset();
        }
        self.pending = None;
        self.completed.clear();
    }

    fn session_index(&self, stream_id: u32) -> Result<usize, &'static str> {
        self.sessions
            .iter()
            .position(|session| session.stream_id == stream_id)
            .ok_or("apple-avd: invalid stream id")
    }

    fn reset_session_state(&mut self, index: usize) {
        let stream_id = self.sessions[index].stream_id;
        self.sessions[index].reset();
        self.completed.retain(|frame| frame.stream_id != stream_id);
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
        let index = self.session_index(stream_id)?;
        let session = &mut self.sessions[index];
        if !session.active {
            session.active = true;
            session.coded_format = coded_format;
        }
        if session.coded_format != coded_format {
            return Err("apple-avd: stream format mismatch");
        }
        Ok(session)
    }

    fn has_pending_for_stream(&self, stream_id: u32) -> Result<bool, &'static str> {
        let index = self.session_index(stream_id)?;
        let session = &self.sessions[index];
        if !session.active {
            return Ok(false);
        }
        Ok(self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.stream_id == stream_id))
    }

    fn destroy_session(&mut self, stream_id: u32) -> Result<(), &'static str> {
        let index = self.session_index(stream_id)?;
        let session = &self.sessions[index];
        if !session.active {
            return Ok(());
        }
        let had_pending = self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.stream_id == stream_id);
        if had_pending {
            let _ = self.pending.take();
        }
        self.reset_session_state(index);
        Ok(())
    }

    fn pending_len(&self) -> usize {
        usize::from(self.pending.is_some())
    }
}

struct AppleAvdVideoBackend {
    avd_id: u32,
    state: Mutex<AvdBackendState>,
    completion_notifier: Mutex<Option<Weak<dyn VideoCompletionNotifier>>>,
    interrupt_id: Mutex<Option<InterruptId>>,
}

impl AppleAvdVideoBackend {
    fn new(avd_id: u32) -> Self {
        Self {
            avd_id,
            state: Mutex::new(AvdBackendState::new()),
            completion_notifier: Mutex::new(None),
            interrupt_id: Mutex::new(None),
        }
    }

    fn avd(&self) -> Result<Arc<Mutex<AppleAvd>>, &'static str> {
        get_apple_avd(self.avd_id).ok_or("apple-avd: backend instance disappeared")
    }

    fn set_interrupt_id(&self, interrupt_id: InterruptId) {
        let _irq_guard = IrqGuard::new();
        *self.interrupt_id.lock() = Some(interrupt_id);
    }

    fn notify_completion(&self) {
        let notifier = {
            let _irq_guard = IrqGuard::new();
            self.completion_notifier
                .lock()
                .as_ref()
                .and_then(Weak::upgrade)
        };
        if let Some(notifier) = notifier {
            notifier.notify_video_completion();
        }
    }

    fn has_pending_decode(&self) -> bool {
        let _irq_guard = IrqGuard::new();
        self.state.lock().pending.is_some()
    }

    fn mask_avd_interrupts(&self) -> InterruptResult<()> {
        let avd = self.avd().map_err(|_| InterruptError::HardwareError)?;
        let _irq_guard = IrqGuard::new();
        avd.lock().registers.mask_irqs();
        arch::io_mb();
        Ok(())
    }

    fn unmask_avd_interrupts(&self) -> InterruptResult<()> {
        let avd = self.avd().map_err(|_| InterruptError::HardwareError)?;
        let _irq_guard = IrqGuard::new();
        let avd = avd.lock();
        if self.state.lock().pending.is_some() {
            avd.registers.enable_irqs();
        } else {
            avd.registers.mask_irqs();
        }
        arch::io_mb();
        Ok(())
    }

    fn clear_stale_avd_interrupts(&self) -> InterruptResult<()> {
        let avd = self.avd().map_err(|_| InterruptError::HardwareError)?;
        let _irq_guard = IrqGuard::new();
        let avd = avd.lock();
        avd.registers.mask_irqs();
        avd.registers.clear_irq_latches();
        let _ = avd.registers.drain_recv_mailbox();
        avd.registers.clear_irq_latches();
        avd.registers.mask_irqs();
        arch::io_mb();
        Ok(())
    }

    fn arm_pending_decode_interrupts(&self, avd: &AppleAvd) {
        if self.interrupt_id.lock().is_some() {
            avd.registers.enable_irqs();
        } else {
            avd.registers.mask_irqs();
        }
        arch::io_mb();
    }

    fn avd_interrupt_cause(&self) -> InterruptResult<AvdInterruptCause> {
        let avd = self.avd().map_err(|_| InterruptError::HardwareError)?;
        let _irq_guard = IrqGuard::new();
        let avd = avd.lock();
        let snapshot = avd.snapshot();
        Ok(AvdInterruptCause {
            irq_enable_status1: snapshot.irq_enable_status1,
            mailbox_raw: snapshot.mailbox_raw,
        })
    }

    fn service_completions(&self) -> Result<(), &'static str> {
        let avd = self.avd()?;
        let _irq_guard = IrqGuard::new();
        let mut avd = avd.lock();
        let mut state = self.state.lock();
        let Some(front) = state.pending.as_ref() else {
            return Ok(());
        };
        let message = avd.poll_firmware_message();
        let message_raw = message.map(|message| message.raw()).unwrap_or(0);
        let status = avd.decode_status();
        let status_before = front.status_before;
        let frame_number = front.frame_number;
        let stream_id = front.stream_id;
        let pending_kind = front.kind;
        let poll_count = front.poll_count;
        let completion_phase = front.completion_phase;

        let status_error = status != status_before && (status & DECODE_STATUS_ERROR_MASK) != 0;
        let firmware_panic = matches!(message, Some(AvdFirmwareMessage::Panic));
        if firmware_panic {
            println!(
                "[apple-avd] firmware panic stream={} frame={} phase={:?} poll={} status_before={:#x} status={:#x} msg={:?}/{:#x}",
                stream_id,
                frame_number,
                completion_phase,
                poll_count,
                status_before,
                status,
                message,
                message_raw
            );
            let _ = state.pending.take();
            avd.recover_decode_engine(status as u64);
            avd.registers.mask_irqs();
            state.reset_all_sessions();
            return Err("apple-avd: firmware panic during decode");
        }

        let processor_error = matches!(
            message,
            Some(AvdFirmwareMessage::VideoProcessorError | AvdFirmwareMessage::UnknownIrq)
        );
        if (status_error || processor_error)
            && (poll_count < 4 || poll_count % 1000 == 0 || message_raw != 0)
        {
            println!(
                "[apple-avd] decode error pending watchdog stream={} frame={} phase={:?} poll={} status_before={:#x} status={:#x} msg={:?}/{:#x}",
                stream_id,
                frame_number,
                completion_phase,
                poll_count,
                status_before,
                status,
                message,
                message_raw
            );
        }

        if completion_phase == AvdDecodeCompletionPhase::WaitingVideo
            && (status & DECODE_STATUS_ACCEPTED) != 0
        {
            avd.clear_decode_status(DECODE_STATUS_ACCEPTED);
            if should_log_decode_progress(frame_number) {
                println!(
                    "[apple-avd] decode accepted stream={} frame={} poll={} status_before={:#x} status={:#x} msg={:#x} clear={:#x}",
                    stream_id,
                    frame_number,
                    poll_count,
                    status_before,
                    status,
                    message_raw,
                    DECODE_STATUS_ACCEPTED
                );
            }
        }

        if pending_kind == AvdPendingDecodeKind::H264
            && completion_phase == AvdDecodeCompletionPhase::WaitingVideo
            && matches!(message, Some(AvdFirmwareMessage::VideoProcessorDone))
        {
            avd.submit_decode_postprocess();
            let front = state
                .pending
                .as_mut()
                .ok_or("apple-avd: pending queue changed during video mailbox completion")?;
            front.completion_phase = AvdDecodeCompletionPhase::WaitingPostprocess;
            front.poll_count = front.poll_count.saturating_add(1);
            if should_log_decode_progress(frame_number) {
                println!(
                    "[apple-avd] decode video mailbox stream={} frame={} poll={} status_before={:#x} status={:#x} msg={:#x} submit_pp=true",
                    stream_id, frame_number, poll_count, status_before, status, message_raw
                );
            }
            return Ok(());
        }

        let vp9_video_done_by_status = pending_kind == AvdPendingDecodeKind::Vp9
            && completion_phase == AvdDecodeCompletionPhase::WaitingVideo
            && (status & DECODE_STATUS_VIDEO_PHASE_MASK) == DECODE_STATUS_VIDEO_PHASE_MASK;
        if vp9_video_done_by_status {
            avd.clear_decode_status(DECODE_STATUS_VP9_VIDEO_DONE_CLEAR);
            avd.submit_decode_postprocess();
            let front = state
                .pending
                .as_mut()
                .ok_or("apple-avd: pending queue changed during VP9 video completion")?;
            front.completion_phase = AvdDecodeCompletionPhase::WaitingPostprocess;
            front.poll_count = front.poll_count.saturating_add(1);
            if should_log_decode_progress(frame_number) {
                println!(
                    "[apple-avd] vp9 decode video status stream={} frame={} poll={} status_before={:#x} status={:#x} msg={:#x} clear={:#x} submit_pp=true",
                    stream_id,
                    frame_number,
                    poll_count,
                    status_before,
                    status,
                    message_raw,
                    DECODE_STATUS_VP9_VIDEO_DONE_CLEAR
                );
            }
            return Ok(());
        }

        let completed_by_mailbox = pending_kind == AvdPendingDecodeKind::H264
            && matches!(message, Some(AvdFirmwareMessage::PostProcessorDone));
        let completed_by_status = pending_kind == AvdPendingDecodeKind::Vp9
            && completion_phase == AvdDecodeCompletionPhase::WaitingPostprocess
            && (status & DECODE_STATUS_POSTPROCESS_PHASE_MASK) == DECODE_STATUS_POSTPROCESS_DONE;
        if should_log_decode_progress(frame_number)
            && (message_raw != 0 || status != status_before || poll_count == 0)
            && (poll_count < 4 || message_raw != 0 || poll_count % 1000 == 0)
        {
            println!(
                "[apple-avd] decode poll stream={} frame={} phase={:?} poll={} status_before={:#x} status={:#x} msg={:#x}",
                stream_id,
                frame_number,
                completion_phase,
                poll_count,
                status_before,
                status,
                message_raw
            );
        }

        if completed_by_mailbox || completed_by_status {
            if completed_by_status && pending_kind == AvdPendingDecodeKind::Vp9 {
                avd.clear_decode_status(DECODE_STATUS_POSTPROCESS_DONE_CLEAR);
            }
            let pending = state
                .pending
                .take()
                .ok_or("apple-avd: pending queue changed under completion")?;
            let completion = AvdCompletionInfo {
                status_before,
                status,
                message_raw,
                by_mailbox: completed_by_mailbox,
                by_status: completed_by_status,
            };
            finish_pending_decode(&mut state, pending, completion)?;
            avd.trace.push(
                AvdTraceKind::DecodeComplete,
                status as u64,
                message_raw as u64,
            );
            avd.registers.clear_irq_latches();
            avd.registers.mask_irqs();
        } else {
            let updated_poll_count = {
                let front = state
                    .pending
                    .as_mut()
                    .ok_or("apple-avd: pending queue changed under poll")?;
                front.poll_count = front.poll_count.saturating_add(1);
                front.poll_count
            };
            if updated_poll_count > AVD_DECODE_POLL_LIMIT {
                println!(
                    "[apple-avd] decode timeout stream={} frame={} phase={:?} polls={} status_before={:#x} status={:#x} msg={:#x}",
                    stream_id,
                    frame_number,
                    completion_phase,
                    updated_poll_count,
                    status_before,
                    status,
                    message_raw
                );
                let _ = state.pending.take();
                avd.recover_decode_engine(status as u64);
                avd.registers.mask_irqs();
                state.reset_all_sessions();
                return Err("apple-avd: decode timed out");
            }
        }
        Ok(())
    }

    fn validate_h264_request(
        &self,
        request: &VideoBackendDecodeRequest,
    ) -> Result<(), &'static str> {
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
        Ok(())
    }

    fn validate_vp9_request(
        &self,
        request: &VideoBackendDecodeRequest,
    ) -> Result<(), &'static str> {
        if request.coded_format != SCARLET_VIDEO_FORMAT_VP9 {
            return Err("apple-avd: unsupported VP9 coded format");
        }
        if request.input_len == 0 {
            return Err("apple-avd: empty VP9 input frame");
        }
        if request.output_len as usize <= SCARLET_VIDEO_FRAME_HEADER_LEN {
            return Err("apple-avd: VP9 output buffer is too small");
        }
        if request.input_len as usize > AVD_MAPPED_INPUT_BYTES {
            return Err("apple-avd: VP9 input exceeds mapped input buffer");
        }
        Ok(())
    }

    fn submit_h264_prepared_locked(
        &self,
        avd: &mut AppleAvd,
        state: &mut AvdBackendState,
        request: &VideoBackendDecodeRequest,
        stream_parameters: H264StreamParameters,
        params: &ScarletVideoH264StatelessParams,
    ) -> Result<(), &'static str> {
        let granule = avd.dma_context().mapping_granule().max(PAGE_SIZE);
        let input_vaddr = request.input_vaddr;
        let input_len = request.input_len as usize;
        let input_clean_len = align_up(input_len, granule);
        let input_map_len = align_up(AVD_MAPPED_INPUT_BYTES, granule);
        arch::clean_invalidate_dcache_to_poc_range(input_vaddr, input_clean_len);

        let session = state.active_session_mut(request.stream_id)?;
        if session.coded_format != request.coded_format {
            return Err("apple-avd: stream format mismatch");
        }
        let layout = stream_parameters.nv12_layout();
        let hardware_payload_len = layout.output_len();
        let display_payload_len =
            nv12_tight_payload_len(stream_parameters.width, stream_parameters.height)?;
        let frame_number = session.next_frame;
        session.next_frame = session.next_frame.wrapping_add(1);
        let log_decode = should_log_decode_progress(frame_number);
        let input_dma_addr = session.ensure_mapped_input(
            avd,
            request.input_paddr,
            request.input_vaddr,
            input_map_len,
        )?;
        let input = AvdDmaRange {
            dma_addr: input_dma_addr,
            len: input_len,
        };
        // SAFETY: `input_vaddr` points at the mapped video input area for
        // `input_len` bytes during this submit, and userspace data has already
        // been copied there before the backend ioctl is invoked.
        let input_bytes =
            unsafe { core::slice::from_raw_parts(input_vaddr as *const u8, input_len) };
        let is_idr = params.decode_params.flags & SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR != 0;
        let store_reference = params.decode_params.nal_ref_idc != 0;
        let max_references =
            (stream_parameters.max_num_ref_frames as usize).min(AVD_REFERENCE_FRAME_TABLE_LEN);
        let valid_dpb_entries =
            session.prune_reference_frames_for_dpb(&params.decode_params.dpb, max_references);
        let reference_output = session.prepare_mapped_output(
            avd,
            request,
            layout,
            stream_parameters,
            store_reference,
            is_idr,
        )?;
        let output = AvdDmaRange {
            dma_addr: reference_output.dma_addr,
            len: hardware_payload_len,
        };
        let decode_request = H264DecodeRequest::from_stateless(
            session.stream_id as u64,
            frame_number,
            params,
            input,
            input_bytes,
            output,
            layout,
        )
        .map_err(h264_error_to_str)?;
        let mut reference_picture_storage =
            [AvdH264ReferencePicture::default(); AVD_REFERENCE_FRAME_TABLE_LEN];
        let reference_picture_count = h264_reference_pictures_from_dpb(
            &decode_request.dpb,
            &session.reference_frames,
            &mut reference_picture_storage,
        );
        let reference_pictures = &reference_picture_storage[..reference_picture_count];

        let (instructions, inst_len, instruction_fifo_dma, reference_dma_addr, sps_tile_dma_addr) = {
            let workspace = session.ensure_workspace()?;
            let workspace_addresses =
                workspace.addresses_for_h264_slot(reference_output.slot, layout)?;
            let reference_dma_addr = workspace_addresses.reference_dma_addr;
            let sps_tile_dma_addr = workspace_addresses.sps_tile_dma_addr;
            let instructions = AvdH264InstructionStream::build(
                &decode_request,
                &stream_parameters,
                &decode_request.slice,
                &workspace_addresses,
                reference_pictures,
            );
            let inst_len = instructions
                .write_le_bytes(workspace.instruction_fifo_mut())
                .map_err(h264_error_to_str)?;
            arch::clean_dcache_to_poc_range(workspace.instruction_fifo_vaddr(), inst_len);
            (
                instructions,
                inst_len,
                workspace_addresses.instruction_fifo_dma_addr,
                reference_dma_addr,
                sps_tile_dma_addr,
            )
        };

        arch::clean_invalidate_dcache_to_poc_range(reference_output.vaddr, reference_output.len);

        let status_before =
            avd.submit_h264_mmio(&decode_request, &instructions, instruction_fifo_dma)?;
        let command_tag = avd.submit_h264_request(&decode_request)?;
        if log_decode {
            let words = instructions.words();
            println!(
                "[apple-avd] decode submit stream={} frame={} idr={} ref={} slice={:?} poc={} refs={} valid_dpb={} slot={}/{} inst_words={} status_before={:#x} tag={:#x}",
                request.stream_id,
                frame_number,
                decode_request.slice.is_idr(),
                decode_request.slice.is_reference(),
                decode_request.slice.kind,
                decode_request.current_poc,
                reference_pictures.len(),
                valid_dpb_entries,
                reference_output.slot,
                reference_output.slot_count,
                words.len(),
                status_before,
                command_tag
            );
        }
        avd.trace
            .push(AvdTraceKind::DecodeSubmit, input_dma_addr, inst_len as u64);
        state.pending = Some(AvdPendingDecode {
            kind: AvdPendingDecodeKind::H264,
            stream_id: request.stream_id,
            frame_number,
            timestamp: request.timestamp,
            layout: layout.into(),
            display_x: stream_parameters.crop_left,
            display_y: stream_parameters.crop_top,
            display_width: stream_parameters.width,
            display_height: stream_parameters.height,
            payload_len: display_payload_len,
            output_header_vaddr: reference_output.header_vaddr,
            output_payload_vaddr: reference_output.vaddr,
            output_payload_dma: reference_output.dma_addr,
            output_payload_offset: reference_output.payload_offset,
            output_payload_len: reference_output.len,
            reference_slot: reference_output.slot,
            reference_slot_count: reference_output.slot_count,
            dpb_capacity: reference_output.dpb_capacity,
            reference_dma_addr,
            sps_tile_dma_addr,
            vp9_rvra_dma_addrs: [0; 4],
            frame_num: decode_request.frame_num,
            pic_num: i32::from(decode_request.frame_num),
            top_field_order_cnt: decode_request.current_poc,
            long_term: false,
            store_reference: decode_request.slice.is_reference(),
            is_idr: decode_request.slice.is_idr(),
            status_before,
            command_tag,
            poll_count: 0,
            completion_phase: AvdDecodeCompletionPhase::WaitingVideo,
        });
        self.arm_pending_decode_interrupts(avd);
        Ok(())
    }

    fn submit_vp9_prepared_locked(
        &self,
        avd: &mut AppleAvd,
        state: &mut AvdBackendState,
        request: &VideoBackendDecodeRequest,
        stream_parameters: Vp9StreamParameters,
        params: &ScarletVideoVp9StatelessParams,
    ) -> Result<(), &'static str> {
        let granule = avd.dma_context().mapping_granule().max(PAGE_SIZE);
        let input_vaddr = request.input_vaddr;
        let input_len = request.input_len as usize;
        let input_clean_len = align_up(input_len, granule);
        let input_map_len = align_up(AVD_MAPPED_INPUT_BYTES, granule);
        let log_request = request.timestamp <= 4;
        if log_request {
            println!(
                "[apple-avd] vp9 prepare entry stream={} ts={} input_len={} input_clean_len={} input_map_len={}",
                request.stream_id, request.timestamp, input_len, input_clean_len, input_map_len
            );
            println!(
                "[apple-avd] vp9 input cache clean begin vaddr={:#x} len={}",
                input_vaddr, input_clean_len
            );
        }
        arch::clean_invalidate_dcache_to_poc_range(input_vaddr, input_clean_len);
        if log_request {
            println!("[apple-avd] vp9 input cache clean ok");
        }

        let session = state.active_session_mut(request.stream_id)?;
        if session.coded_format != request.coded_format {
            return Err("apple-avd: VP9 stream format mismatch");
        }
        let layout = stream_parameters.nv12_layout();
        let hardware_payload_len = layout.output_len();
        let display_payload_len = nv12_tight_payload_len(
            stream_parameters.render_width,
            stream_parameters.render_height,
        )?;
        let frame_number = session.next_frame;
        session.next_frame = session.next_frame.wrapping_add(1);
        let log_decode = log_request || should_log_decode_progress(frame_number);
        if log_decode {
            println!(
                "[apple-avd] vp9 frame begin stream={} frame={} ts={} coded={}x{} render={}x{} tiles={} flags={:#x} refresh={:#x}",
                request.stream_id,
                frame_number,
                request.timestamp,
                stream_parameters.width,
                stream_parameters.height,
                stream_parameters.render_width,
                stream_parameters.render_height,
                params.tiles.tile_count,
                params.frame.flags,
                params.frame.refresh_frame_flags
            );
            println!(
                "[apple-avd] vp9 input map begin paddr={:#x} vaddr={:#x} len={}",
                request.input_paddr, request.input_vaddr, input_map_len
            );
        }
        let input_dma_addr = session.ensure_mapped_input(
            avd,
            request.input_paddr,
            request.input_vaddr,
            input_map_len,
        )?;
        if log_decode {
            println!("[apple-avd] vp9 input map ok dma={:#x}", input_dma_addr);
        }
        let input = AvdDmaRange {
            dma_addr: input_dma_addr,
            len: input_len,
        };
        let key_frame = params.frame.flags & SCARLET_VIDEO_VP9_FRAME_FLAG_KEY_FRAME != 0;
        let store_reference = params.frame.refresh_frame_flags != 0;
        let valid_references = session.prune_vp9_reference_frames(params);
        let reference_slots = vp9_reference_slots_from_frame(params, &session.reference_frames);
        let reference_pictures =
            vp9_reference_pictures_from_frame(params, &session.reference_frames);
        if log_decode {
            println!(
                "[apple-avd] vp9 output prepare begin refs={} store_ref={} key={}",
                valid_references, store_reference, key_frame
            );
        }
        let reference_output = session.prepare_mapped_output_vp9(
            avd,
            request,
            layout,
            store_reference,
            key_frame,
            log_decode,
        )?;
        if log_decode {
            println!(
                "[apple-avd] vp9 output prepare ok slot={} dma={:#x} vaddr={:#x} len={}",
                reference_output.slot,
                reference_output.dma_addr,
                reference_output.vaddr,
                reference_output.len
            );
        }
        let output = AvdDmaRange {
            dma_addr: reference_output.dma_addr,
            len: hardware_payload_len,
        };
        if log_decode {
            println!("[apple-avd] vp9 decode request build begin");
        }
        let decode_request = Vp9DecodeRequest::from_stateless(
            session.stream_id as u64,
            frame_number,
            params,
            input,
            output,
            layout,
        )
        .map_err(vp9_error_to_str)?;
        if log_decode {
            println!("[apple-avd] vp9 decode request build ok");
        }
        let begun_vp9_frame_state = session.vp9_frame_state.begin_picture_state(params);
        let vp9_picture_state = begun_vp9_frame_state.to_picture_state();

        let (instructions, inst_len, instruction_fifo_dma, sps_tile_dma_addr, vp9_rvra_dma_addrs) = {
            if log_decode {
                println!("[apple-avd] vp9 workspace access begin");
            }
            let workspace = session.ensure_workspace()?;
            let workspace_addresses =
                workspace.addresses_for_vp9_slot(reference_output.slot, layout, reference_slots)?;
            if log_decode {
                println!(
                    "[apple-avd] vp9 workspace access ok fifo_dma={:#x} probs_dma={:#x} sps={:#x} rvra=[{:#x},{:#x},{:#x},{:#x}]",
                    workspace_addresses.instruction_fifo_dma_addr,
                    workspace_addresses.probabilities_dma_addr,
                    workspace_addresses.sps_tile_dma_addr,
                    workspace_addresses.current_rvra_dma_addrs[0],
                    workspace_addresses.current_rvra_dma_addrs[1],
                    workspace_addresses.current_rvra_dma_addrs[2],
                    workspace_addresses.current_rvra_dma_addrs[3]
                );
            }
            workspace
                .vp9_probabilities_mut()
                .copy_from_slice(&params.probabilities.data);
            if log_decode {
                println!("[apple-avd] vp9 probabilities cache clean begin");
            }
            arch::clean_dcache_to_poc_range(
                workspace.vp9_probabilities_vaddr(),
                SCARLET_VIDEO_VP9_PROBABILITY_BYTES,
            );
            if log_decode {
                println!("[apple-avd] vp9 probabilities cache clean ok");
                println!("[apple-avd] vp9 instruction build begin");
            }
            let instructions = AvdVp9InstructionStream::build(
                &decode_request,
                &stream_parameters,
                &workspace_addresses,
                &reference_pictures,
                vp9_picture_state,
            );
            if log_decode {
                println!(
                    "[apple-avd] vp9 instruction build ok words={}",
                    instructions.words().len()
                );
                println!("[apple-avd] vp9 instruction write begin");
            }
            let inst_len = instructions
                .write_le_bytes(workspace.instruction_fifo_mut())
                .map_err(vp9_error_to_str)?;
            if log_decode {
                println!("[apple-avd] vp9 instruction write ok bytes={}", inst_len);
                println!("[apple-avd] vp9 instruction cache clean begin");
            }
            arch::clean_dcache_to_poc_range(workspace.instruction_fifo_vaddr(), inst_len);
            if log_decode {
                println!("[apple-avd] vp9 instruction cache clean ok");
            }
            (
                instructions,
                inst_len,
                workspace_addresses.instruction_fifo_dma_addr,
                workspace_addresses.sps_tile_dma_addr,
                workspace_addresses.current_rvra_dma_addrs,
            )
        };

        if log_decode {
            println!(
                "[apple-avd] vp9 output cache invalidate begin vaddr={:#x} len={}",
                reference_output.vaddr, reference_output.len
            );
        }
        arch::clean_invalidate_dcache_to_poc_range(reference_output.vaddr, reference_output.len);
        if log_decode {
            println!("[apple-avd] vp9 output cache invalidate ok");
            println!(
                "[apple-avd] vp9 mmio submit begin fifo_dma={:#x}",
                instruction_fifo_dma
            );
        }

        let status_before =
            avd.submit_vp9_mmio(&decode_request, &instructions, instruction_fifo_dma)?;
        if log_decode {
            println!(
                "[apple-avd] vp9 mmio submit ok status_before={:#x}",
                status_before
            );
        }
        let command_tag = frame_number;
        session.vp9_frame_state = begun_vp9_frame_state.finish_picture(params);
        if log_decode {
            println!(
                "[apple-avd] vp9 direct submit prepared tag={:#x}",
                command_tag
            );
        }
        if log_decode {
            println!(
                "[apple-avd] vp9 submit stream={} frame={} key={} ref={} refs={} slot={}/{} tiles={} inst_words={} status_before={:#x} tag={:#x}",
                request.stream_id,
                frame_number,
                key_frame,
                store_reference,
                valid_references,
                reference_output.slot,
                reference_output.slot_count,
                params.tiles.tile_count,
                instructions.words().len(),
                status_before,
                command_tag
            );
        }
        avd.trace
            .push(AvdTraceKind::DecodeSubmit, input_dma_addr, inst_len as u64);
        state.pending = Some(AvdPendingDecode {
            kind: AvdPendingDecodeKind::Vp9,
            stream_id: request.stream_id,
            frame_number,
            timestamp: request.timestamp,
            layout: layout.into(),
            display_x: 0,
            display_y: 0,
            display_width: stream_parameters.render_width,
            display_height: stream_parameters.render_height,
            payload_len: display_payload_len,
            output_header_vaddr: reference_output.header_vaddr,
            output_payload_vaddr: reference_output.vaddr,
            output_payload_dma: reference_output.dma_addr,
            output_payload_offset: reference_output.payload_offset,
            output_payload_len: reference_output.len,
            reference_slot: reference_output.slot,
            reference_slot_count: reference_output.slot_count,
            dpb_capacity: reference_output.dpb_capacity,
            reference_dma_addr: vp9_rvra_dma_addrs[0],
            sps_tile_dma_addr,
            vp9_rvra_dma_addrs,
            frame_num: 0,
            pic_num: 0,
            top_field_order_cnt: 0,
            long_term: false,
            store_reference,
            is_idr: key_frame,
            status_before,
            command_tag,
            poll_count: 0,
            completion_phase: AvdDecodeCompletionPhase::WaitingVideo,
        });
        self.arm_pending_decode_interrupts(avd);
        if log_decode {
            println!(
                "[apple-avd] vp9 decode start begin tiles={}",
                params.tiles.tile_count
            );
        }
        let status_after_start = avd.start_vp9_decode(params.tiles.tile_count as u32);
        if log_decode {
            println!(
                "[apple-avd] vp9 decode start ok tiles={} status_after_start={:#x}",
                params.tiles.tile_count, status_after_start
            );
        }
        Ok(())
    }
}

impl VideoDecodeBackend for AppleAvdVideoBackend {
    fn name(&self) -> &'static str {
        "apple-avd"
    }

    fn set_completion_notifier(&self, notifier: Option<Weak<dyn VideoCompletionNotifier>>) {
        let _irq_guard = IrqGuard::new();
        *self.completion_notifier.lock() = notifier;
    }

    fn debug_status(&self) -> Option<String> {
        let avd = self.avd().ok()?;
        let (snapshot, decode_status, firmware) = {
            let _irq_guard = IrqGuard::new();
            let avd = avd.lock();
            (
                avd.debug_snapshot(),
                avd.decode_status(),
                avd.firmware_state_name(),
            )
        };
        let _irq_guard = IrqGuard::new();
        let state = self.state.lock();
        Some(format!(
            " fw={} pending={} completed={} decode_status={:#x} status={:#x} irq_enable_status1={:#x} mailbox_status={:#x} mailbox_raw={:#x}",
            firmware,
            state.pending_len(),
            state.completed.len(),
            decode_status,
            snapshot.status,
            snapshot.irq_enable_status1,
            snapshot.mailbox_status,
            snapshot.mailbox_raw
        ))
    }

    fn capabilities(&self) -> VideoBackendCapabilities {
        VideoBackendCapabilities {
            max_sessions: AVD_MAX_SESSIONS as u32,
            max_inflight_decodes: 1,
            mapped_input_len: AVD_MAPPED_INPUT_BYTES as u32,
            mapped_output_len: AVD_MAPPED_OUTPUT_BYTES as u32,
            output_pixel_format: SCARLET_VIDEO_PIXEL_FORMAT_NV12,
            supports_h264: false,
            supports_av1: false,
            supports_hevc: false,
            supports_stateless_h264: true,
        }
    }

    fn supports_stateless_vp9(&self) -> bool {
        true
    }

    fn create_session(&self, coded_format: u32) -> Result<u32, &'static str> {
        if !matches!(
            coded_format,
            SCARLET_VIDEO_FORMAT_H264 | SCARLET_VIDEO_FORMAT_VP9
        ) {
            return Err("apple-avd: only stateless H.264/VP9 sessions are supported");
        }
        self.service_completions()?;
        let _irq_guard = IrqGuard::new();
        let mut state = self.state.lock();
        state.allocate_session(coded_format)
    }

    fn destroy_session(&self, stream_id: u32) -> Result<(), &'static str> {
        let avd = self.avd()?;
        let _irq_guard = IrqGuard::new();
        let mut avd = avd.lock();
        let mut state = self.state.lock();
        if state.has_pending_for_stream(stream_id)? {
            let status = avd.decode_status();
            println!(
                "[apple-avd] destroying stream {} with pending decode; recovering decode engine without firmware restart status={:#x}",
                stream_id, status
            );
            avd.recover_decode_engine(status as u64);
        }
        state.destroy_session(stream_id)
    }

    fn submit_decode(&self, request: &VideoBackendDecodeRequest) -> Result<(), &'static str> {
        let _ = request;
        Err("apple-avd: stateful submit is unsupported; use stateless H.264 or VP9")
    }

    fn submit_h264_stateless(
        &self,
        request: &VideoBackendH264StatelessRequest,
    ) -> Result<(), &'static str> {
        self.validate_h264_request(&request.decode)?;
        let stream_parameters = H264StreamParameters::from_stateless_sps(&request.h264.sps)
            .map_err(h264_error_to_str)?;

        self.service_completions()?;
        let avd = self.avd()?;
        let _irq_guard = IrqGuard::new();
        let mut avd = avd.lock();
        let mut state = self.state.lock();
        if state.pending.is_some() {
            return Err("apple-avd: decode already pending");
        }
        avd.ensure_firmware_running()?;
        {
            let session =
                state.session_for_submit(request.decode.stream_id, request.decode.coded_format)?;
            session.stream_parameters = Some(stream_parameters);
        }
        self.submit_h264_prepared_locked(
            &mut avd,
            &mut state,
            &request.decode,
            stream_parameters,
            &request.h264,
        )
    }

    fn submit_vp9_stateless(
        &self,
        request: &VideoBackendVp9StatelessRequest,
    ) -> Result<(), &'static str> {
        let log_request = request.decode.timestamp <= 4;
        if log_request {
            println!(
                "[apple-avd] vp9 stateless ioctl begin stream={} ts={} input_len={} coded_format={}",
                request.decode.stream_id,
                request.decode.timestamp,
                request.decode.input_len,
                request.decode.coded_format
            );
        }
        self.validate_vp9_request(&request.decode)?;
        if log_request {
            println!("[apple-avd] vp9 stateless validate ok");
        }
        let stream_parameters = Vp9StreamParameters::from_stateless_frame(&request.vp9.frame)
            .map_err(vp9_error_to_str)?;
        if log_request {
            println!(
                "[apple-avd] vp9 stateless stream params ok coded={}x{} render={}x{} tiles={}x{}",
                stream_parameters.width,
                stream_parameters.height,
                stream_parameters.render_width,
                stream_parameters.render_height,
                1u32 << stream_parameters.tile_cols_log2,
                1u32 << stream_parameters.tile_rows_log2
            );
            println!("[apple-avd] vp9 service completions begin");
        }

        self.service_completions()?;
        if log_request {
            println!("[apple-avd] vp9 service completions ok");
            println!("[apple-avd] vp9 state lock begin");
        }
        let avd = self.avd()?;
        let _irq_guard = IrqGuard::new();
        let mut avd = avd.lock();
        let mut state = self.state.lock();
        if log_request {
            println!("[apple-avd] vp9 state lock ok");
        }
        if state.pending.is_some() {
            return Err("apple-avd: decode already pending");
        }
        if log_request {
            println!("[apple-avd] vp9 firmware ensure begin");
        }
        avd.ensure_firmware_running()?;
        if log_request {
            println!("[apple-avd] vp9 firmware ensure ok");
        }
        {
            let session =
                state.session_for_submit(request.decode.stream_id, request.decode.coded_format)?;
            session.stream_parameters = None;
        }
        if log_request {
            println!("[apple-avd] vp9 prepared submit begin");
        }
        self.submit_vp9_prepared_locked(
            &mut avd,
            &mut state,
            &request.decode,
            stream_parameters,
            &request.vp9,
        )?;
        if log_request {
            println!("[apple-avd] vp9 stateless ioctl ok");
        }
        Ok(())
    }

    fn dequeue_frame(
        &self,
        stream_id: u32,
    ) -> Result<Option<VideoBackendDecodedFrame>, &'static str> {
        self.service_completions()?;
        let _irq_guard = IrqGuard::new();
        let mut state = self.state.lock();
        let session_index = state.session_index(stream_id)?;
        if !state.sessions[session_index].active {
            return Err("apple-avd: inactive stream id");
        }
        let Some(index) = state
            .completed
            .position(|frame| frame.stream_id == stream_id)
        else {
            return Ok(None);
        };
        Ok(state.completed.remove(index))
    }
}

impl InterruptSource for AppleAvdVideoBackend {
    fn interrupt_id(&self) -> Option<InterruptId> {
        let _irq_guard = IrqGuard::new();
        *self.interrupt_id.lock()
    }

    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        let cause = self.avd_interrupt_cause()?;
        if !cause.is_asserted() {
            return Ok(InterruptClaim::NotMine);
        }

        if !self.has_pending_decode() {
            self.clear_stale_avd_interrupts()?;
            return Ok(InterruptClaim::Handled);
        }

        if let Err(error) = self.service_completions() {
            println!("[apple-avd] IRQ completion service failed: {}", error);
        }
        self.notify_completion();
        Ok(InterruptClaim::Handled)
    }
}

impl MaskableInterruptSource for AppleAvdVideoBackend {
    fn mask_source(&self) -> InterruptResult<()> {
        self.mask_avd_interrupts()
    }

    fn unmask_source(&self) -> InterruptResult<()> {
        self.unmask_avd_interrupts()
    }

    fn clear_pending_source(&self) -> InterruptResult<()> {
        self.clear_stale_avd_interrupts()
    }
}

fn finish_pending_decode(
    state: &mut AvdBackendState,
    pending: AvdPendingDecode,
    completion: AvdCompletionInfo,
) -> Result<(), &'static str> {
    arch::invalidate_dcache_to_poc_range(pending.output_payload_vaddr, pending.output_payload_len);
    if should_log_decode_completion(&pending, completion) {
        let sample = sample_output_payload(
            pending.output_payload_vaddr,
            pending.output_payload_len,
            pending.layout,
        );
        println!(
            "[apple-avd] decode complete stream={} frame={} idr={} ref={} slot={}/{} by_mailbox={} by_status={} status_before={:#x} status={:#x} msg={:#x} out={:#x}+{} sample y0={} u0={} v0={} y_words=[{:#x},{:#x},{:#x},{:#x}] y_hash={:#x} y_nonzero={} uv_words=[{:#x},{:#x}] uv_hash={:#x} uv_non_neutral={}",
            pending.stream_id,
            pending.frame_number,
            pending.is_idr,
            pending.store_reference,
            pending.reference_slot,
            pending.reference_slot_count,
            completion.by_mailbox,
            completion.by_status,
            completion.status_before,
            completion.status,
            completion.message_raw,
            pending.output_payload_dma,
            pending.output_payload_len,
            sample.y0,
            sample.u0,
            sample.v0,
            sample.y_first[0],
            sample.y_first[1],
            sample.y_first[2],
            sample.y_first[3],
            sample.y_checksum,
            sample.y_nonzero,
            sample.uv_first[0],
            sample.uv_first[1],
            sample.uv_checksum,
            sample.uv_non_neutral
        );
    }
    compact_nv12_output_payload(&pending)?;
    arch::clean_dcache_to_poc_range(pending.output_payload_vaddr, pending.payload_len);
    write_frame_header(
        pending.output_header_vaddr,
        pending.display_width,
        pending.display_height,
        pending.layout.pixel_format,
        pending.payload_len as u32,
    )?;
    arch::clean_dcache_to_poc_range(pending.output_header_vaddr, SCARLET_VIDEO_FRAME_HEADER_LEN);

    let session = state.session_mut(pending.stream_id)?;
    if pending.is_idr {
        session.clear_reference_frames();
    }
    if pending.store_reference {
        session.insert_reference_frame(AvdReferenceFrame {
            slot: pending.reference_slot,
            frame_number: pending.frame_number,
            timestamp: pending.timestamp,
            layout: pending.layout,
            reference_dma_addr: pending.reference_dma_addr,
            sps_tile_dma_addr: pending.sps_tile_dma_addr,
            vp9_rvra_dma_addrs: pending.vp9_rvra_dma_addrs,
            frame_num: pending.frame_num,
            pic_num: pending.pic_num,
            top_field_order_cnt: pending.top_field_order_cnt,
            long_term: pending.long_term,
        })?;
        session.trim_reference_frames(pending.dpb_capacity.max(1));
    }

    state.completed.push_back(VideoBackendDecodedFrame {
        stream_id: pending.stream_id,
        frame: ScarletVideoDequeuedFrame {
            width: pending.display_width,
            height: pending.display_height,
            pixel_format: pending.layout.pixel_format,
            payload_offset: pending.output_payload_offset as u64,
            payload_len: pending.payload_len as u32,
            flags: pending.command_tag,
            timestamp: pending.timestamp,
        },
    })?;
    Ok(())
}

fn compact_nv12_output_payload(pending: &AvdPendingDecode) -> Result<(), &'static str> {
    if pending.layout.pixel_format != SCARLET_VIDEO_PIXEL_FORMAT_NV12 {
        return Err("apple-avd: unsupported decoded pixel format");
    }
    if pending.display_width == 0 || pending.display_height == 0 {
        return Err("apple-avd: invalid display dimensions");
    }
    if pending.display_x > pending.layout.width
        || pending.display_y > pending.layout.height
        || pending.display_width > pending.layout.width - pending.display_x
        || pending.display_height > pending.layout.height - pending.display_y
    {
        return Err("apple-avd: display crop exceeds coded frame");
    }
    if pending.display_width > pending.layout.y_stride
        || pending.display_width > pending.layout.uv_stride
    {
        return Err("apple-avd: display width exceeds decoded stride");
    }
    if (pending.display_x & 1) != 0 || (pending.display_y & 1) != 0 {
        return Err("apple-avd: invalid NV12 crop alignment");
    }

    let expected_len = nv12_tight_payload_len(pending.display_width, pending.display_height)?;
    if expected_len != pending.payload_len || expected_len > pending.output_payload_len {
        return Err("apple-avd: invalid display payload length");
    }

    let y_stride = pending.layout.y_stride as usize;
    let uv_stride = pending.layout.uv_stride as usize;
    let coded_height = pending.layout.height as usize;
    let display_x = pending.display_x as usize;
    let display_y = pending.display_y as usize;
    let display_width = pending.display_width as usize;
    let display_height = pending.display_height as usize;
    let uv_height = display_height / 2;
    let source_y_end = plane_copy_end(
        display_y,
        display_height,
        y_stride,
        display_x,
        display_width,
    )
    .ok_or("apple-avd: display y source overflow")?;
    let source_uv_base = y_stride
        .checked_mul(coded_height)
        .ok_or("apple-avd: decoded UV offset overflow")?;
    let source_uv_end = source_uv_base
        .checked_add(
            plane_copy_end(
                display_y / 2,
                uv_height,
                uv_stride,
                display_x,
                display_width,
            )
            .ok_or("apple-avd: display UV source overflow")?,
        )
        .ok_or("apple-avd: display UV source overflow")?;
    if source_y_end > pending.output_payload_len || source_uv_end > pending.output_payload_len {
        return Err("apple-avd: display crop exceeds output payload");
    }

    if pending.display_x == 0
        && pending.display_y == 0
        && pending.display_width == pending.layout.width
        && pending.display_height == pending.layout.height
        && pending.layout.y_stride == pending.display_width
        && pending.layout.uv_stride == pending.display_width
    {
        return Ok(());
    }

    let base = pending.output_payload_vaddr;
    for row in 0..display_height {
        let src = base + (display_y + row) * y_stride + display_x;
        let dst = base + row * display_width;
        // SAFETY: Source and destination are validated to lie in the same
        // output payload. `copy` is used because in-place compaction can overlap.
        unsafe {
            core::ptr::copy(src as *const u8, dst as *mut u8, display_width);
        }
    }

    let dst_uv_base = base + display_width * display_height;
    for row in 0..uv_height {
        let src = base + source_uv_base + (display_y / 2 + row) * uv_stride + display_x;
        let dst = dst_uv_base + row * display_width;
        // SAFETY: Source and destination are validated to lie in the same
        // output payload. `copy` is used because in-place compaction can overlap.
        unsafe {
            core::ptr::copy(src as *const u8, dst as *mut u8, display_width);
        }
    }
    Ok(())
}

fn plane_copy_end(
    start_row: usize,
    rows: usize,
    stride: usize,
    x: usize,
    width: usize,
) -> Option<usize> {
    if rows == 0 {
        return x.checked_add(width);
    }
    start_row
        .checked_add(rows - 1)?
        .checked_mul(stride)?
        .checked_add(x)?
        .checked_add(width)
}

fn nv12_tight_payload_len(width: u32, height: u32) -> Result<usize, &'static str> {
    if width == 0 || height == 0 || (width & 1) != 0 || (height & 1) != 0 {
        return Err("apple-avd: invalid NV12 dimensions");
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|y_len| y_len.checked_add(y_len / 2))
        .ok_or("apple-avd: NV12 payload length overflow")
}

fn should_log_decode_completion(pending: &AvdPendingDecode, completion: AvdCompletionInfo) -> bool {
    should_log_decode_progress(pending.frame_number)
        || (completion.status & DECODE_STATUS_ERROR_MASK) != 0
}

fn should_log_decode_progress(_frame_number: u32) -> bool {
    _frame_number < AVD_DECODE_TRACE_FRAMES
        || (AVD_DECODE_PROGRESS_INTERVAL != 0 && _frame_number % AVD_DECODE_PROGRESS_INTERVAL == 0)
}

fn sample_output_payload(
    output_vaddr: usize,
    output_len: usize,
    layout: AvdDecodedLayout,
) -> AvdOutputSample {
    let y_sample_len = output_len.min(AVD_OUTPUT_SAMPLE_BYTES);
    let y_bytes = if y_sample_len == 0 {
        &[]
    } else {
        // SAFETY: `output_vaddr..output_vaddr + output_len` is a mapped
        // output buffer retained by the pending decode until completion.
        unsafe { core::slice::from_raw_parts(output_vaddr as *const u8, y_sample_len) }
    };

    let uv_offset = layout.y_stride as usize * layout.height as usize;
    let uv_sample_len = output_len
        .saturating_sub(uv_offset)
        .min(AVD_OUTPUT_UV_SAMPLE_BYTES);
    let uv_bytes = if uv_sample_len == 0 {
        &[]
    } else {
        // SAFETY: `uv_offset` was clamped against `output_len` above.
        unsafe {
            core::slice::from_raw_parts((output_vaddr + uv_offset) as *const u8, uv_sample_len)
        }
    };

    AvdOutputSample {
        y_first: [
            read_sample_word(y_bytes, 0),
            read_sample_word(y_bytes, 4),
            read_sample_word(y_bytes, 8),
            read_sample_word(y_bytes, 12),
        ],
        y_checksum: fnv1a32(y_bytes),
        y_nonzero: y_bytes.iter().filter(|byte| **byte != 0).count(),
        uv_first: [read_sample_word(uv_bytes, 0), read_sample_word(uv_bytes, 4)],
        uv_checksum: fnv1a32(uv_bytes),
        uv_non_neutral: uv_bytes.iter().filter(|byte| **byte != 0x80).count(),
        y0: y_bytes.first().copied().unwrap_or(0),
        u0: uv_bytes.first().copied().unwrap_or(0),
        v0: uv_bytes.get(1).copied().unwrap_or(0),
    }
}

fn read_sample_word(bytes: &[u8], offset: usize) -> u32 {
    let Some(word) = bytes.get(offset..offset + 4) else {
        return 0;
    };
    u32::from_le_bytes(word.try_into().expect("sample word bytes"))
}

fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for byte in bytes {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
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
        H264FrontendError::InvalidDimensions => "apple-avd: H.264 dimensions are invalid",
        H264FrontendError::UnsupportedSps => "apple-avd: H.264 SPS uses unsupported features",
        H264FrontendError::MalformedSlice => "apple-avd: H.264 slice header is malformed",
        H264FrontendError::InvalidSliceRange => "apple-avd: H.264 slice range is invalid",
        H264FrontendError::InstructionStreamTooLarge => {
            "apple-avd: generated H.264 instruction stream is too large"
        }
    }
}

fn vp9_error_to_str(error: Vp9FrontendError) -> &'static str {
    match error {
        Vp9FrontendError::InvalidDimensions => "apple-avd: VP9 dimensions are invalid",
        Vp9FrontendError::UnsupportedFrame => "apple-avd: VP9 frame uses unsupported features",
        Vp9FrontendError::InvalidTiles => "apple-avd: VP9 tile table is invalid",
        Vp9FrontendError::InstructionStreamTooLarge => {
            "apple-avd: generated VP9 instruction stream is too large"
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

    let mut enabled_paddrs = Vec::new();
    for paddr in paddrs {
        if enabled_paddrs.iter().any(|seen| *seen == paddr) {
            continue;
        }
        enabled_paddrs.push(paddr);
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
    let backend = Arc::new(AppleAvdVideoBackend::new(id));
    let interrupt_id = if let Some(irq_resource) = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::IRQ))
    {
        let interrupt_id = scarlet::interrupt::resolve_platform_irq(irq_resource)
            .map_err(|_| "apple-avd: failed to resolve IRQ")?;
        backend.set_interrupt_id(interrupt_id);
        let interrupt_source: Arc<dyn MaskableInterruptSource> = backend.clone();
        scarlet::interrupt::register_and_enable_interrupt_source(
            interrupt_source,
            arch::get_cpu().get_cpuid() as u32,
        )
        .map_err(|_| "apple-avd: failed to register IRQ handler")?;
        Some(interrupt_id)
    } else {
        println!("[apple-avd] no IRQ resource; completion will rely on polling");
        None
    };
    let video_backend: Arc<dyn VideoDecodeBackend> = backend.clone();
    let backend_id = register_video_backend(Arc::clone(&video_backend));
    let video_name = register_video_decode_device(Arc::clone(&video_backend));
    debug_device::register_avd_debug_device(id, Arc::clone(&video_backend));

    println!(
        "[apple-avd] registered {} id={} backend={} video={} soc={} mmio={:#x}+{:#x} irq={:?} interrupt={:?} reset={} status={:#x} irq_enable_status1={:#x} mailbox_status={:#x} mailbox_raw={:#x}",
        device.name(),
        id,
        backend_id,
        video_name,
        soc.name(),
        paddr,
        size,
        irq,
        interrupt_id,
        has_reset,
        snapshot.status,
        snapshot.irq_enable_status1,
        snapshot.mailbox_status,
        snapshot.mailbox_raw
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

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

fn check_workspace_window(
    offset: usize,
    len: usize,
    limit: usize,
    error: &'static str,
) -> Result<(), &'static str> {
    let end = offset.checked_add(len).ok_or(error)?;
    if end > limit {
        return Err(error);
    }
    Ok(())
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
