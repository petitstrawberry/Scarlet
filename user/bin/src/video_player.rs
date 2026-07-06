#![no_std]
#![no_main]
#![feature(portable_simd)]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::any::Any;
use core::simd::cmp::SimdOrd;
use core::simd::{
    Simd,
    num::{SimdInt, SimdUint},
};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use core::time::Duration;

use rust_h264::decoder::{Frame, OrderedDecoder};
use rust_h264::nal::parse_annex_b;
use sas_client::{SasClient, SasStream, StreamConfig};
use scarlet_ui::{
    Application, ApplicationRunExt, Canvas, CanvasView, Color, ComponentElement, Element, Event,
    InvalidationKind, KeyCode, KeyEvent, Listenable, MouseButton, MouseEvent, Scene, Size,
    SubscriptionId, View, ViewExt, Window, WindowGroup, graphics,
};
use std::audio::AUDIO_PCM_FORMAT_S16LE;
use std::fs::{File, OpenOptions};
use std::handle::capability::memory_mapping::{flags as mmap_flags, munmap, prot};
use std::io::{ErrorKind, Read, SeekFrom};
use std::socket::Socket;
use std::sync::Mutex;
use std::task::{SCHED_UTIL_SCALE, exit};
use std::{format, println, thread};
#[cfg(feature = "mp4-aac")]
use symphonia_codec_aac::AacDecoder;
#[cfg(feature = "mp4-aac")]
use symphonia_core::audio::layouts;
#[cfg(feature = "mp4-aac")]
use symphonia_core::codecs::audio::well_known::CODEC_ID_AAC;
#[cfg(feature = "mp4-aac")]
use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
#[cfg(feature = "mp4-aac")]
use symphonia_core::packet::PacketRef;
#[cfg(feature = "mp4-aac")]
use symphonia_core::units::{Duration as AudioDuration, Timestamp as AudioTimestamp};

const DEFAULT_VIDEO_PATH: &str = "/root/media/bad_apple.h264";
const APP_NAME: &str = env!("CARGO_BIN_NAME");
const VIDEO_WIDTH: u32 = 480;
const VIDEO_HEIGHT: u32 = 360;
const DISPLAY_WIDTH: u32 = 640;
const DISPLAY_HEIGHT: u32 = 360;
const FRAME_INTERVAL_MS: u64 = 33;
const CONTROLS_HIDE_INTERVAL_MS: u64 = 250;
const CONTROLS_HIDE_IDLE_TICKS: u32 = 6;
const STREAM_POLL_INTERVAL_MS: u64 = 25;
const STREAM_REORDER_HOLD_SAMPLES: usize = 8;
const STREAM_DECODE_BATCH_SAMPLES: usize = 8;
const STREAM_START_BUFFER_US: u64 = 1_000_000;
const STREAM_START_BUFFER_SAMPLES: usize = 24;
const DISPLAY_QUEUE_MAX_FRAMES: usize = 30;
const DISPLAY_QUEUE_MAX_BYTES: usize = 96 * 1024 * 1024;
const DECODE_TARGET_LEAD_FRAMES: usize = 10;
const AUDIO_CLOCK_START_TIMEOUT_MS: u64 = 3_000;
const LATE_VIDEO_DROP_THRESHOLD_US: u64 = 250_000;
const SEEK_COALESCE_DELAY_MS: u64 = 35;
const CONTROLS_MIN_WIDTH: u32 = 96;
const CONTROLS_MIN_HEIGHT: u32 = 48;
const CONTROLS_PANEL_HEIGHT: u32 = 34;
const PLAY_BUTTON_SIZE: u32 = 22;
const PLAY_BUTTON_LEFT_INSET: u32 = 10;
const PLAY_BUTTON_TOP_INSET: u32 = 6;
const LOOP_BUTTON_WIDTH: u32 = 24;
const LOOP_BUTTON_HEIGHT: u32 = 22;
const LOOP_BUTTON_LEFT_INSET: u32 = 38;
const SEEK_TRACK_LEFT_INSET: u32 = 74;
const SEEK_TRACK_RIGHT_INSET: u32 = 18;
const SEEK_TRACK_BOTTOM_INSET: u32 = 16;
const SEEK_TRACK_HEIGHT: u32 = 2;
const SEEK_TRACK_HIT_INSET: u32 = 10;
const SEEK_KNOB_WIDTH: u32 = 4;
const SEEK_KNOB_HEIGHT: u32 = 8;
const VIDEO_DEVICE_PATH: &str = "/dev/video0";
const SCARLET_VIDEO_FRAME_HEADER_LEN: usize = 20;
const NV12_VIDEO_RANGE_PIXEL_FORMAT: u32 = 0x3432_3076;
const SCARLET_VIDEO_GET_BUFFER: u32 = 0x5600;
const SCARLET_VIDEO_SUBMIT: u32 = 0x5601;
const SCARLET_VIDEO_DEQUEUE: u32 = 0x5602;
const SCARLET_VIDEO_CREATE_SESSION: u32 = 0x5603;
const SCARLET_VIDEO_SUBMIT_SESSION: u32 = 0x5604;
const SCARLET_VIDEO_DEQUEUE_SESSION: u32 = 0x5605;
const SCARLET_VIDEO_DESTROY_SESSION: u32 = 0x5606;
const SCARLET_VIDEO_GET_CAPS: u32 = 0x5607;
const SCARLET_VIDEO_SUBMIT_H264_STATELESS: u32 = 0x5608;
const SCARLET_VIDEO_CAPS_VERSION: u32 = 1;
const SCARLET_VIDEO_CAP_STATEFUL_H264: u32 = 1 << 0;
const SCARLET_VIDEO_CAP_STATEFUL_AV1: u32 = 1 << 1;
const SCARLET_VIDEO_CAP_STATELESS_H264: u32 = 1 << 8;
const SCARLET_VIDEO_CAP_MAPPED_BUFFERS: u32 = 1 << 16;
const SCARLET_VIDEO_CAP_SESSIONS: u32 = 1 << 17;
const SCARLET_VIDEO_H264_SPS_FLAG_SEPARATE_COLOUR_PLANE: u32 = 1 << 0;
const SCARLET_VIDEO_H264_SPS_FLAG_QPPRIME_Y_ZERO_TRANSFORM_BYPASS: u32 = 1 << 1;
const SCARLET_VIDEO_H264_SPS_FLAG_DELTA_PIC_ORDER_ALWAYS_ZERO: u32 = 1 << 2;
const SCARLET_VIDEO_H264_SPS_FLAG_GAPS_IN_FRAME_NUM_VALUE_ALLOWED: u32 = 1 << 3;
const SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY: u32 = 1 << 4;
const SCARLET_VIDEO_H264_SPS_FLAG_MB_ADAPTIVE_FRAME_FIELD: u32 = 1 << 5;
const SCARLET_VIDEO_H264_SPS_FLAG_DIRECT_8X8_INFERENCE: u32 = 1 << 6;
const SCARLET_VIDEO_H264_SPS_FLAG_FRAME_CROPPING: u32 = 1 << 7;
const SCARLET_VIDEO_H264_PPS_FLAG_ENTROPY_CODING_MODE: u16 = 1 << 0;
const SCARLET_VIDEO_H264_PPS_FLAG_BOTTOM_FIELD_PIC_ORDER_IN_FRAME_PRESENT: u16 = 1 << 1;
const SCARLET_VIDEO_H264_PPS_FLAG_WEIGHTED_PRED: u16 = 1 << 2;
const SCARLET_VIDEO_H264_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT: u16 = 1 << 3;
const SCARLET_VIDEO_H264_PPS_FLAG_CONSTRAINED_INTRA_PRED: u16 = 1 << 4;
const SCARLET_VIDEO_H264_PPS_FLAG_REDUNDANT_PIC_CNT_PRESENT: u16 = 1 << 5;
const SCARLET_VIDEO_H264_PPS_FLAG_TRANSFORM_8X8_MODE: u16 = 1 << 6;
const SCARLET_VIDEO_H264_SLICE_FLAG_DIRECT_SPATIAL_MV_PRED: u32 = 1 << 0;
const SCARLET_VIDEO_H264_SLICE_FLAG_REF_LISTS_PRESENT: u32 = 1 << 1;
const SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR: u32 = 1 << 0;
const SCARLET_VIDEO_H264_DPB_FLAG_VALID: u32 = 1 << 0;
const SCARLET_VIDEO_H264_DPB_FLAG_LONG_TERM: u32 = 1 << 1;
const VIRTIO_VIDEO_FORMAT_H264: u32 = 4098;
const VIRTIO_VIDEO_FORMAT_AV1: u32 = 4103;
const SCARLET_AV1_ACCESS_UNIT_MAGIC: &[u8; 4] = b"SVA1";
const VIDEO_DECODE_UTIL_MIN: u32 = SCHED_UTIL_SCALE * 7 / 8;
const VIDEO_DISPLAY_UTIL_MIN: u32 = SCHED_UTIL_SCALE * 3 / 4;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoBufferInfo {
    mmap_offset: u64,
    mmap_len: u64,
    input_offset: u64,
    input_len: u32,
    output_offset: u64,
    output_len: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoSubmit {
    input_len: u32,
    coded_format: u32,
    timestamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoDequeuedFrame {
    width: u32,
    height: u32,
    pixel_format: u32,
    payload_offset: u64,
    payload_len: u32,
    flags: u32,
    timestamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoSessionInfo {
    stream_id: u32,
    padding: u32,
    buffer: ScarletVideoBufferInfo,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoSessionSubmit {
    stream_id: u32,
    input_len: u32,
    coded_format: u32,
    padding: u32,
    timestamp: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoSessionDequeuedFrame {
    stream_id: u32,
    padding: u32,
    frame: ScarletVideoDequeuedFrame,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoCapabilities {
    version: u32,
    flags: u32,
    max_sessions: u32,
    output_pixel_format: u32,
    mapped_input_len: u32,
    mapped_output_len: u32,
    reserved: [u32; 8],
}

impl ScarletVideoCapabilities {
    fn has_flag(self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ScarletVideoH264Sps {
    profile_idc: u8,
    constraint_set_flags: u8,
    level_idc: u8,
    seq_parameter_set_id: u8,
    chroma_format_idc: u8,
    bit_depth_luma_minus8: u8,
    bit_depth_chroma_minus8: u8,
    log2_max_frame_num_minus4: u8,
    pic_order_cnt_type: u8,
    log2_max_pic_order_cnt_lsb_minus4: u8,
    max_num_ref_frames: u8,
    num_ref_frames_in_pic_order_cnt_cycle: u8,
    offset_for_ref_frame: [i32; 255],
    offset_for_non_ref_pic: i32,
    offset_for_top_to_bottom_field: i32,
    pic_width_in_mbs_minus1: u16,
    pic_height_in_map_units_minus1: u16,
    frame_crop_left_offset: u32,
    frame_crop_right_offset: u32,
    frame_crop_top_offset: u32,
    frame_crop_bottom_offset: u32,
    flags: u32,
}

impl Default for ScarletVideoH264Sps {
    fn default() -> Self {
        Self {
            profile_idc: 0,
            constraint_set_flags: 0,
            level_idc: 0,
            seq_parameter_set_id: 0,
            chroma_format_idc: 0,
            bit_depth_luma_minus8: 0,
            bit_depth_chroma_minus8: 0,
            log2_max_frame_num_minus4: 0,
            pic_order_cnt_type: 0,
            log2_max_pic_order_cnt_lsb_minus4: 0,
            max_num_ref_frames: 0,
            num_ref_frames_in_pic_order_cnt_cycle: 0,
            offset_for_ref_frame: [0; 255],
            offset_for_non_ref_pic: 0,
            offset_for_top_to_bottom_field: 0,
            pic_width_in_mbs_minus1: 0,
            pic_height_in_map_units_minus1: 0,
            frame_crop_left_offset: 0,
            frame_crop_right_offset: 0,
            frame_crop_top_offset: 0,
            frame_crop_bottom_offset: 0,
            flags: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoH264Pps {
    pic_parameter_set_id: u8,
    seq_parameter_set_id: u8,
    num_slice_groups_minus1: u8,
    num_ref_idx_l0_default_active_minus1: u8,
    num_ref_idx_l1_default_active_minus1: u8,
    weighted_bipred_idc: u8,
    pic_init_qp_minus26: i8,
    pic_init_qs_minus26: i8,
    chroma_qp_index_offset: i8,
    second_chroma_qp_index_offset: i8,
    flags: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ScarletVideoH264ScalingMatrix {
    scaling_list_4x4: [[u8; 16]; 6],
    scaling_list_8x8: [[u8; 64]; 6],
}

impl Default for ScarletVideoH264ScalingMatrix {
    fn default() -> Self {
        Self {
            scaling_list_4x4: [[0; 16]; 6],
            scaling_list_8x8: [[0; 64]; 6],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoH264Reference {
    fields: u8,
    index: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoH264WeightFactors {
    luma_weight: [i16; 32],
    luma_offset: [i16; 32],
    chroma_weight: [[i16; 2]; 32],
    chroma_offset: [[i16; 2]; 32],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoH264PredWeights {
    luma_log2_weight_denom: u16,
    chroma_log2_weight_denom: u16,
    weight_factors: [ScarletVideoH264WeightFactors; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ScarletVideoH264SliceParams {
    header_bit_size: u32,
    nal_offset: u32,
    nal_len: u32,
    first_mb_in_slice: u32,
    slice_type: u8,
    pic_parameter_set_id: u8,
    colour_plane_id: u8,
    redundant_pic_cnt: u8,
    cabac_init_idc: u8,
    slice_qp_delta: i8,
    slice_qs_delta: i8,
    disable_deblocking_filter_idc: u8,
    slice_alpha_c0_offset_div2: i8,
    slice_beta_offset_div2: i8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    reserved: u8,
    ref_pic_list0: [ScarletVideoH264Reference; 32],
    ref_pic_list1: [ScarletVideoH264Reference; 32],
    flags: u32,
}

impl Default for ScarletVideoH264SliceParams {
    fn default() -> Self {
        Self {
            header_bit_size: 0,
            nal_offset: 0,
            nal_len: 0,
            first_mb_in_slice: 0,
            slice_type: 0,
            pic_parameter_set_id: 0,
            colour_plane_id: 0,
            redundant_pic_cnt: 0,
            cabac_init_idc: 0,
            slice_qp_delta: 0,
            slice_qs_delta: 0,
            disable_deblocking_filter_idc: 0,
            slice_alpha_c0_offset_div2: 0,
            slice_beta_offset_div2: 0,
            num_ref_idx_l0_active_minus1: 0,
            num_ref_idx_l1_active_minus1: 0,
            reserved: 0,
            ref_pic_list0: [ScarletVideoH264Reference::default(); 32],
            ref_pic_list1: [ScarletVideoH264Reference::default(); 32],
            flags: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoH264DpbEntry {
    reference_ts: u64,
    pic_num: i32,
    frame_num: u16,
    fields: u8,
    reserved: [u8; 5],
    top_field_order_cnt: i32,
    bottom_field_order_cnt: i32,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoH264DecodeParams {
    dpb: [ScarletVideoH264DpbEntry; 16],
    nal_ref_idc: u16,
    frame_num: u16,
    top_field_order_cnt: i32,
    bottom_field_order_cnt: i32,
    idr_pic_id: u16,
    pic_order_cnt_lsb: u16,
    delta_pic_order_cnt_bottom: i32,
    delta_pic_order_cnt0: i32,
    delta_pic_order_cnt1: i32,
    dec_ref_pic_marking_bit_size: u32,
    pic_order_cnt_bit_size: u32,
    slice_group_change_cycle: u32,
    reserved: u32,
    flags: u32,
}

#[derive(Clone, Copy, Default)]
struct ScarletVideoH264StatelessParams {
    sps: ScarletVideoH264Sps,
    pps: ScarletVideoH264Pps,
    scaling_matrix: ScarletVideoH264ScalingMatrix,
    pred_weights: ScarletVideoH264PredWeights,
    slice_params: ScarletVideoH264SliceParams,
    decode_params: ScarletVideoH264DecodeParams,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoH264ParamPtrs {
    sps: u64,
    pps: u64,
    scaling_matrix: u64,
    pred_weights: u64,
    slice_params: u64,
    decode_params: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ScarletVideoH264StatelessSubmit {
    stream_id: u32,
    input_len: u32,
    timestamp: u64,
    params: ScarletVideoH264ParamPtrs,
    flags: u32,
    padding: u32,
}

struct VideoFrameStore {
    data: Mutex<VideoFrameData>,
}

struct VideoFrameData {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    current_frame: u32,
    total_frames: u32,
}

impl VideoFrameStore {
    fn new() -> Self {
        Self {
            data: Mutex::new(VideoFrameData {
                pixels: vec![0; (VIDEO_WIDTH * VIDEO_HEIGHT * 4) as usize],
                width: VIDEO_WIDTH,
                height: VIDEO_HEIGHT,
                current_frame: 0,
                total_frames: 0,
            }),
        }
    }

    fn update_from_frame(&self, frame: &Frame, current_frame: u32, total_frames: u32) {
        let width = frame.width;
        let height = frame.height;
        let mut data = self.data.lock();
        let required_len = width as usize * height as usize * 4;
        if data.pixels.len() != required_len {
            data.pixels.resize(required_len, 0);
        }
        yuv420_to_bgra(frame, &mut data.pixels);
        data.width = width;
        data.height = height;
        data.current_frame = current_frame;
        data.total_frames = total_frames;
    }

    fn update_from_nv12(
        &self,
        frame: &ScarletVideoFrame,
        current_frame: u32,
        total_frames: u32,
    ) -> Result<(), String> {
        let width = frame.width;
        let height = frame.height;
        if frame.pixel_format != NV12_VIDEO_RANGE_PIXEL_FORMAT {
            return Err(format!(
                "hardware decoder returned unsupported pixel format 0x{:08x}",
                frame.pixel_format
            ));
        }
        let required_nv12_len = width as usize * height as usize * 3 / 2;
        let payload = frame.payload();
        if payload.len() < required_nv12_len {
            return Err(String::from(
                "hardware decoder returned truncated NV12 frame",
            ));
        }

        let mut data = self.data.lock();
        let required_len = width as usize * height as usize * 4;
        if data.pixels.len() != required_len {
            data.pixels.resize(required_len, 0);
        }
        nv12_to_bgra(width, height, payload, &mut data.pixels);
        data.width = width;
        data.height = height;
        data.current_frame = current_frame;
        data.total_frames = total_frames;
        Ok(())
    }

    fn mark_complete(&self) {
        let mut data = self.data.lock();
        if data.total_frames != 0 {
            data.current_frame = data.total_frames;
        }
    }

    fn reset_for_replay(&self) {
        let mut data = self.data.lock();
        data.current_frame = 0;
    }
}

struct ControlsOverlay {
    visible: AtomicBool,
    debug_visible: AtomicBool,
    loop_enabled: AtomicBool,
    activity_epoch: AtomicU32,
    paused: AtomicBool,
    pause_after_seek: AtomicBool,
    scrubbing: AtomicBool,
    finished: AtomicBool,
    replay_epoch: AtomicU32,
    seek_epoch: AtomicU32,
    video_ready_seek_epoch: AtomicU32,
    seek_target_us: AtomicU64,
    desired_position_us: AtomicU64,
    media_duration_us: AtomicU64,
    buffered_position_us: AtomicU64,
    canvas_width: AtomicU32,
    canvas_height: AtomicU32,
    presented_frames: AtomicU64,
    dropped_frames: AtomicU64,
    fps_display_x10: AtomicU32,
    fps_window_frames: AtomicU64,
    fps_window_start_us: AtomicU64,
    last_clock_us: AtomicU64,
    last_video_pts_us: AtomicU64,
    last_lag_us: AtomicU64,
}

#[derive(Clone, Copy)]
struct UiScale {
    milli: u32,
}

impl UiScale {
    fn current() -> Self {
        Self {
            milli: graphics::current_scale_milli().max(1),
        }
    }

    fn logical_len(self, physical: u32) -> u32 {
        ((u64::from(physical) * 1000 + u64::from(self.milli).saturating_sub(1))
            / u64::from(self.milli))
        .max(1) as u32
    }

    fn physical_pos(self, logical: u32) -> u32 {
        (u64::from(logical) * u64::from(self.milli) / 1000) as u32
    }

    fn physical_len(self, logical: u32) -> u32 {
        ((u64::from(logical) * u64::from(self.milli) + 999) / 1000).max(1) as u32
    }

    fn physical_i32(self, logical: i32) -> i32 {
        ((i64::from(logical) * i64::from(self.milli)) / 1000) as i32
    }

    fn physical_font(self, logical: f32) -> f32 {
        logical * (self.milli as f32) / 1000.0
    }
}

impl ControlsOverlay {
    fn new(loop_enabled: bool) -> Self {
        Self {
            visible: AtomicBool::new(false),
            debug_visible: AtomicBool::new(false),
            loop_enabled: AtomicBool::new(loop_enabled),
            activity_epoch: AtomicU32::new(0),
            paused: AtomicBool::new(false),
            pause_after_seek: AtomicBool::new(false),
            scrubbing: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            replay_epoch: AtomicU32::new(0),
            seek_epoch: AtomicU32::new(0),
            video_ready_seek_epoch: AtomicU32::new(u32::MAX),
            seek_target_us: AtomicU64::new(0),
            desired_position_us: AtomicU64::new(0),
            media_duration_us: AtomicU64::new(0),
            buffered_position_us: AtomicU64::new(0),
            canvas_width: AtomicU32::new(DISPLAY_WIDTH),
            canvas_height: AtomicU32::new(DISPLAY_HEIGHT),
            presented_frames: AtomicU64::new(0),
            dropped_frames: AtomicU64::new(0),
            fps_display_x10: AtomicU32::new(0),
            fps_window_frames: AtomicU64::new(0),
            fps_window_start_us: AtomicU64::new(u64::MAX),
            last_clock_us: AtomicU64::new(0),
            last_video_pts_us: AtomicU64::new(0),
            last_lag_us: AtomicU64::new(0),
        }
    }

    fn is_visible(&self) -> bool {
        self.visible.load(Ordering::Acquire)
    }

    fn show_for_mouse_activity(&self) -> bool {
        self.activity_epoch.fetch_add(1, Ordering::AcqRel);
        self.visible.swap(true, Ordering::AcqRel) != true
    }

    fn hide(&self) -> bool {
        self.visible.swap(false, Ordering::AcqRel) != false
    }

    fn activity_epoch(&self) -> u32 {
        self.activity_epoch.load(Ordering::Acquire)
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    fn is_debug_visible(&self) -> bool {
        self.debug_visible.load(Ordering::Acquire)
    }

    fn is_loop_enabled(&self) -> bool {
        self.loop_enabled.load(Ordering::Acquire)
    }

    fn toggle_paused(&self) {
        let paused = !self.paused.load(Ordering::Acquire);
        self.paused.store(paused, Ordering::Release);
        self.show_for_mouse_activity();
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn mark_finished(&self) {
        self.finished.store(true, Ordering::Release);
        self.paused.store(false, Ordering::Release);
    }

    fn request_replay(&self) {
        self.desired_position_us.store(0, Ordering::Release);
        self.seek_target_us.store(0, Ordering::Release);
        self.pause_after_seek.store(false, Ordering::Release);
        self.paused.store(false, Ordering::Release);
        self.reset_fps_window();
        self.seek_epoch.fetch_add(1, Ordering::AcqRel);
        self.replay_epoch.fetch_add(1, Ordering::AcqRel);
        self.finished.store(false, Ordering::Release);
    }

    fn current_replay_epoch(&self) -> u32 {
        self.replay_epoch.load(Ordering::Acquire)
    }

    fn set_media_duration_us(&self, duration_us: u64) {
        let mut current = self.media_duration_us.load(Ordering::Acquire);
        while duration_us > current {
            match self.media_duration_us.compare_exchange_weak(
                current,
                duration_us,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    fn media_duration_us(&self) -> u64 {
        self.media_duration_us.load(Ordering::Acquire)
    }

    fn set_buffered_position_us(&self, buffered_position_us: u64) -> bool {
        let mut current = self.buffered_position_us.load(Ordering::Acquire);
        while buffered_position_us > current {
            match self.buffered_position_us.compare_exchange_weak(
                current,
                buffered_position_us,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
        false
    }

    fn current_seek_epoch(&self) -> u32 {
        self.seek_epoch.load(Ordering::Acquire)
    }

    fn current_seek_target_us(&self) -> u64 {
        self.seek_target_us.load(Ordering::Acquire)
    }

    fn mark_video_ready_for_seek(&self, seek_epoch: u32) {
        self.video_ready_seek_epoch
            .store(seek_epoch, Ordering::Release);
        if self.current_seek_epoch() == seek_epoch
            && self.pause_after_seek.swap(false, Ordering::AcqRel)
        {
            self.paused.store(true, Ordering::Release);
        }
    }

    fn is_video_ready_for_seek(&self, seek_epoch: u32) -> bool {
        self.video_ready_seek_epoch.load(Ordering::Acquire) == seek_epoch
    }

    fn wait_for_video_seek_ready(&self, seek_epoch: u32) -> bool {
        while self.video_ready_seek_epoch.load(Ordering::Acquire) != seek_epoch {
            if self.current_seek_epoch() != seek_epoch {
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
        true
    }

    fn request_seek_to_us(&self, target_us: u64) {
        let duration_us = self.media_duration_us();
        let target_us = if duration_us == 0 {
            target_us
        } else {
            target_us.min(duration_us.saturating_sub(1))
        };
        let previous_target = self.desired_position_us.swap(target_us, Ordering::AcqRel);
        let was_finished = self.finished.swap(false, Ordering::AcqRel);
        if previous_target == target_us && !was_finished {
            self.show_for_mouse_activity();
            return;
        }
        self.seek_target_us.store(target_us, Ordering::Release);
        let pause_after_seek =
            !was_finished && (self.is_paused() || self.pause_after_seek.load(Ordering::Acquire));
        self.pause_after_seek
            .store(pause_after_seek, Ordering::Release);
        self.paused.store(false, Ordering::Release);
        self.reset_fps_window();
        self.seek_epoch.fetch_add(1, Ordering::AcqRel);
        self.show_for_mouse_activity();
    }

    fn request_relative_seek_ms(&self, delta_ms: i64) {
        let seek_epoch = self.current_seek_epoch();
        let current_us = if self.is_video_ready_for_seek(seek_epoch) {
            self.last_video_pts_us.load(Ordering::Acquire)
        } else {
            self.current_seek_target_us()
        };
        let delta_us = delta_ms.saturating_mul(1_000);
        let target_us = if delta_us >= 0 {
            current_us.saturating_add(delta_us as u64)
        } else {
            current_us.saturating_sub(delta_us.unsigned_abs())
        };
        self.request_seek_to_us(target_us);
    }

    fn set_scrubbing(&self, scrubbing: bool) {
        self.scrubbing.store(scrubbing, Ordering::Release);
    }

    fn is_scrubbing(&self) -> bool {
        self.scrubbing.load(Ordering::Acquire)
    }

    fn toggle_debug(&self) {
        let visible = !self.debug_visible.load(Ordering::Acquire);
        self.debug_visible.store(visible, Ordering::Release);
        self.show_for_mouse_activity();
    }

    fn toggle_loop(&self) {
        let enabled = !self.loop_enabled.load(Ordering::Acquire);
        self.loop_enabled.store(enabled, Ordering::Release);
        self.show_for_mouse_activity();
    }

    fn record_presented_frame(&self, presentation_time_us: u64, clock_time_us: Option<u64>) {
        self.presented_frames.fetch_add(1, Ordering::AcqRel);
        self.record_video_timing(presentation_time_us, clock_time_us);
    }

    fn record_preview_frame(&self, presentation_time_us: u64) {
        self.last_clock_us
            .store(presentation_time_us, Ordering::Release);
        self.last_video_pts_us
            .store(presentation_time_us, Ordering::Release);
        self.last_lag_us.store(0, Ordering::Release);
    }

    fn record_dropped_frame(&self, presentation_time_us: u64, clock_time_us: Option<u64>) {
        self.dropped_frames.fetch_add(1, Ordering::AcqRel);
        self.record_video_timing(presentation_time_us, clock_time_us);
    }

    fn reset_fps_window(&self) {
        self.fps_window_start_us.store(u64::MAX, Ordering::Release);
        self.fps_window_frames.store(
            self.presented_frames.load(Ordering::Acquire),
            Ordering::Release,
        );
    }

    fn record_video_timing(&self, presentation_time_us: u64, clock_time_us: Option<u64>) {
        let clock_time_us = clock_time_us.unwrap_or(presentation_time_us);
        const FPS_WINDOW_US: u64 = 1_000_000;
        let window_start = self.fps_window_start_us.load(Ordering::Acquire);
        let elapsed = clock_time_us.saturating_sub(window_start);
        if window_start == u64::MAX || elapsed >= FPS_WINDOW_US {
            if window_start != u64::MAX && elapsed > 0 {
                let window_frames = self
                    .presented_frames
                    .load(Ordering::Acquire)
                    .saturating_sub(self.fps_window_frames.load(Ordering::Acquire));
                self.fps_display_x10.store(
                    (window_frames.saturating_mul(10_000_000) / elapsed) as u32,
                    Ordering::Release,
                );
            }
            self.fps_window_start_us
                .store(clock_time_us, Ordering::Release);
            self.fps_window_frames.store(
                self.presented_frames.load(Ordering::Acquire),
                Ordering::Release,
            );
        }
        self.last_clock_us.store(clock_time_us, Ordering::Release);
        self.last_video_pts_us
            .store(presentation_time_us, Ordering::Release);
        if self.is_video_ready_for_seek(self.current_seek_epoch()) {
            self.desired_position_us
                .store(presentation_time_us, Ordering::Release);
        }
        self.last_lag_us.store(
            clock_time_us.saturating_sub(presentation_time_us),
            Ordering::Release,
        );
    }

    fn update_canvas_size(&self, width: u32, height: u32) {
        self.canvas_width.store(width, Ordering::Release);
        self.canvas_height.store(height, Ordering::Release);
    }

    fn play_pause_button_contains(&self, x: i32, y: i32) -> bool {
        let width = self.canvas_width.load(Ordering::Acquire);
        let height = self.canvas_height.load(Ordering::Acquire);
        let Some((button_x, button_y)) = play_pause_button_origin(width, height) else {
            return false;
        };
        let Ok(x) = u32::try_from(x) else {
            return false;
        };
        let Ok(y) = u32::try_from(y) else {
            return false;
        };

        x >= button_x
            && x < button_x + PLAY_BUTTON_SIZE
            && y >= button_y
            && y < button_y + PLAY_BUTTON_SIZE
    }

    fn loop_button_contains(&self, x: i32, y: i32) -> bool {
        let width = self.canvas_width.load(Ordering::Acquire);
        let height = self.canvas_height.load(Ordering::Acquire);
        let Some((button_x, button_y)) = loop_button_origin(width, height) else {
            return false;
        };
        let Ok(x) = u32::try_from(x) else {
            return false;
        };
        let Ok(y) = u32::try_from(y) else {
            return false;
        };

        x >= button_x
            && x < button_x + LOOP_BUTTON_WIDTH
            && y >= button_y
            && y < button_y + LOOP_BUTTON_HEIGHT
    }
}

struct PaintSignal {
    next_subscription: AtomicU32,
    subscribers: Mutex<BTreeMap<SubscriptionId, Arc<dyn Fn() + Send + Sync>>>,
}

impl PaintSignal {
    fn new() -> Self {
        Self {
            next_subscription: AtomicU32::new(0),
            subscribers: Mutex::new(BTreeMap::new()),
        }
    }

    fn notify(&self) {
        let subscribers = self.subscribers.lock();
        for callback in subscribers.values() {
            callback();
        }
    }
}

impl Listenable for PaintSignal {
    fn subscribe_any(&self, callback: Arc<dyn Fn() + Send + Sync>) -> SubscriptionId {
        let id = SubscriptionId::new(self.next_subscription.fetch_add(1, Ordering::Relaxed));
        self.subscribers.lock().insert(id, callback);
        id
    }

    fn unsubscribe(&self, id: SubscriptionId) -> bool {
        self.subscribers.lock().remove(&id).is_some()
    }

    fn invalidation_kind(&self) -> InvalidationKind {
        InvalidationKind::Paint
    }
}

#[derive(Clone)]
struct VideoPlayerApp {
    path: String,
    window_title: String,
    mp4_data: Option<Arc<Vec<u8>>>,
    audio_source: Option<PlayerAudioSource>,
    hardware_decode: bool,
    streaming: bool,
    loop_playback: bool,
    stream_complete_path: Option<String>,
    stream_socket_path: Option<String>,
    frame_store: Arc<VideoFrameStore>,
    controls: Arc<ControlsOverlay>,
    paint_signal: Arc<PaintSignal>,
    clock: Arc<AudioClock>,
}

#[derive(Clone)]
enum PlayerAudioSource {
    Wav(String),
    Mp4Aac(Arc<Vec<u8>>),
    StreamingMp4Aac {
        path: String,
        complete_path: Option<String>,
    },
    StreamingMp4AacSocket {
        socket_path: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AudioPlaybackStatus {
    Completed,
    Interrupted,
}

impl VideoPlayerApp {
    fn new(
        path: String,
        window_title: String,
        mp4_data: Option<Arc<Vec<u8>>>,
        audio_source: Option<PlayerAudioSource>,
        hardware_decode: bool,
        streaming: bool,
        loop_playback: bool,
        stream_complete_path: Option<String>,
        stream_socket_path: Option<String>,
    ) -> Self {
        Self {
            path,
            window_title,
            mp4_data,
            audio_source,
            hardware_decode,
            streaming,
            loop_playback,
            stream_complete_path,
            stream_socket_path,
            frame_store: Arc::new(VideoFrameStore::new()),
            controls: Arc::new(ControlsOverlay::new(loop_playback)),
            paint_signal: Arc::new(PaintSignal::new()),
            clock: Arc::new(AudioClock::new()),
        }
    }
}

struct AudioClock {
    video_ready: AtomicBool,
    started: AtomicBool,
    finished: AtomicBool,
    unavailable: AtomicBool,
    sample_rate: AtomicU64,
    base_frames: AtomicU64,
    read_frames: AtomicU64,
    loop_duration_us: AtomicU64,
}

impl AudioClock {
    fn new() -> Self {
        Self {
            video_ready: AtomicBool::new(false),
            started: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
            sample_rate: AtomicU64::new(48_000),
            base_frames: AtomicU64::new(0),
            read_frames: AtomicU64::new(0),
            loop_duration_us: AtomicU64::new(0),
        }
    }

    fn mark_video_ready(&self) {
        self.video_ready.store(true, Ordering::Release);
    }

    fn mark_started(&self, sample_rate: u32) {
        self.sample_rate
            .store(u64::from(sample_rate), Ordering::Release);
        self.finished.store(false, Ordering::Release);
        self.started.store(true, Ordering::Release);
    }

    fn mark_finished(&self) {
        self.finished.store(true, Ordering::Release);
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn mark_unavailable(&self) {
        self.unavailable.store(true, Ordering::Release);
    }

    fn is_unavailable(&self) -> bool {
        self.unavailable.load(Ordering::Acquire)
    }

    fn update_read_frames(&self, read_frames: u64) {
        let base_frames = self.base_frames.load(Ordering::Acquire);
        self.read_frames
            .store(base_frames.saturating_add(read_frames), Ordering::Release);
    }

    fn advance_base_frames(&self, frames: u64) {
        self.base_frames.fetch_add(frames, Ordering::AcqRel);
    }

    fn set_start_position_us(&self, start_us: u64, sample_rate: u32) {
        let rate = u64::from(sample_rate).max(1);
        let base_frames = start_us.saturating_mul(rate) / 1_000_000;
        self.sample_rate.store(rate, Ordering::Release);
        self.base_frames.store(base_frames, Ordering::Release);
        self.read_frames.store(base_frames, Ordering::Release);
        self.finished.store(false, Ordering::Release);
    }

    fn set_loop_duration_us(&self, duration_us: u64) {
        self.loop_duration_us
            .store(duration_us.max(1), Ordering::Release);
    }

    fn loop_duration_us(&self) -> Option<u64> {
        let duration = self.loop_duration_us.load(Ordering::Acquire);
        (duration != 0).then_some(duration)
    }

    fn wait_until_video_ready(&self) -> bool {
        while !self.video_ready.load(Ordering::Acquire) {
            if self.unavailable.load(Ordering::Acquire) {
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
        true
    }

    fn elapsed_us(&self) -> Option<u64> {
        if self.unavailable.load(Ordering::Acquire) {
            return None;
        }
        if !self.started.load(Ordering::Acquire) {
            return None;
        }
        let rate = self.sample_rate.load(Ordering::Acquire).max(1);
        let audio_frames = self.read_frames.load(Ordering::Acquire);
        Some(audio_frames.saturating_mul(1_000_000) / rate)
    }

    fn reset_for_replay(&self) {
        self.base_frames.store(0, Ordering::Release);
        self.read_frames.store(0, Ordering::Release);
        self.started.store(false, Ordering::Release);
        self.finished.store(false, Ordering::Release);
        self.video_ready.store(false, Ordering::Release);
        self.unavailable.store(false, Ordering::Release);
    }

    /// Reset audio timing for replay without touching `video_ready`.
    /// The decoder thread owns `video_ready` — it clears it before
    /// decoding and sets it again via `mark_video_ready()`.
    /// If the audio thread also cleared `video_ready` it could race
    /// with the decoder and deadlock.
    fn reset_for_replay_audio(&self) {
        self.base_frames.store(0, Ordering::Release);
        self.read_frames.store(0, Ordering::Release);
        self.started.store(false, Ordering::Release);
        self.finished.store(false, Ordering::Release);
        // intentionally skip video_ready
        self.unavailable.store(false, Ordering::Release);
    }
}

impl View for VideoPlayerApp {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(ComponentElement::new(self.clone()))
    }

    fn listenables(&self) -> Vec<&dyn scarlet_ui::Listenable> {
        vec![self.paint_signal.as_ref()]
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Application for VideoPlayerApp {
    fn scenes(&self) -> impl Scene {
        let frame_store = self.frame_store.clone();
        let controls = self.controls.clone();
        let controls_for_event = self.controls.clone();
        let paint_signal_for_event = self.paint_signal.clone();
        let controls_for_key = self.controls.clone();
        let paint_signal_for_key = self.paint_signal.clone();
        WindowGroup::new(
            "main",
            Window::new(
                self.window_title.clone(),
                CanvasView::new(
                    DISPLAY_WIDTH as f32,
                    DISPLAY_HEIGHT as f32,
                    Rc::new(move |buffer, width, height| {
                        draw_video_frame(buffer, width, height, &frame_store, &controls);
                    }),
                )
                .on_event(move |event| {
                    handle_canvas_event(event, &controls_for_event, &paint_signal_for_event)
                })
                .on_key(move |event| {
                    handle_key_event(event, &controls_for_key, &paint_signal_for_key)
                }),
            )
            .app_id("org.scarlet-os.video-player")
            .size(Size::new(DISPLAY_WIDTH as f32, DISPLAY_HEIGHT as f32)),
        )
    }

    fn init(&mut self) {
        start_controls_thread(self.controls.clone(), self.paint_signal.clone());
        if let Some(audio_source) = self.audio_source.clone() {
            start_audio_thread(
                audio_source,
                self.clock.clone(),
                self.controls.clone(),
                self.loop_playback,
            );
        }
        start_decoder_thread(
            self.path.clone(),
            self.mp4_data.clone(),
            self.frame_store.clone(),
            self.paint_signal.clone(),
            self.controls.clone(),
            self.audio_source.is_some().then(|| self.clock.clone()),
            self.hardware_decode,
            self.streaming,
            self.stream_complete_path.clone(),
            self.stream_socket_path.clone(),
        );
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn start_decoder_thread(
    path: String,
    mp4_data: Option<Arc<Vec<u8>>>,
    frame_store: Arc<VideoFrameStore>,
    paint_signal: Arc<PaintSignal>,
    controls: Arc<ControlsOverlay>,
    clock: Option<Arc<AudioClock>>,
    hardware_decode: bool,
    streaming: bool,
    stream_complete_path: Option<String>,
    stream_socket_path: Option<String>,
) {
    thread::Builder::new()
        .name("video-decode")
        .util_min(VIDEO_DECODE_UTIL_MIN)
        .spawn(move || {
            let display_queue = Arc::new(DisplayQueue::new(clock.clone()));
            start_display_thread(
                frame_store.clone(),
                paint_signal.clone(),
                controls.clone(),
                clock.clone(),
                display_queue.clone(),
            );

            let result = if hardware_decode {
                if streaming {
                    if let Some(socket_path) = stream_socket_path.as_deref() {
                        decode_loop_hardware_streaming_mp4_socket(
                            socket_path,
                            &frame_store,
                            &paint_signal,
                            &controls,
                            clock.as_deref(),
                            &display_queue,
                        )
                    } else if is_mp4_path(&path) {
                        decode_loop_hardware_streaming_mp4(
                            &path,
                            stream_complete_path.as_deref(),
                            &frame_store,
                            &paint_signal,
                            &controls,
                            clock.as_deref(),
                            &display_queue,
                        )
                    } else {
                        Err(String::from(
                            "hardware streaming decode currently requires an MP4 stream",
                        ))
                    }
                } else {
                    decode_loop_hardware(
                        &path,
                        mp4_data.as_deref().map(Vec::as_slice),
                        &frame_store,
                        &paint_signal,
                        &controls,
                        clock.as_deref(),
                        &display_queue,
                    )
                }
            } else {
                decode_loop_software(
                    &path,
                    mp4_data.as_deref().map(Vec::as_slice),
                    &controls,
                    clock.as_deref(),
                    &display_queue,
                )
            };

            if let Err(err) = result {
                if let Some(clock) = clock.as_deref() {
                    clock.mark_unavailable();
                }
                println!("[{}] {}", APP_NAME, err);
            }
            display_queue.close();
        })
        .expect("failed to spawn video decoder thread");
}

fn start_display_thread(
    frame_store: Arc<VideoFrameStore>,
    paint_signal: Arc<PaintSignal>,
    controls: Arc<ControlsOverlay>,
    clock: Option<Arc<AudioClock>>,
    queue: Arc<DisplayQueue>,
) {
    thread::Builder::new()
        .name("video-display")
        .util_min(VIDEO_DISPLAY_UTIL_MIN)
        .spawn(move || {
            loop {
                let Some(item) = queue.pop() else {
                    break;
                };
                match item {
                    DisplayItem::Frame {
                        frame,
                        presentation_time_us,
                        display_index,
                        total_frames,
                        seek_epoch,
                    } => {
                        if controls.current_seek_epoch() != seek_epoch {
                            continue;
                        }
                        match pace_frame(
                            &controls,
                            presentation_time_us,
                            seek_epoch,
                            clock.as_deref(),
                        ) {
                            PaceDecision::Stale => continue,
                            PaceDecision::Drop { sync_time_us } => {
                                controls.record_dropped_frame(presentation_time_us, sync_time_us);
                            }
                            PaceDecision::Present { sync_time_us } => {
                                if let Err(err) = publish_frame(
                                    &frame_store,
                                    &paint_signal,
                                    &controls,
                                    frame,
                                    display_index,
                                    total_frames,
                                ) {
                                    println!("[{}] {}", APP_NAME, err);
                                    continue;
                                }
                                controls.mark_video_ready_for_seek(seek_epoch);
                                if let Some(clock) = clock.as_deref() {
                                    clock.mark_video_ready();
                                }
                                controls.record_presented_frame(presentation_time_us, sync_time_us);
                            }
                        }
                    }
                    DisplayItem::EndOfPass { seek_epoch } => {
                        if controls.current_seek_epoch() != seek_epoch {
                            continue;
                        }
                        frame_store.mark_complete();
                        paint_signal.notify();
                        let _ = wait_for_replay_request(&controls);
                        frame_store.reset_for_replay();
                        paint_signal.notify();
                    }
                }
            }
        })
        .expect("failed to spawn video display thread");
}

fn start_controls_thread(controls: Arc<ControlsOverlay>, paint_signal: Arc<PaintSignal>) {
    thread::spawn(move || {
        let mut last_epoch = controls.activity_epoch();
        let mut idle_ticks = 0u32;

        loop {
            thread::sleep(Duration::from_millis(CONTROLS_HIDE_INTERVAL_MS));
            let epoch = controls.activity_epoch();
            if epoch != last_epoch {
                last_epoch = epoch;
                idle_ticks = 0;
                continue;
            }

            if controls.is_visible() {
                idle_ticks = idle_ticks.saturating_add(1);
                if idle_ticks >= CONTROLS_HIDE_IDLE_TICKS {
                    idle_ticks = 0;
                    if controls.hide() {
                        paint_signal.notify();
                    }
                }
            } else {
                idle_ticks = 0;
            }
        }
    });
}

fn decode_loop_software(
    path: &str,
    mp4_data: Option<&[u8]>,
    controls: &ControlsOverlay,
    clock: Option<&AudioClock>,
    queue: &DisplayQueue,
) -> Result<(), String> {
    let source = load_video_source(path, mp4_data)?;
    if source
        .access_units
        .iter()
        .any(|unit| unit.codec != VideoCodec::H264)
    {
        return Err(String::from(
            "software decoder supports only H.264; use --hwdc for this video",
        ));
    }
    let total_frames = source.access_units.len().max(1) as u32;
    let mut access_unit_scratch = Vec::new();
    println!(
        "[{}] software decode: {} {} access units",
        APP_NAME,
        source.description(),
        source.access_units.len()
    );
    let loop_duration_us = video_source_duration_us(&source);
    controls.set_media_duration_us(loop_duration_us);
    controls.set_buffered_position_us(loop_duration_us);
    let mut loop_index = 0u64;
    let mut seek_epoch = controls.current_seek_epoch();
    let mut seek_target_us = 0u64;

    loop {
        let mut decoder = OrderedDecoder::<u64>::new();
        let seek_plan = video_seek_plan(&source, seek_target_us);
        let mut display_index = seek_plan.publish_start_rank;
        let loop_time_offset_us = video_loop_time_offset_us(clock, loop_duration_us, loop_index);
        let mut restart_for_seek = false;

        for access_unit in &source.access_units[seek_plan.decode_start_index..] {
            if let Some(target_us) = consume_seek_request(controls, &mut seek_epoch) {
                queue.clear();
                seek_target_us = target_us;
                loop_index = 0;
                restart_for_seek = true;
                break;
            }
            wait_while_paused(controls);
            let access_unit_bytes = access_unit.bytes(mp4_data, &mut access_unit_scratch)?;
            let nals = parse_annex_b(access_unit_bytes);
            let unit_presentation_time_us = access_unit
                .presentation_time_us
                .saturating_add(loop_time_offset_us);
            for nal in &nals {
                match decoder.decode_nal_with_meta(nal, unit_presentation_time_us) {
                    Ok(frames) => {
                        for (frame, presentation_time_us) in frames {
                            if presentation_time_us
                                < seek_plan
                                    .publish_target_us
                                    .saturating_add(loop_time_offset_us)
                            {
                                continue;
                            }
                            match queue.push_frame(
                                DisplayItem::frame(
                                    DecodedVideoFrame::Software(frame),
                                    presentation_time_us,
                                    display_index,
                                    total_frames,
                                    seek_epoch,
                                ),
                                controls,
                            ) {
                                QueuePush::Pushed => {
                                    display_index += 1;
                                }
                                QueuePush::StaleEpoch => {
                                    queue.clear();
                                    seek_target_us = controls.current_seek_target_us();
                                    seek_epoch = controls.current_seek_epoch();
                                    loop_index = 0;
                                    restart_for_seek = true;
                                    break;
                                }
                                QueuePush::Closed => return Ok(()),
                            }
                        }
                        if restart_for_seek {
                            break;
                        }
                    }
                    Err(err) => return Err(format!("decode failed: {err}")),
                }
            }
            if restart_for_seek {
                break;
            }
        }

        if restart_for_seek {
            continue;
        }
        for (frame, presentation_time_us) in decoder.flush_with_meta() {
            if presentation_time_us
                < seek_plan
                    .publish_target_us
                    .saturating_add(loop_time_offset_us)
            {
                continue;
            }
            match queue.push_frame(
                DisplayItem::frame(
                    DecodedVideoFrame::Software(frame),
                    presentation_time_us,
                    display_index,
                    total_frames,
                    seek_epoch,
                ),
                controls,
            ) {
                QueuePush::Pushed => {
                    display_index += 1;
                }
                QueuePush::StaleEpoch => {
                    queue.clear();
                    seek_target_us = controls.current_seek_target_us();
                    seek_epoch = controls.current_seek_epoch();
                    loop_index = 0;
                    restart_for_seek = true;
                    break;
                }
                QueuePush::Closed => return Ok(()),
            }
        }

        if restart_for_seek {
            continue;
        }

        if !controls.is_loop_enabled() {
            match queue.push_frame(DisplayItem::EndOfPass { seek_epoch }, controls) {
                QueuePush::Pushed => {}
                QueuePush::StaleEpoch => {
                    queue.clear();
                    seek_target_us = controls.current_seek_target_us();
                    seek_epoch = controls.current_seek_epoch();
                    loop_index = 0;
                    continue;
                }
                QueuePush::Closed => return Ok(()),
            }
            println!("[{}] finished: {} frames", APP_NAME, display_index);
            seek_target_us = wait_for_replay_or_seek_request(controls).unwrap_or(0);
            if let Some(clock) = clock {
                clock.reset_for_replay();
            }
            queue.clear();
            seek_epoch = controls.current_seek_epoch();
            loop_index = 0;
            continue;
        }
        println!(
            "[{}] loop {} complete: {} frames",
            APP_NAME,
            loop_index + 1,
            display_index
        );
        loop_index = loop_index.saturating_add(1);
    }
}

fn decode_loop_hardware(
    path: &str,
    mp4_data: Option<&[u8]>,
    frame_store: &VideoFrameStore,
    paint_signal: &PaintSignal,
    controls: &ControlsOverlay,
    clock: Option<&AudioClock>,
    queue: &DisplayQueue,
) -> Result<(), String> {
    let source = load_video_source(path, mp4_data)?;
    let total_frames = source.access_units.len().max(1) as u32;
    let mut access_unit_scratch = Vec::new();
    println!(
        "[{}] hardware decode: {} {} access units",
        APP_NAME,
        source.description(),
        source.access_units.len()
    );
    let loop_duration_us = video_source_duration_us(&source);
    controls.set_media_duration_us(loop_duration_us);
    controls.set_buffered_position_us(loop_duration_us);
    let mut loop_index = 0u64;
    let mut seek_epoch = controls.current_seek_epoch();
    let mut seek_target_us = 0u64;

    loop {
        let seek_plan = video_seek_plan(&source, seek_target_us);
        let mut reorder = FrameReorderBuffer::new_from(total_frames, seek_plan.publish_start_rank);
        let loop_time_offset_us = video_loop_time_offset_us(clock, loop_duration_us, loop_index);
        let mut restart_for_seek = false;

        if seek_epoch != 0 || seek_target_us != 0 {
            if !publish_hardware_seek_preview(
                &source,
                mp4_data,
                &seek_plan,
                frame_store,
                paint_signal,
                controls,
                &mut access_unit_scratch,
                total_frames,
                seek_epoch,
            )? {
                seek_target_us = controls.current_seek_target_us();
                seek_epoch = controls.current_seek_epoch();
                loop_index = 0;
                continue;
            }
        }

        let mut decoder = HardwareVideoDecoder::open()?;
        for access_unit in &source.access_units[seek_plan.decode_start_index..] {
            if let Some(target_us) = consume_seek_request(controls, &mut seek_epoch) {
                queue.clear();
                seek_target_us = target_us;
                loop_index = 0;
                restart_for_seek = true;
                break;
            }
            wait_while_paused(controls);
            let access_unit_bytes = access_unit.bytes(mp4_data, &mut access_unit_scratch)?;
            let Some(frame) = decoder.decode_access_unit(access_unit.codec, access_unit_bytes)?
            else {
                return Err(String::from("hardware decoder produced no frame"));
            };
            let presentation_time_us = access_unit
                .presentation_time_us
                .saturating_add(loop_time_offset_us);
            if !video_should_publish_after_seek(access_unit, seek_plan.publish_target_us) {
                continue;
            }
            if reorder.can_publish_immediately(access_unit.display_rank) {
                if !reorder.publish_immediate(
                    controls,
                    queue,
                    presentation_time_us,
                    seek_epoch,
                    DecodedVideoFrame::Hardware(frame),
                )? {
                    queue.clear();
                    seek_target_us = controls.current_seek_target_us();
                    seek_epoch = controls.current_seek_epoch();
                    loop_index = 0;
                    restart_for_seek = true;
                    break;
                }
            } else {
                reorder.push(
                    access_unit.display_rank,
                    presentation_time_us,
                    DecodedVideoFrame::Hardware(frame.into_owned()),
                )?;
                if !reorder.publish_ready(controls, queue, seek_epoch)? {
                    queue.clear();
                    seek_target_us = controls.current_seek_target_us();
                    seek_epoch = controls.current_seek_epoch();
                    loop_index = 0;
                    restart_for_seek = true;
                    break;
                }
            }
        }
        if restart_for_seek {
            continue;
        }
        if !reorder.finish(controls, queue, seek_epoch)? {
            queue.clear();
            seek_target_us = controls.current_seek_target_us();
            seek_epoch = controls.current_seek_epoch();
            loop_index = 0;
            continue;
        }

        if !controls.is_loop_enabled() {
            match queue.push_frame(DisplayItem::EndOfPass { seek_epoch }, controls) {
                QueuePush::Pushed => {}
                QueuePush::StaleEpoch => {
                    queue.clear();
                    seek_target_us = controls.current_seek_target_us();
                    seek_epoch = controls.current_seek_epoch();
                    loop_index = 0;
                    continue;
                }
                QueuePush::Closed => return Ok(()),
            }
            println!("[{}] finished: {} frames", APP_NAME, reorder.published());
            seek_target_us = wait_for_replay_or_seek_request(controls).unwrap_or(0);
            if let Some(clock) = clock {
                clock.reset_for_replay();
            }
            queue.clear();
            seek_epoch = controls.current_seek_epoch();
            loop_index = 0;
            continue;
        }
        println!(
            "[{}] loop {} complete: {} frames",
            APP_NAME,
            loop_index + 1,
            reorder.published()
        );
        loop_index = loop_index.saturating_add(1);
    }
}

fn decode_loop_hardware_streaming_mp4(
    path: &str,
    complete_path: Option<&str>,
    frame_store: &VideoFrameStore,
    paint_signal: &PaintSignal,
    controls: &ControlsOverlay,
    clock: Option<&AudioClock>,
    queue: &DisplayQueue,
) -> Result<(), String> {
    let mut data = Vec::new();
    let mut decoder = HardwareVideoDecoder::open()?;
    let mut reorder = FrameReorderBuffer::new(u32::MAX);
    let mut access_unit_scratch = Vec::new();
    let mut decoded = 0usize;
    let mut announced = false;
    let mut last_log_len = 0usize;
    let mut last_log_samples = 0usize;
    let mut logged_first_decode = false;
    let mut seek_epoch = controls.current_seek_epoch();
    let mut seek_target_us = controls.current_seek_target_us();
    let mut active_seek_epoch = seek_epoch;
    let mut active_publish_target_us = 0u64;
    let mut active_preview_published = false;

    loop {
        append_growing_file(path, &mut data)?;
        let complete = complete_path.map(marker_exists).unwrap_or(false);
        let source = match load_mp4_video_source_with_options(&data, false, true) {
            Ok(source) => source,
            Err(err) if complete => return Err(err),
            Err(_) => {
                if data.len() != last_log_len || complete {
                    println!(
                        "[{}] stream waiting for video samples bytes={} complete={}",
                        APP_NAME,
                        data.len(),
                        complete
                    );
                    last_log_len = data.len();
                }
                thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
                continue;
            }
        };
        if source
            .access_units
            .iter()
            .any(|unit| unit.codec != VideoCodec::H264)
        {
            return Err(String::from(
                "streaming hardware decode currently supports MP4/H.264",
            ));
        }
        if !announced {
            println!(
                "[{}] hardware stream decode: {}",
                APP_NAME,
                source.description()
            );
            announced = true;
        }

        let available = source.access_units.len();
        controls.set_media_duration_us(video_source_duration_us(&source));
        if controls.set_buffered_position_us(video_source_duration_us(&source)) {
            paint_signal.notify();
        }
        reorder.set_total_frames(stream_total_frames(&source, decoded, complete));
        if decoded == 0 && !complete && !stream_start_buffer_ready(&source) {
            if data.len() != last_log_len || available != last_log_samples {
                println!(
                    "[{}] stream prebuffering video bytes={} samples={} target={}ms",
                    APP_NAME,
                    data.len(),
                    available,
                    STREAM_START_BUFFER_US / 1_000
                );
                last_log_len = data.len();
                last_log_samples = available;
            }
            thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
            continue;
        }
        let decode_limit = if complete {
            available
        } else {
            available.saturating_sub(STREAM_REORDER_HOLD_SAMPLES)
        };
        if let Some(target_us) = consume_seek_request(controls, &mut seek_epoch) {
            queue.clear();
            seek_target_us = target_us;
        }
        if active_seek_epoch != seek_epoch {
            let seek_plan = video_seek_plan(&source, seek_target_us);
            if !complete && !stream_seek_target_available(&source, seek_plan.publish_target_us) {
                thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
                continue;
            }
            drop(decoder);
            if let Some(clock) = clock {
                clock.reset_for_replay();
            }
            decoder = HardwareVideoDecoder::open()?;
            reorder = FrameReorderBuffer::new_from(
                stream_total_frames(&source, seek_plan.decode_start_index, complete),
                seek_plan.publish_start_rank,
            );
            decoded = seek_plan.decode_start_index;
            active_seek_epoch = seek_epoch;
            active_publish_target_us = seek_plan.publish_target_us;
            active_preview_published = false;
            logged_first_decode = false;
        }
        if !controls.is_scrubbing() && active_preview_published {
            let seek_plan = video_seek_plan(&source, seek_target_us);
            drop(decoder);
            decoder = HardwareVideoDecoder::open()?;
            reorder = FrameReorderBuffer::new_from(
                stream_total_frames(&source, seek_plan.decode_start_index, complete),
                seek_plan.publish_start_rank,
            );
            decoded = seek_plan.decode_start_index;
            active_publish_target_us = seek_plan.publish_target_us;
            active_preview_published = false;
            logged_first_decode = false;
        }
        if controls.is_scrubbing() && active_preview_published {
            thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
            continue;
        }
        if decode_limit <= decoded {
            if data.len() != last_log_len || available != last_log_samples || complete {
                println!(
                    "[{}] stream buffered video bytes={} samples={} decoded={} complete={}",
                    APP_NAME,
                    data.len(),
                    available,
                    decoded,
                    complete
                );
                last_log_len = data.len();
                last_log_samples = available;
            }
            if complete && decoded >= available {
                break;
            }
            thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
            continue;
        }

        let batch_limit = decode_limit.min(decoded.saturating_add(STREAM_DECODE_BATCH_SAMPLES));
        for access_unit in &source.access_units[decoded..batch_limit] {
            if let Some(target_us) = consume_seek_request(controls, &mut seek_epoch) {
                queue.clear();
                seek_target_us = target_us;
                break;
            }
            if controls.is_paused() {
                thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
                break;
            }
            let access_unit_bytes = access_unit.bytes(Some(&data), &mut access_unit_scratch)?;
            if !logged_first_decode {
                println!(
                    "[{}] stream decoding first frame sample={} bytes={} pts_us={}",
                    APP_NAME,
                    decoded,
                    access_unit_bytes.len(),
                    access_unit.presentation_time_us
                );
            }
            let Some(frame) = decoder.decode_access_unit(access_unit.codec, access_unit_bytes)?
            else {
                return Err(String::from("hardware decoder produced no frame"));
            };
            if !logged_first_decode {
                println!("[{}] stream decoded first frame", APP_NAME);
                logged_first_decode = true;
            }
            if access_unit.presentation_time_us < active_publish_target_us {
                decoded += 1;
                continue;
            }
            if controls.is_scrubbing() {
                publish_seek_preview(
                    frame_store,
                    paint_signal,
                    controls,
                    DecodedVideoFrame::Hardware(frame),
                    access_unit.display_rank,
                    stream_total_frames(&source, decoded, complete),
                    access_unit.presentation_time_us,
                    active_seek_epoch,
                )?;
                active_preview_published = true;
                decoded += 1;
                break;
            }
            if reorder.can_publish_immediately(access_unit.display_rank) {
                if !reorder.publish_immediate(
                    controls,
                    queue,
                    access_unit.presentation_time_us,
                    active_seek_epoch,
                    DecodedVideoFrame::Hardware(frame),
                )? {
                    queue.clear();
                    seek_epoch = controls.current_seek_epoch();
                    seek_target_us = controls.current_seek_target_us();
                    break;
                }
            } else {
                reorder.push(
                    access_unit.display_rank,
                    access_unit.presentation_time_us,
                    DecodedVideoFrame::Hardware(frame.into_owned()),
                )?;
                if !reorder.publish_ready(controls, queue, active_seek_epoch)? {
                    queue.clear();
                    seek_epoch = controls.current_seek_epoch();
                    seek_target_us = controls.current_seek_target_us();
                    break;
                }
            }
            decoded += 1;
        }
    }

    if !reorder.finish(controls, queue, controls.current_seek_epoch())? {
        queue.clear();
    }
    drop(decoder);

    if controls.is_loop_enabled() {
        let source = load_mp4_video_source_with_options(&data, false, true)?;
        if source
            .access_units
            .iter()
            .any(|unit| unit.codec != VideoCodec::H264)
        {
            return Err(String::from(
                "streaming hardware decode currently supports MP4/H.264",
            ));
        }
        println!(
            "[{}] stream loop source ready: {} frames",
            APP_NAME,
            source.access_units.len()
        );
        replay_hardware_source_loops(
            &source,
            &data,
            frame_store,
            paint_signal,
            controls,
            clock,
            queue,
            1,
        )?;
    } else {
        let seek_epoch = controls.current_seek_epoch();
        match queue.push_frame(DisplayItem::EndOfPass { seek_epoch }, controls) {
            QueuePush::Pushed => {}
            QueuePush::StaleEpoch => queue.clear(),
            QueuePush::Closed => return Ok(()),
        }
        println!("[{}] finished: {} frames", APP_NAME, reorder.published());

        let _ = wait_for_replay_or_seek_request(controls);
        if let Some(clock) = clock {
            clock.reset_for_replay();
        }
        queue.clear();
        let source = load_mp4_video_source_with_options(&data, false, true)?;
        if source
            .access_units
            .iter()
            .any(|unit| unit.codec != VideoCodec::H264)
        {
            return Err(String::from(
                "streaming hardware decode currently supports MP4/H.264",
            ));
        }
        replay_hardware_source_loops(
            &source,
            &data,
            frame_store,
            paint_signal,
            controls,
            clock,
            queue,
            0,
        )?;
    }
    println!("[{}] finished: {} frames", APP_NAME, reorder.published());
    Ok(())
}

fn decode_loop_hardware_streaming_mp4_socket(
    socket_path: &str,
    frame_store: &VideoFrameStore,
    paint_signal: &PaintSignal,
    controls: &ControlsOverlay,
    clock: Option<&AudioClock>,
    queue: &DisplayQueue,
) -> Result<(), String> {
    let socket_state = start_stream_socket_reader(String::from(socket_path));

    let mut data = Vec::new();
    let mut decoder = HardwareVideoDecoder::open()?;
    let mut reorder = FrameReorderBuffer::new(u32::MAX);
    let mut access_unit_scratch = Vec::new();
    let mut decoded = 0usize;
    let mut complete = false;
    let mut announced = false;
    let mut last_log_len = 0usize;
    let mut last_log_samples = 0usize;
    let mut logged_first_decode = false;
    let mut seek_epoch = controls.current_seek_epoch();
    let mut seek_target_us = controls.current_seek_target_us();
    let mut active_seek_epoch = seek_epoch;
    let mut active_publish_target_us = 0u64;
    let mut active_preview_published = false;

    loop {
        {
            let state = socket_state.lock();
            if let Some(error) = state.error.as_ref() {
                return Err(error.clone());
            }
            if state.data.len() != data.len() || state.complete != complete {
                data = state.data.clone();
                complete = state.complete;
            }
        }

        let source = match load_mp4_video_source_with_options(&data, false, true) {
            Ok(source) => source,
            Err(err) if complete => return Err(err),
            Err(_) => {
                if data.len() != last_log_len || complete {
                    println!(
                        "[{}] stream waiting for video samples bytes={} complete={}",
                        APP_NAME,
                        data.len(),
                        complete
                    );
                    last_log_len = data.len();
                }
                thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
                continue;
            }
        };
        if source
            .access_units
            .iter()
            .any(|unit| unit.codec != VideoCodec::H264)
        {
            return Err(String::from(
                "streaming hardware decode currently supports MP4/H.264",
            ));
        }
        if !announced {
            println!(
                "[{}] hardware stream decode: {}",
                APP_NAME,
                source.description()
            );
            announced = true;
        }

        let available = source.access_units.len();
        controls.set_media_duration_us(video_source_duration_us(&source));
        if controls.set_buffered_position_us(video_source_duration_us(&source)) {
            paint_signal.notify();
        }
        reorder.set_total_frames(stream_total_frames(&source, decoded, complete));
        if decoded == 0 && !complete && !stream_start_buffer_ready(&source) {
            if data.len() != last_log_len || available != last_log_samples {
                println!(
                    "[{}] stream prebuffering video bytes={} samples={} target={}ms",
                    APP_NAME,
                    data.len(),
                    available,
                    STREAM_START_BUFFER_US / 1_000
                );
                last_log_len = data.len();
                last_log_samples = available;
            }
            thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
            continue;
        }
        let decode_limit = if complete {
            available
        } else {
            available.saturating_sub(STREAM_REORDER_HOLD_SAMPLES)
        };
        if let Some(target_us) = consume_seek_request(controls, &mut seek_epoch) {
            queue.clear();
            seek_target_us = target_us;
        }
        if active_seek_epoch != seek_epoch {
            let seek_plan = video_seek_plan(&source, seek_target_us);
            if !complete && !stream_seek_target_available(&source, seek_plan.publish_target_us) {
                thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
                continue;
            }
            drop(decoder);
            if let Some(clock) = clock {
                clock.reset_for_replay();
            }
            decoder = HardwareVideoDecoder::open()?;
            reorder = FrameReorderBuffer::new_from(
                stream_total_frames(&source, seek_plan.decode_start_index, complete),
                seek_plan.publish_start_rank,
            );
            decoded = seek_plan.decode_start_index;
            active_seek_epoch = seek_epoch;
            active_publish_target_us = seek_plan.publish_target_us;
            active_preview_published = false;
            logged_first_decode = false;
        }
        if !controls.is_scrubbing() && active_preview_published {
            let seek_plan = video_seek_plan(&source, seek_target_us);
            drop(decoder);
            decoder = HardwareVideoDecoder::open()?;
            reorder = FrameReorderBuffer::new_from(
                stream_total_frames(&source, seek_plan.decode_start_index, complete),
                seek_plan.publish_start_rank,
            );
            decoded = seek_plan.decode_start_index;
            active_publish_target_us = seek_plan.publish_target_us;
            active_preview_published = false;
            logged_first_decode = false;
        }
        if controls.is_scrubbing() && active_preview_published {
            thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
            continue;
        }
        if decode_limit <= decoded {
            if data.len() != last_log_len || available != last_log_samples || complete {
                println!(
                    "[{}] stream buffered video bytes={} samples={} decoded={} complete={}",
                    APP_NAME,
                    data.len(),
                    available,
                    decoded,
                    complete
                );
                last_log_len = data.len();
                last_log_samples = available;
            }
            if complete && decoded >= available {
                break;
            }
            thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
            continue;
        }

        let batch_limit = decode_limit.min(decoded.saturating_add(STREAM_DECODE_BATCH_SAMPLES));
        for access_unit in &source.access_units[decoded..batch_limit] {
            if let Some(target_us) = consume_seek_request(controls, &mut seek_epoch) {
                queue.clear();
                seek_target_us = target_us;
                break;
            }
            if controls.is_paused() {
                thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
                break;
            }
            let access_unit_bytes = access_unit.bytes(Some(&data), &mut access_unit_scratch)?;
            if !logged_first_decode {
                println!(
                    "[{}] stream decoding first frame sample={} bytes={} pts_us={}",
                    APP_NAME,
                    decoded,
                    access_unit_bytes.len(),
                    access_unit.presentation_time_us
                );
            }
            let Some(frame) = decoder.decode_access_unit(access_unit.codec, access_unit_bytes)?
            else {
                return Err(String::from("hardware decoder produced no frame"));
            };
            if !logged_first_decode {
                println!("[{}] stream decoded first frame", APP_NAME);
                logged_first_decode = true;
            }
            if access_unit.presentation_time_us < active_publish_target_us {
                decoded += 1;
                continue;
            }
            if controls.is_scrubbing() {
                publish_seek_preview(
                    frame_store,
                    paint_signal,
                    controls,
                    DecodedVideoFrame::Hardware(frame),
                    access_unit.display_rank,
                    stream_total_frames(&source, decoded, complete),
                    access_unit.presentation_time_us,
                    active_seek_epoch,
                )?;
                active_preview_published = true;
                decoded += 1;
                break;
            }
            if reorder.can_publish_immediately(access_unit.display_rank) {
                if !reorder.publish_immediate(
                    controls,
                    queue,
                    access_unit.presentation_time_us,
                    active_seek_epoch,
                    DecodedVideoFrame::Hardware(frame),
                )? {
                    queue.clear();
                    seek_epoch = controls.current_seek_epoch();
                    seek_target_us = controls.current_seek_target_us();
                    break;
                }
            } else {
                reorder.push(
                    access_unit.display_rank,
                    access_unit.presentation_time_us,
                    DecodedVideoFrame::Hardware(frame.into_owned()),
                )?;
                if !reorder.publish_ready(controls, queue, active_seek_epoch)? {
                    queue.clear();
                    seek_epoch = controls.current_seek_epoch();
                    seek_target_us = controls.current_seek_target_us();
                    break;
                }
            }
            decoded += 1;
        }
    }

    if !reorder.finish(controls, queue, controls.current_seek_epoch())? {
        queue.clear();
    }
    drop(decoder);

    if controls.is_loop_enabled() {
        let source = load_mp4_video_source_with_options(&data, false, true)?;
        if source
            .access_units
            .iter()
            .any(|unit| unit.codec != VideoCodec::H264)
        {
            return Err(String::from(
                "streaming hardware decode currently supports MP4/H.264",
            ));
        }
        println!(
            "[{}] stream loop source ready: {} frames",
            APP_NAME,
            source.access_units.len()
        );
        replay_hardware_source_loops(
            &source,
            &data,
            frame_store,
            paint_signal,
            controls,
            clock,
            queue,
            1,
        )?;
    } else {
        let seek_epoch = controls.current_seek_epoch();
        match queue.push_frame(DisplayItem::EndOfPass { seek_epoch }, controls) {
            QueuePush::Pushed => {}
            QueuePush::StaleEpoch => queue.clear(),
            QueuePush::Closed => return Ok(()),
        }
        println!("[{}] finished: {} frames", APP_NAME, reorder.published());

        let _ = wait_for_replay_or_seek_request(controls);
        if let Some(clock) = clock {
            clock.reset_for_replay();
        }
        queue.clear();
        let source = load_mp4_video_source_with_options(&data, false, true)?;
        if source
            .access_units
            .iter()
            .any(|unit| unit.codec != VideoCodec::H264)
        {
            return Err(String::from(
                "streaming hardware decode currently supports MP4/H.264",
            ));
        }
        replay_hardware_source_loops(
            &source,
            &data,
            frame_store,
            paint_signal,
            controls,
            clock,
            queue,
            0,
        )?;
    }
    println!("[{}] finished: {} frames", APP_NAME, reorder.published());
    Ok(())
}

fn replay_hardware_source_loops(
    source: &VideoSource,
    mp4_data: &[u8],
    frame_store: &VideoFrameStore,
    paint_signal: &PaintSignal,
    controls: &ControlsOverlay,
    clock: Option<&AudioClock>,
    queue: &DisplayQueue,
    mut loop_index: u64,
) -> Result<(), String> {
    let total_frames = source.access_units.len().max(1) as u32;
    let loop_duration_us = video_source_duration_us(source);
    controls.set_media_duration_us(loop_duration_us);
    controls.set_buffered_position_us(loop_duration_us);
    let mut access_unit_scratch = Vec::new();
    let mut seek_epoch = controls.current_seek_epoch();
    let mut seek_target_us = 0u64;

    loop {
        let seek_plan = video_seek_plan(source, seek_target_us);
        let mut reorder = FrameReorderBuffer::new_from(total_frames, seek_plan.publish_start_rank);
        let loop_time_offset_us = video_loop_time_offset_us(clock, loop_duration_us, loop_index);
        let mut restart_for_seek = false;

        if seek_epoch != 0 || seek_target_us != 0 {
            if !publish_hardware_seek_preview(
                source,
                Some(mp4_data),
                &seek_plan,
                frame_store,
                paint_signal,
                controls,
                &mut access_unit_scratch,
                total_frames,
                seek_epoch,
            )? {
                seek_target_us = controls.current_seek_target_us();
                seek_epoch = controls.current_seek_epoch();
                loop_index = 0;
                continue;
            }
        }

        let mut decoder = HardwareVideoDecoder::open()?;
        for access_unit in &source.access_units[seek_plan.decode_start_index..] {
            if let Some(target_us) = consume_seek_request(controls, &mut seek_epoch) {
                queue.clear();
                seek_target_us = target_us;
                loop_index = 0;
                restart_for_seek = true;
                break;
            }
            wait_while_paused(controls);
            let access_unit_bytes = access_unit.bytes(Some(mp4_data), &mut access_unit_scratch)?;
            let Some(frame) = decoder.decode_access_unit(access_unit.codec, access_unit_bytes)?
            else {
                return Err(String::from("hardware decoder produced no frame"));
            };
            let presentation_time_us = access_unit
                .presentation_time_us
                .saturating_add(loop_time_offset_us);
            if !video_should_publish_after_seek(access_unit, seek_plan.publish_target_us) {
                continue;
            }
            if reorder.can_publish_immediately(access_unit.display_rank) {
                if !reorder.publish_immediate(
                    controls,
                    queue,
                    presentation_time_us,
                    seek_epoch,
                    DecodedVideoFrame::Hardware(frame),
                )? {
                    queue.clear();
                    seek_target_us = controls.current_seek_target_us();
                    seek_epoch = controls.current_seek_epoch();
                    loop_index = 0;
                    restart_for_seek = true;
                    break;
                }
            } else {
                reorder.push(
                    access_unit.display_rank,
                    presentation_time_us,
                    DecodedVideoFrame::Hardware(frame.into_owned()),
                )?;
                if !reorder.publish_ready(controls, queue, seek_epoch)? {
                    queue.clear();
                    seek_target_us = controls.current_seek_target_us();
                    seek_epoch = controls.current_seek_epoch();
                    loop_index = 0;
                    restart_for_seek = true;
                    break;
                }
            }
        }

        if restart_for_seek {
            continue;
        }
        if !reorder.finish(controls, queue, seek_epoch)? {
            queue.clear();
            seek_target_us = controls.current_seek_target_us();
            seek_epoch = controls.current_seek_epoch();
            loop_index = 0;
            continue;
        }
        if controls.is_loop_enabled() {
            println!(
                "[{}] loop {} complete: {} frames",
                APP_NAME,
                loop_index + 1,
                reorder.published()
            );
            loop_index = loop_index.saturating_add(1);
            continue;
        }

        match queue.push_frame(DisplayItem::EndOfPass { seek_epoch }, controls) {
            QueuePush::Pushed => {}
            QueuePush::StaleEpoch => {
                queue.clear();
                seek_target_us = controls.current_seek_target_us();
                seek_epoch = controls.current_seek_epoch();
                loop_index = 0;
                continue;
            }
            QueuePush::Closed => return Ok(()),
        }
        println!("[{}] finished: {} frames", APP_NAME, reorder.published());

        seek_target_us = wait_for_replay_or_seek_request(controls).unwrap_or(0);
        if let Some(clock) = clock {
            clock.reset_for_replay();
        }
        queue.clear();
        seek_epoch = controls.current_seek_epoch();
        loop_index = 0;
    }
}

struct VideoSource {
    format: VideoContainerFormat,
    access_units: Vec<VideoAccessUnit>,
    estimated_total_frames: Option<u32>,
}

struct VideoAccessUnit {
    payload: VideoAccessUnitPayload,
    codec: VideoCodec,
    display_rank: usize,
    presentation_time_us: u64,
    is_keyframe: bool,
}

fn video_source_duration_us(source: &VideoSource) -> u64 {
    source
        .access_units
        .iter()
        .map(|unit| unit.presentation_time_us)
        .max()
        .unwrap_or(0)
        .saturating_add(FRAME_INTERVAL_MS * 1_000)
        .max(FRAME_INTERVAL_MS * 1_000)
}

fn video_loop_time_offset_us(
    clock: Option<&AudioClock>,
    fallback_duration_us: u64,
    loop_index: u64,
) -> u64 {
    let duration_us = clock
        .and_then(AudioClock::loop_duration_us)
        .unwrap_or(fallback_duration_us);
    loop_index.saturating_mul(duration_us)
}

struct VideoSeekPlan {
    decode_start_index: usize,
    publish_target_us: u64,
    publish_start_rank: usize,
    preview_index: Option<usize>,
}

fn video_seek_plan(source: &VideoSource, target_us: u64) -> VideoSeekPlan {
    if source.access_units.is_empty() {
        return VideoSeekPlan {
            decode_start_index: 0,
            publish_target_us: 0,
            publish_start_rank: 0,
            preview_index: None,
        };
    }
    let publish_target_us = target_us.min(video_source_duration_us(source).saturating_sub(1));
    let mut decode_start_index = 0usize;
    for (index, access_unit) in source.access_units.iter().enumerate() {
        if access_unit.presentation_time_us <= publish_target_us && access_unit.is_keyframe {
            decode_start_index = index;
        }
        if access_unit.presentation_time_us > publish_target_us {
            break;
        }
    }
    let publish_start_rank = source
        .access_units
        .iter()
        .filter(|unit| unit.presentation_time_us < publish_target_us)
        .count();
    let preview_index = source
        .access_units
        .iter()
        .enumerate()
        .find(|(_, unit)| unit.is_keyframe && unit.presentation_time_us >= publish_target_us)
        .map(|(index, _)| index)
        .or_else(|| {
            source
                .access_units
                .iter()
                .enumerate()
                .rev()
                .find(|(_, unit)| unit.is_keyframe)
                .map(|(index, _)| index)
        });
    VideoSeekPlan {
        decode_start_index,
        publish_target_us,
        publish_start_rank,
        preview_index,
    }
}

fn video_should_publish_after_seek(access_unit: &VideoAccessUnit, publish_target_us: u64) -> bool {
    access_unit.presentation_time_us >= publish_target_us
}

fn consume_seek_request(controls: &ControlsOverlay, seek_epoch: &mut u32) -> Option<u64> {
    let current_epoch = controls.current_seek_epoch();
    if current_epoch != *seek_epoch {
        thread::sleep(Duration::from_millis(SEEK_COALESCE_DELAY_MS));
        let current_epoch = controls.current_seek_epoch();
        *seek_epoch = current_epoch;
        Some(controls.current_seek_target_us())
    } else {
        None
    }
}

enum VideoAccessUnitPayload {
    Owned(Vec<u8>),
    Mp4Av1Sample {
        offset: usize,
        size: usize,
        config: Av1Config,
    },
}

impl VideoAccessUnit {
    fn bytes<'a>(
        &'a self,
        mp4_data: Option<&'a [u8]>,
        scratch: &'a mut Vec<u8>,
    ) -> Result<&'a [u8], String> {
        match &self.payload {
            VideoAccessUnitPayload::Owned(bytes) => Ok(bytes),
            VideoAccessUnitPayload::Mp4Av1Sample {
                offset,
                size,
                config,
            } => {
                let data =
                    mp4_data.ok_or_else(|| String::from("MP4 backing data is unavailable"))?;
                let end = offset
                    .checked_add(*size)
                    .ok_or_else(|| String::from("MP4 AV1 sample offset overflow"))?;
                let sample = data
                    .get(*offset..end)
                    .ok_or_else(|| String::from("MP4 AV1 sample points outside file"))?;
                av1_sample_to_scarlet_into(config, sample, scratch)?;
                Ok(scratch)
            }
        }
    }
}

enum VideoContainerFormat {
    RawH264,
    Mp4H264,
    Mp4Av1,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VideoCodec {
    H264,
    Av1,
}

impl VideoCodec {
    fn coded_format(self) -> u32 {
        match self {
            VideoCodec::H264 => VIRTIO_VIDEO_FORMAT_H264,
            VideoCodec::Av1 => VIRTIO_VIDEO_FORMAT_AV1,
        }
    }

    fn name(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Av1 => "AV1",
        }
    }
}

impl VideoSource {
    fn description(&self) -> &'static str {
        match self.format {
            VideoContainerFormat::RawH264 => "raw H.264",
            VideoContainerFormat::Mp4H264 => "MP4/H.264",
            VideoContainerFormat::Mp4Av1 => "MP4/AV1",
        }
    }
}

fn load_video_source(path: &str, mp4_data: Option<&[u8]>) -> Result<VideoSource, String> {
    if let Some(data) = mp4_data {
        return load_mp4_video_source(data, true);
    }
    let data = read_file(path)?;
    if looks_like_mp4(&data) {
        return load_mp4_video_source(&data, false);
    }
    Ok(VideoSource {
        format: VideoContainerFormat::RawH264,
        estimated_total_frames: None,
        access_units: annex_b_access_units(&data)
            .into_iter()
            .enumerate()
            .map(|(display_rank, bytes)| VideoAccessUnit {
                is_keyframe: h264_access_unit_is_keyframe(&bytes),
                payload: VideoAccessUnitPayload::Owned(bytes),
                codec: VideoCodec::H264,
                display_rank,
                presentation_time_us: display_rank as u64 * FRAME_INTERVAL_MS * 1_000,
            })
            .collect(),
    })
}

fn annex_b_access_units(data: &[u8]) -> Vec<Vec<u8>> {
    let nals = parse_raw_annex_b(data);
    let mut access_units = Vec::new();
    let mut access_unit = Vec::new();
    let mut access_unit_has_vcl = false;

    for nal in &nals {
        if nal.is_vcl() && access_unit_has_vcl && nal.starts_new_picture() {
            access_units.push(access_unit);
            access_unit = Vec::new();
            access_unit_has_vcl = false;
        }

        append_annex_b_nal(&mut access_unit, nal.bytes);
        access_unit_has_vcl |= nal.is_vcl();
    }

    if access_unit_has_vcl {
        access_units.push(access_unit);
    }
    access_units
}

#[derive(Clone, Copy)]
struct Mp4Box {
    typ: [u8; 4],
    start: usize,
    data_start: usize,
    data_end: usize,
}

#[derive(Default)]
struct Mp4Track {
    track_id: u32,
    is_video: bool,
    is_audio: bool,
    avcc: Option<AvcConfig>,
    av1: Option<Av1Config>,
    aac: Option<AacConfig>,
    media_timescale: u32,
    media_duration: u64,
    sample_sizes: Vec<u32>,
    time_to_sample: Vec<TimeToSampleEntry>,
    composition_offsets: Vec<i64>,
    sample_to_chunk: Vec<SampleToChunkEntry>,
    chunk_offsets: Vec<u64>,
    sync_samples: Vec<u32>,
}

#[derive(Clone)]
struct AvcConfig {
    nal_length_size: usize,
    parameter_sets: Vec<Vec<u8>>,
}

#[derive(Clone)]
struct Av1Config {
    config_record: Vec<u8>,
    width: u32,
    height: u32,
}

#[derive(Clone)]
struct AacConfig {
    audio_specific_config: Vec<u8>,
    sample_rate: u32,
    channels: u16,
}

#[derive(Clone, Copy)]
struct SampleToChunkEntry {
    first_chunk: u32,
    samples_per_chunk: u32,
}

#[derive(Clone, Copy)]
struct TimeToSampleEntry {
    sample_count: u32,
    sample_delta: u32,
}

#[derive(Clone, Copy)]
struct Mp4MediaSample {
    offset: u64,
    size: u32,
}

struct Mp4SampleLayout {
    samples: Vec<Mp4MediaSample>,
    display_ranks: Vec<usize>,
    presentation_times_us: Vec<u64>,
}

#[derive(Default, Clone, Copy)]
struct Mp4FragmentDefaults {
    duration: u32,
    size: u32,
    flags: u32,
}

#[derive(Clone, Copy)]
struct Mp4FragmentHeader {
    track_id: u32,
    base_data_offset: Option<u64>,
    defaults: Mp4FragmentDefaults,
}

#[derive(Clone, Copy)]
struct Mp4TrunSample {
    size: u32,
    duration: u32,
    composition_offset: i64,
}

struct Mp4Trun {
    data_offset: Option<i32>,
    samples: Vec<Mp4TrunSample>,
}

fn looks_like_mp4(data: &[u8]) -> bool {
    let mut offset = 0usize;
    while let Some(mp4_box) = read_mp4_box(data, offset, data.len()) {
        if &mp4_box.typ == b"ftyp" || &mp4_box.typ == b"moov" {
            return true;
        }
        offset = mp4_box.data_end;
    }
    false
}

fn load_mp4_video_source(data: &[u8], can_reference_mp4_data: bool) -> Result<VideoSource, String> {
    load_mp4_video_source_with_options(data, can_reference_mp4_data, false)
}

fn load_mp4_video_source_with_options(
    data: &[u8],
    can_reference_mp4_data: bool,
    allow_partial: bool,
) -> Result<VideoSource, String> {
    let mut offset = 0usize;
    let mut video_track = None;
    while let Some(mp4_box) = read_mp4_box(data, offset, data.len()) {
        if &mp4_box.typ == b"moov" {
            video_track = find_mp4_video_track(data, mp4_box.data_start, mp4_box.data_end)?;
            break;
        }
        offset = mp4_box.data_end;
    }

    let track = video_track.ok_or_else(|| String::from("MP4 has no supported video track"))?;
    let video_format = if track.avcc.is_some() {
        VideoContainerFormat::Mp4H264
    } else if track.av1.is_some() {
        VideoContainerFormat::Mp4Av1
    } else {
        return Err(String::from("MP4 has no supported video codec"));
    };
    let sample_layout = mp4_sample_layout(data, &track)?;

    let mut access_units = Vec::new();
    for (index, media_sample) in sample_layout.samples.iter().enumerate() {
        let offset = media_sample.offset as usize;
        let size = media_sample.size as usize;
        let end = offset
            .checked_add(size)
            .ok_or_else(|| String::from("MP4 sample offset overflow"))?;
        let Some(sample) = data.get(offset..end) else {
            if allow_partial {
                break;
            }
            return Err(String::from("MP4 sample points outside file"));
        };
        let (payload, codec) = match video_format {
            VideoContainerFormat::Mp4H264 => {
                let avcc = track
                    .avcc
                    .as_ref()
                    .ok_or_else(|| String::from("MP4 H.264 track has no avcC configuration"))?;
                (
                    VideoAccessUnitPayload::Owned(avc_sample_to_annex_b(avcc, sample)?),
                    VideoCodec::H264,
                )
            }
            VideoContainerFormat::Mp4Av1 => {
                let av1 = track
                    .av1
                    .as_ref()
                    .ok_or_else(|| String::from("MP4 AV1 track has no av1C configuration"))?;
                if can_reference_mp4_data {
                    (
                        VideoAccessUnitPayload::Mp4Av1Sample {
                            offset,
                            size,
                            config: av1.clone(),
                        },
                        VideoCodec::Av1,
                    )
                } else {
                    (
                        VideoAccessUnitPayload::Owned(av1_sample_to_scarlet(av1, sample)?),
                        VideoCodec::Av1,
                    )
                }
            }
            VideoContainerFormat::RawH264 => unreachable!(),
        };
        let is_keyframe = if track.sync_samples.is_empty() {
            match (&payload, codec) {
                (VideoAccessUnitPayload::Owned(bytes), VideoCodec::H264) => {
                    h264_access_unit_is_keyframe(bytes)
                }
                (_, _) => index == 0,
            }
        } else {
            track
                .sync_samples
                .binary_search(&((index + 1).min(u32::MAX as usize) as u32))
                .is_ok()
        };
        access_units.push(VideoAccessUnit {
            payload,
            codec,
            display_rank: sample_layout.display_ranks[index],
            presentation_time_us: sample_layout.presentation_times_us[index],
            is_keyframe,
        });
    }

    let estimated_total_frames = mp4_estimated_total_frames(
        track_duration_us(&track),
        &sample_layout.presentation_times_us,
        access_units.len(),
        allow_partial,
    );

    Ok(VideoSource {
        format: video_format,
        access_units,
        estimated_total_frames,
    })
}

fn track_duration_us(track: &Mp4Track) -> Option<u64> {
    if track.media_timescale == 0
        || track.media_duration == 0
        || track.media_duration == u64::MAX
        || track.media_duration == u64::from(u32::MAX)
    {
        return None;
    }
    Some((u128::from(track.media_duration) * 1_000_000 / u128::from(track.media_timescale)) as u64)
}

fn mp4_estimated_total_frames(
    duration_us: Option<u64>,
    presentation_times_us: &[u64],
    sample_count: usize,
    allow_partial: bool,
) -> Option<u32> {
    if sample_count == 0 {
        return None;
    }
    if !allow_partial {
        return Some(sample_count.min(u32::MAX as usize).max(1) as u32);
    }

    let duration_us = duration_us?;
    if presentation_times_us.len() < 2 {
        return None;
    }

    let mut min_time = u64::MAX;
    let mut max_time = 0u64;
    for time in presentation_times_us {
        min_time = min_time.min(*time);
        max_time = max_time.max(*time);
    }
    let observed_span = max_time.saturating_sub(min_time);
    if observed_span == 0 {
        return None;
    }

    let observed_intervals = presentation_times_us.len().saturating_sub(1).max(1);
    let estimated =
        (u128::from(duration_us) * observed_intervals as u128 / u128::from(observed_span)) + 1;
    let estimated = usize::try_from(estimated).unwrap_or(usize::MAX);
    Some(estimated.max(sample_count).min(u32::MAX as usize).max(1) as u32)
}

struct Mp4AacAudioSource {
    data: Arc<Vec<u8>>,
    config: AacConfig,
    samples: Vec<SampleRange>,
}

#[derive(Clone, Copy)]
struct SampleRange {
    offset: usize,
    size: usize,
}

fn load_mp4_aac_audio_source(data: Arc<Vec<u8>>) -> Result<Mp4AacAudioSource, String> {
    let mut offset = 0usize;
    let mut audio_track = None;
    while let Some(mp4_box) = read_mp4_box(data.as_slice(), offset, data.len()) {
        if &mp4_box.typ == b"moov" {
            audio_track =
                find_mp4_audio_track(data.as_slice(), mp4_box.data_start, mp4_box.data_end)?;
            break;
        }
        offset = mp4_box.data_end;
    }

    let track = audio_track.ok_or_else(|| String::from("MP4 has no AAC audio track"))?;
    let config = track
        .aac
        .clone()
        .ok_or_else(|| String::from("MP4 audio track has no AAC config"))?;
    let sample_layout = mp4_sample_layout(data.as_slice(), &track)?;

    let mut samples = Vec::new();
    for media_sample in &sample_layout.samples {
        let offset = media_sample.offset as usize;
        let size = media_sample.size as usize;
        let end = offset
            .checked_add(size)
            .ok_or_else(|| String::from("MP4 AAC sample offset overflow"))?;
        data.get(offset..end)
            .ok_or_else(|| String::from("MP4 AAC sample points outside file"))?;
        samples.push(SampleRange { offset, size });
    }

    Ok(Mp4AacAudioSource {
        data,
        config,
        samples,
    })
}

fn find_mp4_video_track(data: &[u8], start: usize, end: usize) -> Result<Option<Mp4Track>, String> {
    let mut offset = start;
    while let Some(mp4_box) = read_mp4_box(data, offset, end) {
        if &mp4_box.typ == b"trak" {
            let track = parse_mp4_track(data, mp4_box.data_start, mp4_box.data_end)?;
            if track.is_video && (track.avcc.is_some() || track.av1.is_some()) {
                return Ok(Some(track));
            }
        }
        offset = mp4_box.data_end;
    }
    Ok(None)
}

fn find_mp4_audio_track(data: &[u8], start: usize, end: usize) -> Result<Option<Mp4Track>, String> {
    let mut offset = start;
    while let Some(mp4_box) = read_mp4_box(data, offset, end) {
        if &mp4_box.typ == b"trak" {
            let track = parse_mp4_track(data, mp4_box.data_start, mp4_box.data_end)?;
            if track.is_audio && track.aac.is_some() {
                return Ok(Some(track));
            }
        }
        offset = mp4_box.data_end;
    }
    Ok(None)
}

fn parse_mp4_track(data: &[u8], start: usize, end: usize) -> Result<Mp4Track, String> {
    let mut track = Mp4Track::default();
    parse_mp4_track_boxes(data, start, end, &mut track)?;
    Ok(track)
}

fn parse_mp4_track_boxes(
    data: &[u8],
    start: usize,
    end: usize,
    track: &mut Mp4Track,
) -> Result<(), String> {
    let mut offset = start;
    while let Some(mp4_box) = read_mp4_box(data, offset, end) {
        match &mp4_box.typ {
            b"mdia" | b"minf" | b"stbl" => {
                parse_mp4_track_boxes(data, mp4_box.data_start, mp4_box.data_end, track)?;
            }
            b"tkhd" => track.track_id = parse_tkhd_track_id(data, mp4_box.data_start)?,
            b"hdlr" => parse_hdlr(data, mp4_box.data_start, mp4_box.data_end, track)?,
            b"mdhd" => {
                let mdhd = parse_mdhd(data, mp4_box.data_start)?;
                track.media_timescale = mdhd.timescale;
                track.media_duration = mdhd.duration;
            }
            b"stsd" => parse_stsd(data, mp4_box.data_start, mp4_box.data_end, track)?,
            b"stts" => {
                track.time_to_sample = parse_stts(data, mp4_box.data_start, mp4_box.data_end)?
            }
            b"ctts" => {
                track.composition_offsets = parse_ctts(data, mp4_box.data_start, mp4_box.data_end)?
            }
            b"stsz" => track.sample_sizes = parse_stsz(data, mp4_box.data_start, mp4_box.data_end)?,
            b"stsc" => {
                track.sample_to_chunk = parse_stsc(data, mp4_box.data_start, mp4_box.data_end)?
            }
            b"stss" => track.sync_samples = parse_stss(data, mp4_box.data_start, mp4_box.data_end)?,
            b"stco" => {
                track.chunk_offsets = parse_stco(data, mp4_box.data_start, mp4_box.data_end)?
            }
            b"co64" => {
                track.chunk_offsets = parse_co64(data, mp4_box.data_start, mp4_box.data_end)?
            }
            _ => {}
        }
        offset = mp4_box.data_end;
    }
    Ok(())
}

fn read_mp4_box(data: &[u8], offset: usize, limit: usize) -> Option<Mp4Box> {
    if offset.checked_add(8)? > limit || limit > data.len() {
        return None;
    }
    let size32 = read_u32_be(data.get(offset..offset + 4)?) as u64;
    let typ = [
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ];
    let (size, data_start) = if size32 == 1 {
        if offset.checked_add(16)? > limit {
            return None;
        }
        (
            read_u64_be(data.get(offset + 8..offset + 16)?) as usize,
            offset + 16,
        )
    } else if size32 == 0 {
        (limit - offset, offset + 8)
    } else {
        (size32 as usize, offset + 8)
    };
    let data_end = offset.checked_add(size)?;
    if size < data_start - offset || data_end > limit {
        return None;
    }
    Some(Mp4Box {
        typ,
        start: offset,
        data_start,
        data_end,
    })
}

fn parse_tkhd_track_id(data: &[u8], start: usize) -> Result<u32, String> {
    let version = *data
        .get(start)
        .ok_or_else(|| String::from("MP4 tkhd box is truncated"))?;
    let track_id_offset = if version == 1 {
        start
            .checked_add(20)
            .ok_or_else(|| String::from("MP4 tkhd offset overflow"))?
    } else {
        start
            .checked_add(12)
            .ok_or_else(|| String::from("MP4 tkhd offset overflow"))?
    };
    Ok(read_u32_be(
        data.get(track_id_offset..track_id_offset + 4)
            .ok_or_else(|| String::from("MP4 tkhd track id is truncated"))?,
    ))
}

fn parse_hdlr(data: &[u8], start: usize, _end: usize, track: &mut Mp4Track) -> Result<(), String> {
    let handler = data
        .get(start + 8..start + 12)
        .ok_or_else(|| String::from("MP4 hdlr box is truncated"))?;
    track.is_video = handler == b"vide";
    track.is_audio = handler == b"soun";
    Ok(())
}

struct Mp4MediaHeader {
    timescale: u32,
    duration: u64,
}

fn parse_mdhd(data: &[u8], start: usize) -> Result<Mp4MediaHeader, String> {
    let version = *data
        .get(start)
        .ok_or_else(|| String::from("MP4 mdhd box is truncated"))?;
    let (timescale_offset, duration_offset, duration_len) = if version == 1 {
        (
            start
                .checked_add(20)
                .ok_or_else(|| String::from("MP4 mdhd offset overflow"))?,
            start
                .checked_add(24)
                .ok_or_else(|| String::from("MP4 mdhd offset overflow"))?,
            8usize,
        )
    } else {
        (
            start
                .checked_add(12)
                .ok_or_else(|| String::from("MP4 mdhd offset overflow"))?,
            start
                .checked_add(16)
                .ok_or_else(|| String::from("MP4 mdhd offset overflow"))?,
            4usize,
        )
    };
    let timescale = read_u32_be(
        data.get(timescale_offset..timescale_offset + 4)
            .ok_or_else(|| String::from("MP4 mdhd timescale is truncated"))?,
    );
    let duration = if duration_len == 8 {
        read_u64_be(
            data.get(duration_offset..duration_offset + 8)
                .ok_or_else(|| String::from("MP4 mdhd duration is truncated"))?,
        )
    } else {
        u64::from(read_u32_be(
            data.get(duration_offset..duration_offset + 4)
                .ok_or_else(|| String::from("MP4 mdhd duration is truncated"))?,
        ))
    };
    Ok(Mp4MediaHeader {
        timescale,
        duration,
    })
}

fn parse_stsd(data: &[u8], start: usize, end: usize, track: &mut Mp4Track) -> Result<(), String> {
    let entry_count = read_u32_be(
        data.get(start + 4..start + 8)
            .ok_or_else(|| String::from("MP4 stsd box is truncated"))?,
    ) as usize;
    let mut offset = start + 8;
    for _ in 0..entry_count {
        let Some(entry) = read_mp4_box(data, offset, end) else {
            return Err(String::from("MP4 stsd sample entry is truncated"));
        };
        if &entry.typ == b"avc1" || &entry.typ == b"avc3" {
            parse_avc_sample_entry(data, entry.data_start, entry.data_end, track)?;
        } else if &entry.typ == b"av01" {
            parse_av1_sample_entry(data, entry.data_start, entry.data_end, track)?;
        } else if &entry.typ == b"mp4a" {
            parse_mp4a_sample_entry(data, entry.data_start, entry.data_end, track)?;
        }
        offset = entry.data_end;
    }
    Ok(())
}

fn parse_mp4a_sample_entry(
    data: &[u8],
    start: usize,
    end: usize,
    track: &mut Mp4Track,
) -> Result<(), String> {
    let entry = data
        .get(start..end)
        .ok_or_else(|| String::from("MP4 mp4a sample entry is truncated"))?;
    if entry.len() < 28 {
        return Err(String::from("MP4 mp4a sample entry is truncated"));
    }
    let fallback_channels = read_u16_be(&entry[16..18]);
    let fallback_sample_rate = read_u32_be(&entry[24..28]) >> 16;
    let mut offset = start
        .checked_add(28)
        .ok_or_else(|| String::from("MP4 mp4a sample entry overflow"))?;
    while let Some(mp4_box) = read_mp4_box(data, offset, end) {
        if &mp4_box.typ == b"esds" {
            let asc = parse_esds(data, mp4_box.data_start, mp4_box.data_end)?;
            let (sample_rate, channels, object_type) = parse_aac_audio_specific_config(&asc)
                .unwrap_or((fallback_sample_rate, fallback_channels, 2));
            if object_type != 2 {
                return Err(String::from("MP4 AAC track is not AAC-LC"));
            }
            track.aac = Some(AacConfig {
                audio_specific_config: asc,
                sample_rate,
                channels,
            });
            return Ok(());
        }
        offset = mp4_box.data_end;
    }
    Ok(())
}

fn parse_esds(data: &[u8], start: usize, end: usize) -> Result<Vec<u8>, String> {
    let esds = data
        .get(start..end)
        .ok_or_else(|| String::from("MP4 esds box is truncated"))?;
    if esds.len() < 4 {
        return Err(String::from("MP4 esds box is truncated"));
    }
    let mut cursor = 4usize;
    let tag = read_mp4_descriptor(esds, &mut cursor)?;
    if tag.tag != 0x03 {
        return Err(String::from("MP4 esds missing ES_Descriptor"));
    }
    let es_end = tag
        .payload_start
        .checked_add(tag.payload_len)
        .ok_or_else(|| String::from("MP4 esds descriptor overflow"))?;
    cursor = tag
        .payload_start
        .checked_add(3)
        .ok_or_else(|| String::from("MP4 esds descriptor overflow"))?;
    if cursor > es_end {
        return Err(String::from("MP4 esds ES_Descriptor is truncated"));
    }
    let flags = esds[cursor - 1];
    if flags & 0x80 != 0 {
        cursor = cursor
            .checked_add(2)
            .ok_or_else(|| String::from("MP4 esds dependsOn overflow"))?;
    }
    if flags & 0x40 != 0 {
        let url_len = *esds
            .get(cursor)
            .ok_or_else(|| String::from("MP4 esds URL is truncated"))?
            as usize;
        cursor = cursor
            .checked_add(1 + url_len)
            .ok_or_else(|| String::from("MP4 esds URL overflow"))?;
    }
    if flags & 0x20 != 0 {
        cursor = cursor
            .checked_add(2)
            .ok_or_else(|| String::from("MP4 esds OCR overflow"))?;
    }
    if cursor > es_end {
        return Err(String::from("MP4 esds ES_Descriptor overread"));
    }

    let decoder_config = read_mp4_descriptor(esds, &mut cursor)?;
    if decoder_config.tag != 0x04 {
        return Err(String::from("MP4 esds missing DecoderConfigDescriptor"));
    }
    let decoder_start = decoder_config.payload_start;
    let decoder_end = decoder_start
        .checked_add(decoder_config.payload_len)
        .ok_or_else(|| String::from("MP4 esds decoder config overflow"))?;
    if decoder_start.checked_add(13).unwrap_or(usize::MAX) > decoder_end {
        return Err(String::from(
            "MP4 esds DecoderConfigDescriptor is truncated",
        ));
    }
    if esds[decoder_start] != 0x40 {
        return Err(String::from("MP4 esds object type is not MPEG-4 AAC"));
    }
    cursor = decoder_start + 13;
    let decoder_specific = read_mp4_descriptor(esds, &mut cursor)?;
    if decoder_specific.tag != 0x05 {
        return Err(String::from("MP4 esds missing AudioSpecificConfig"));
    }
    let asc_end = decoder_specific
        .payload_start
        .checked_add(decoder_specific.payload_len)
        .ok_or_else(|| String::from("MP4 esds AudioSpecificConfig overflow"))?;
    Ok(esds
        .get(decoder_specific.payload_start..asc_end)
        .ok_or_else(|| String::from("MP4 esds AudioSpecificConfig is truncated"))?
        .to_vec())
}

struct Mp4Descriptor {
    tag: u8,
    payload_start: usize,
    payload_len: usize,
}

fn read_mp4_descriptor(data: &[u8], cursor: &mut usize) -> Result<Mp4Descriptor, String> {
    let tag = *data
        .get(*cursor)
        .ok_or_else(|| String::from("MP4 descriptor tag is truncated"))?;
    *cursor += 1;
    let mut len = 0usize;
    for _ in 0..4 {
        let byte = *data
            .get(*cursor)
            .ok_or_else(|| String::from("MP4 descriptor length is truncated"))?;
        *cursor += 1;
        len = (len << 7) | usize::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Ok(Mp4Descriptor {
                tag,
                payload_start: *cursor,
                payload_len: len,
            });
        }
    }
    Err(String::from("MP4 descriptor length is invalid"))
}

fn parse_aac_audio_specific_config(asc: &[u8]) -> Result<(u32, u16, u8), String> {
    let mut reader = BitReaderMsb::new(asc);
    let object_type = reader
        .read_bits(5)
        .ok_or_else(|| String::from("AAC AudioSpecificConfig object type is truncated"))?
        as u8;
    let frequency_index = reader
        .read_bits(4)
        .ok_or_else(|| String::from("AAC AudioSpecificConfig frequency is truncated"))?
        as usize;
    let sample_rate = if frequency_index == 15 {
        reader
            .read_bits(24)
            .ok_or_else(|| String::from("AAC explicit sample rate is truncated"))?
    } else {
        *AAC_SAMPLE_RATES
            .get(frequency_index)
            .ok_or_else(|| String::from("AAC sample rate index is unsupported"))?
    };
    let channel_config = reader
        .read_bits(4)
        .ok_or_else(|| String::from("AAC AudioSpecificConfig channel config is truncated"))?
        as u16;
    Ok((sample_rate, channel_config, object_type))
}

const AAC_SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

struct BitReaderMsb<'a> {
    bytes: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReaderMsb<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            bit_offset: 0,
        }
    }

    fn read_bits(&mut self, count: usize) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            let byte = *self.bytes.get(self.bit_offset / 8)?;
            let bit = (byte >> (7 - (self.bit_offset % 8))) & 1;
            self.bit_offset += 1;
            value = (value << 1) | u32::from(bit);
        }
        Some(value)
    }
}

fn parse_avc_sample_entry(
    data: &[u8],
    start: usize,
    end: usize,
    track: &mut Mp4Track,
) -> Result<(), String> {
    let mut offset = start
        .checked_add(78)
        .ok_or_else(|| String::from("MP4 avc sample entry overflow"))?;
    while let Some(mp4_box) = read_mp4_box(data, offset, end) {
        if &mp4_box.typ == b"avcC" {
            track.avcc = Some(parse_avcc(data, mp4_box.data_start, mp4_box.data_end)?);
            return Ok(());
        }
        offset = mp4_box.data_end;
    }
    Ok(())
}

fn parse_av1_sample_entry(
    data: &[u8],
    start: usize,
    end: usize,
    track: &mut Mp4Track,
) -> Result<(), String> {
    let entry = data
        .get(start..end)
        .ok_or_else(|| String::from("MP4 av01 sample entry is truncated"))?;
    if entry.len() < 78 {
        return Err(String::from("MP4 av01 sample entry is truncated"));
    }
    let width = read_u16_be(&entry[24..26]) as u32;
    let height = read_u16_be(&entry[26..28]) as u32;
    let mut offset = start
        .checked_add(78)
        .ok_or_else(|| String::from("MP4 av01 sample entry overflow"))?;
    while let Some(mp4_box) = read_mp4_box(data, offset, end) {
        if &mp4_box.typ == b"av1C" {
            let config_record = data
                .get(mp4_box.data_start..mp4_box.data_end)
                .ok_or_else(|| String::from("MP4 av1C box is truncated"))?
                .to_vec();
            if config_record.len() < 4 || config_record[0] >> 7 != 1 {
                return Err(String::from("MP4 av1C configuration is unsupported"));
            }
            track.av1 = Some(Av1Config {
                config_record,
                width,
                height,
            });
            return Ok(());
        }
        offset = mp4_box.data_end;
    }
    Ok(())
}

fn parse_avcc(data: &[u8], start: usize, end: usize) -> Result<AvcConfig, String> {
    let avcc = data
        .get(start..end)
        .ok_or_else(|| String::from("MP4 avcC box is truncated"))?;
    if avcc.len() < 7 || avcc[0] != 1 {
        return Err(String::from("MP4 avcC configuration is unsupported"));
    }
    let nal_length_size = ((avcc[4] & 0x03) + 1) as usize;
    let sps_count = (avcc[5] & 0x1f) as usize;
    let mut offset = 6usize;
    let mut parameter_sets = Vec::new();
    for _ in 0..sps_count {
        let bytes = read_avcc_parameter_set(avcc, &mut offset)?;
        parameter_sets.push(bytes);
    }
    let pps_count = *avcc
        .get(offset)
        .ok_or_else(|| String::from("MP4 avcC PPS count is missing"))? as usize;
    offset += 1;
    for _ in 0..pps_count {
        let bytes = read_avcc_parameter_set(avcc, &mut offset)?;
        parameter_sets.push(bytes);
    }
    Ok(AvcConfig {
        nal_length_size,
        parameter_sets,
    })
}

fn read_avcc_parameter_set(avcc: &[u8], offset: &mut usize) -> Result<Vec<u8>, String> {
    let length_end = (*offset)
        .checked_add(2)
        .ok_or_else(|| String::from("MP4 avcC parameter set length overflow"))?;
    let len = read_u16_be(
        avcc.get(*offset..length_end)
            .ok_or_else(|| String::from("MP4 avcC parameter set length is truncated"))?,
    ) as usize;
    *offset = length_end;
    let data_end = (*offset)
        .checked_add(len)
        .ok_or_else(|| String::from("MP4 avcC parameter set overflow"))?;
    let bytes = avcc
        .get(*offset..data_end)
        .ok_or_else(|| String::from("MP4 avcC parameter set is truncated"))?
        .to_vec();
    *offset = data_end;
    Ok(bytes)
}

fn parse_stsz(data: &[u8], start: usize, end: usize) -> Result<Vec<u32>, String> {
    let sample_size = read_u32_be(
        data.get(start + 4..start + 8)
            .ok_or_else(|| String::from("MP4 stsz box is truncated"))?,
    );
    let sample_count = read_u32_be(
        data.get(start + 8..start + 12)
            .ok_or_else(|| String::from("MP4 stsz sample count is truncated"))?,
    ) as usize;
    if sample_size != 0 {
        let mut sizes = Vec::new();
        sizes.resize(sample_count, sample_size);
        return Ok(sizes);
    }
    let mut sizes = Vec::new();
    let mut offset = start + 12;
    for _ in 0..sample_count {
        let size = read_u32_be(
            data.get(offset..offset + 4)
                .ok_or_else(|| String::from("MP4 stsz sample size table is truncated"))?,
        );
        sizes.push(size);
        offset += 4;
    }
    if offset > end {
        return Err(String::from("MP4 stsz box overread"));
    }
    Ok(sizes)
}

fn parse_stts(data: &[u8], start: usize, _end: usize) -> Result<Vec<TimeToSampleEntry>, String> {
    let entry_count = read_u32_be(
        data.get(start + 4..start + 8)
            .ok_or_else(|| String::from("MP4 stts box is truncated"))?,
    ) as usize;
    let mut entries = Vec::new();
    let mut offset = start + 8;
    for _ in 0..entry_count {
        let sample_count = read_u32_be(
            data.get(offset..offset + 4)
                .ok_or_else(|| String::from("MP4 stts sample_count is truncated"))?,
        );
        let sample_delta = read_u32_be(
            data.get(offset + 4..offset + 8)
                .ok_or_else(|| String::from("MP4 stts sample_delta is truncated"))?,
        );
        entries.push(TimeToSampleEntry {
            sample_count,
            sample_delta,
        });
        offset += 8;
    }
    Ok(entries)
}

fn parse_ctts(data: &[u8], start: usize, _end: usize) -> Result<Vec<i64>, String> {
    let version = *data
        .get(start)
        .ok_or_else(|| String::from("MP4 ctts box is truncated"))?;
    let entry_count = read_u32_be(
        data.get(start + 4..start + 8)
            .ok_or_else(|| String::from("MP4 ctts entry count is truncated"))?,
    ) as usize;
    let mut offsets = Vec::new();
    let mut offset = start + 8;
    for _ in 0..entry_count {
        let sample_count = read_u32_be(
            data.get(offset..offset + 4)
                .ok_or_else(|| String::from("MP4 ctts sample_count is truncated"))?,
        ) as usize;
        let raw_offset = read_u32_be(
            data.get(offset + 4..offset + 8)
                .ok_or_else(|| String::from("MP4 ctts sample_offset is truncated"))?,
        );
        let sample_offset = if version == 1 {
            i64::from(i32::from_be_bytes(raw_offset.to_be_bytes()))
        } else {
            i64::from(raw_offset)
        };
        for _ in 0..sample_count {
            offsets.push(sample_offset);
        }
        offset += 8;
    }
    Ok(offsets)
}

fn parse_stsc(data: &[u8], start: usize, _end: usize) -> Result<Vec<SampleToChunkEntry>, String> {
    let entry_count = read_u32_be(
        data.get(start + 4..start + 8)
            .ok_or_else(|| String::from("MP4 stsc box is truncated"))?,
    ) as usize;
    let mut entries = Vec::new();
    let mut offset = start + 8;
    for _ in 0..entry_count {
        let first_chunk = read_u32_be(
            data.get(offset..offset + 4)
                .ok_or_else(|| String::from("MP4 stsc first_chunk is truncated"))?,
        );
        let samples_per_chunk = read_u32_be(
            data.get(offset + 4..offset + 8)
                .ok_or_else(|| String::from("MP4 stsc samples_per_chunk is truncated"))?,
        );
        entries.push(SampleToChunkEntry {
            first_chunk,
            samples_per_chunk,
        });
        offset += 12;
    }
    Ok(entries)
}

fn parse_stss(data: &[u8], start: usize, _end: usize) -> Result<Vec<u32>, String> {
    let entry_count = read_u32_be(
        data.get(start + 4..start + 8)
            .ok_or_else(|| String::from("MP4 stss box is truncated"))?,
    ) as usize;
    let mut samples = Vec::new();
    let mut offset = start + 8;
    for _ in 0..entry_count {
        samples.push(read_u32_be(data.get(offset..offset + 4).ok_or_else(
            || String::from("MP4 stss sample number is truncated"),
        )?));
        offset += 4;
    }
    Ok(samples)
}

fn parse_stco(data: &[u8], start: usize, _end: usize) -> Result<Vec<u64>, String> {
    let entry_count = read_u32_be(
        data.get(start + 4..start + 8)
            .ok_or_else(|| String::from("MP4 stco box is truncated"))?,
    ) as usize;
    let mut offsets = Vec::new();
    let mut offset = start + 8;
    for _ in 0..entry_count {
        offsets.push(read_u32_be(
            data.get(offset..offset + 4)
                .ok_or_else(|| String::from("MP4 stco entry is truncated"))?,
        ) as u64);
        offset += 4;
    }
    Ok(offsets)
}

fn parse_co64(data: &[u8], start: usize, _end: usize) -> Result<Vec<u64>, String> {
    let entry_count = read_u32_be(
        data.get(start + 4..start + 8)
            .ok_or_else(|| String::from("MP4 co64 box is truncated"))?,
    ) as usize;
    let mut offsets = Vec::new();
    let mut offset = start + 8;
    for _ in 0..entry_count {
        offsets.push(read_u64_be(
            data.get(offset..offset + 8)
                .ok_or_else(|| String::from("MP4 co64 entry is truncated"))?,
        ));
        offset += 8;
    }
    Ok(offsets)
}

fn mp4_sample_layout(data: &[u8], track: &Mp4Track) -> Result<Mp4SampleLayout, String> {
    if !track.sample_sizes.is_empty() {
        let sample_offsets = mp4_sample_offsets(track)?;
        if sample_offsets.len() != track.sample_sizes.len() {
            return Err(String::from("MP4 sample table is inconsistent"));
        }
        let samples = sample_offsets
            .iter()
            .enumerate()
            .map(|(index, offset)| Mp4MediaSample {
                offset: *offset,
                size: track.sample_sizes[index],
            })
            .collect();
        let (display_ranks, presentation_times_us) = mp4_display_timing(track)?;
        return Ok(Mp4SampleLayout {
            samples,
            display_ranks,
            presentation_times_us,
        });
    }

    mp4_fragment_sample_layout(data, track)
}

fn mp4_fragment_sample_layout(data: &[u8], track: &Mp4Track) -> Result<Mp4SampleLayout, String> {
    let fragment_defaults = mp4_fragment_defaults(data, track.track_id)?;
    let mut samples = Vec::new();
    let mut presentation_order = Vec::new();
    let mut offset = 0usize;

    while let Some(mp4_box) = read_mp4_box(data, offset, data.len()) {
        if &mp4_box.typ == b"moof" {
            parse_moof_samples(
                data,
                &mp4_box,
                track,
                fragment_defaults,
                &mut samples,
                &mut presentation_order,
            )?;
        }
        offset = mp4_box.data_end;
    }

    if samples.is_empty() {
        return Err(String::from("MP4 fragmented sample table is missing"));
    }

    presentation_order.sort_by_key(|(presentation_time, index)| (*presentation_time, *index));
    let mut display_ranks = Vec::new();
    display_ranks.resize(samples.len(), 0usize);
    for (rank, (_, sample_index)) in presentation_order.iter().enumerate() {
        display_ranks[*sample_index] = rank;
    }

    let first_presentation_time = presentation_order
        .first()
        .map(|(presentation_time, _)| *presentation_time)
        .unwrap_or(0);
    let timescale = u64::from(track.media_timescale).max(1);
    let mut presentation_times_us = Vec::new();
    presentation_times_us.resize(samples.len(), 0u64);
    for (presentation_time, sample_index) in &presentation_order {
        let relative_time = presentation_time.saturating_sub(first_presentation_time);
        presentation_times_us[*sample_index] =
            (relative_time as u128 * 1_000_000 / u128::from(timescale)) as u64;
    }

    Ok(Mp4SampleLayout {
        samples,
        display_ranks,
        presentation_times_us,
    })
}

fn parse_moof_samples(
    data: &[u8],
    moof: &Mp4Box,
    track: &Mp4Track,
    fragment_defaults: Mp4FragmentDefaults,
    samples: &mut Vec<Mp4MediaSample>,
    presentation_order: &mut Vec<(i128, usize)>,
) -> Result<(), String> {
    let mut offset = moof.data_start;
    while let Some(mp4_box) = read_mp4_box(data, offset, moof.data_end) {
        if &mp4_box.typ == b"traf" {
            parse_traf_samples(
                data,
                &mp4_box,
                moof.start,
                track,
                fragment_defaults,
                samples,
                presentation_order,
            )?;
        }
        offset = mp4_box.data_end;
    }
    Ok(())
}

fn parse_traf_samples(
    data: &[u8],
    traf: &Mp4Box,
    moof_start: usize,
    track: &Mp4Track,
    fragment_defaults: Mp4FragmentDefaults,
    samples: &mut Vec<Mp4MediaSample>,
    presentation_order: &mut Vec<(i128, usize)>,
) -> Result<(), String> {
    let mut header = None;
    let mut base_decode_time = 0u64;
    let mut truns = Vec::new();
    let mut offset = traf.data_start;

    while let Some(mp4_box) = read_mp4_box(data, offset, traf.data_end) {
        match &mp4_box.typ {
            b"tfhd" => {
                header = Some(parse_tfhd(
                    data,
                    mp4_box.data_start,
                    mp4_box.data_end,
                    fragment_defaults,
                )?)
            }
            b"tfdt" => {
                base_decode_time = parse_tfdt(data, mp4_box.data_start, mp4_box.data_end)?;
            }
            b"trun" => {
                truns.push(parse_trun(
                    data,
                    mp4_box.data_start,
                    mp4_box.data_end,
                    fragment_defaults,
                    header.as_ref().map(|header| header.defaults),
                )?);
            }
            _ => {}
        }
        offset = mp4_box.data_end;
    }

    let Some(header) = header else {
        return Ok(());
    };
    if track.track_id != 0 && header.track_id != track.track_id {
        return Ok(());
    }

    let base_data_offset = header.base_data_offset.unwrap_or(moof_start as u64);
    let mut current_data_offset = base_data_offset;
    let mut decode_time = base_decode_time;
    for trun in truns {
        let mut sample_offset = if let Some(data_offset) = trun.data_offset {
            add_signed_u64(base_data_offset, data_offset)?
        } else {
            current_data_offset
        };
        for trun_sample in trun.samples {
            let sample_index = samples.len();
            samples.push(Mp4MediaSample {
                offset: sample_offset,
                size: trun_sample.size,
            });
            presentation_order.push((
                i128::from(decode_time) + i128::from(trun_sample.composition_offset),
                sample_index,
            ));
            sample_offset = sample_offset
                .checked_add(u64::from(trun_sample.size))
                .ok_or_else(|| String::from("MP4 fragment sample offset overflow"))?;
            decode_time = decode_time
                .checked_add(u64::from(trun_sample.duration))
                .ok_or_else(|| String::from("MP4 fragment decode timestamp overflow"))?;
        }
        current_data_offset = sample_offset;
    }

    Ok(())
}

fn mp4_fragment_defaults(data: &[u8], track_id: u32) -> Result<Mp4FragmentDefaults, String> {
    let mut offset = 0usize;
    while let Some(mp4_box) = read_mp4_box(data, offset, data.len()) {
        if &mp4_box.typ == b"moov" {
            return parse_moov_fragment_defaults(
                data,
                mp4_box.data_start,
                mp4_box.data_end,
                track_id,
            );
        }
        offset = mp4_box.data_end;
    }
    Ok(Mp4FragmentDefaults::default())
}

fn parse_moov_fragment_defaults(
    data: &[u8],
    start: usize,
    end: usize,
    track_id: u32,
) -> Result<Mp4FragmentDefaults, String> {
    let mut offset = start;
    while let Some(mp4_box) = read_mp4_box(data, offset, end) {
        if &mp4_box.typ == b"mvex" {
            return parse_mvex_fragment_defaults(
                data,
                mp4_box.data_start,
                mp4_box.data_end,
                track_id,
            );
        }
        offset = mp4_box.data_end;
    }
    Ok(Mp4FragmentDefaults::default())
}

fn parse_mvex_fragment_defaults(
    data: &[u8],
    start: usize,
    end: usize,
    track_id: u32,
) -> Result<Mp4FragmentDefaults, String> {
    let mut offset = start;
    let mut first_defaults = None;
    while let Some(mp4_box) = read_mp4_box(data, offset, end) {
        if &mp4_box.typ == b"trex" {
            let (trex_track_id, defaults) = parse_trex(data, mp4_box.data_start, mp4_box.data_end)?;
            if first_defaults.is_none() {
                first_defaults = Some(defaults);
            }
            if track_id == 0 || trex_track_id == track_id {
                return Ok(defaults);
            }
        }
        offset = mp4_box.data_end;
    }
    Ok(first_defaults.unwrap_or_default())
}

fn parse_trex(
    data: &[u8],
    start: usize,
    _end: usize,
) -> Result<(u32, Mp4FragmentDefaults), String> {
    let track_id = read_u32_be(
        data.get(start + 4..start + 8)
            .ok_or_else(|| String::from("MP4 trex track id is truncated"))?,
    );
    let duration = read_u32_be(
        data.get(start + 12..start + 16)
            .ok_or_else(|| String::from("MP4 trex duration is truncated"))?,
    );
    let size = read_u32_be(
        data.get(start + 16..start + 20)
            .ok_or_else(|| String::from("MP4 trex size is truncated"))?,
    );
    let flags = read_u32_be(
        data.get(start + 20..start + 24)
            .ok_or_else(|| String::from("MP4 trex flags are truncated"))?,
    );
    Ok((
        track_id,
        Mp4FragmentDefaults {
            duration,
            size,
            flags,
        },
    ))
}

fn parse_tfhd(
    data: &[u8],
    start: usize,
    end: usize,
    fragment_defaults: Mp4FragmentDefaults,
) -> Result<Mp4FragmentHeader, String> {
    let flags = read_full_box_flags(data, start, end, "tfhd")?;
    let track_id = read_u32_be(
        data.get(start + 4..start + 8)
            .ok_or_else(|| String::from("MP4 tfhd track id is truncated"))?,
    );
    let mut offset = start + 8;
    let base_data_offset = if flags & 0x000001 != 0 {
        let value = read_u64_be(
            data.get(offset..offset + 8)
                .ok_or_else(|| String::from("MP4 tfhd base-data-offset is truncated"))?,
        );
        offset += 8;
        Some(value)
    } else {
        None
    };
    if flags & 0x000002 != 0 {
        offset = offset
            .checked_add(4)
            .ok_or_else(|| String::from("MP4 tfhd offset overflow"))?;
    }
    let mut defaults = fragment_defaults;
    if flags & 0x000008 != 0 {
        defaults.duration = read_u32_be(
            data.get(offset..offset + 4)
                .ok_or_else(|| String::from("MP4 tfhd default duration is truncated"))?,
        );
        offset += 4;
    }
    if flags & 0x000010 != 0 {
        defaults.size = read_u32_be(
            data.get(offset..offset + 4)
                .ok_or_else(|| String::from("MP4 tfhd default size is truncated"))?,
        );
        offset += 4;
    }
    if flags & 0x000020 != 0 {
        defaults.flags = read_u32_be(
            data.get(offset..offset + 4)
                .ok_or_else(|| String::from("MP4 tfhd default flags are truncated"))?,
        );
    }
    Ok(Mp4FragmentHeader {
        track_id,
        base_data_offset,
        defaults,
    })
}

fn parse_tfdt(data: &[u8], start: usize, end: usize) -> Result<u64, String> {
    let version = *data
        .get(start)
        .ok_or_else(|| String::from("MP4 tfdt box is truncated"))?;
    read_full_box_flags(data, start, end, "tfdt")?;
    if version == 1 {
        Ok(read_u64_be(data.get(start + 4..start + 12).ok_or_else(
            || String::from("MP4 tfdt decode time is truncated"),
        )?))
    } else {
        Ok(u64::from(read_u32_be(
            data.get(start + 4..start + 8)
                .ok_or_else(|| String::from("MP4 tfdt decode time is truncated"))?,
        )))
    }
}

fn parse_trun(
    data: &[u8],
    start: usize,
    end: usize,
    fragment_defaults: Mp4FragmentDefaults,
    track_defaults: Option<Mp4FragmentDefaults>,
) -> Result<Mp4Trun, String> {
    let version = *data
        .get(start)
        .ok_or_else(|| String::from("MP4 trun box is truncated"))?;
    let flags = read_full_box_flags(data, start, end, "trun")?;
    let sample_count = read_u32_be(
        data.get(start + 4..start + 8)
            .ok_or_else(|| String::from("MP4 trun sample count is truncated"))?,
    ) as usize;
    let defaults = track_defaults.unwrap_or(fragment_defaults);
    let mut offset = start + 8;
    let data_offset = if flags & 0x000001 != 0 {
        let value = read_i32_be(
            data.get(offset..offset + 4)
                .ok_or_else(|| String::from("MP4 trun data offset is truncated"))?,
        );
        offset += 4;
        Some(value)
    } else {
        None
    };
    if flags & 0x000004 != 0 {
        offset = offset
            .checked_add(4)
            .ok_or_else(|| String::from("MP4 trun offset overflow"))?;
    }

    let mut samples = Vec::new();
    for _ in 0..sample_count {
        let duration = if flags & 0x000100 != 0 {
            let value = read_u32_be(
                data.get(offset..offset + 4)
                    .ok_or_else(|| String::from("MP4 trun sample duration is truncated"))?,
            );
            offset += 4;
            value
        } else {
            defaults.duration
        };
        let size = if flags & 0x000200 != 0 {
            let value = read_u32_be(
                data.get(offset..offset + 4)
                    .ok_or_else(|| String::from("MP4 trun sample size is truncated"))?,
            );
            offset += 4;
            value
        } else {
            defaults.size
        };
        if flags & 0x000400 != 0 {
            offset = offset
                .checked_add(4)
                .ok_or_else(|| String::from("MP4 trun offset overflow"))?;
        }
        let composition_offset = if flags & 0x000800 != 0 {
            let raw = data
                .get(offset..offset + 4)
                .ok_or_else(|| String::from("MP4 trun composition offset is truncated"))?;
            offset += 4;
            if version == 1 {
                i64::from(read_i32_be(raw))
            } else {
                i64::from(read_u32_be(raw))
            }
        } else {
            0
        };
        if size == 0 {
            return Err(String::from("MP4 trun sample size is missing"));
        }
        samples.push(Mp4TrunSample {
            size,
            duration,
            composition_offset,
        });
    }
    Ok(Mp4Trun {
        data_offset,
        samples,
    })
}

fn read_full_box_flags(data: &[u8], start: usize, end: usize, name: &str) -> Result<u32, String> {
    let bytes = data
        .get(start..start + 4)
        .ok_or_else(|| format!("MP4 {} full box header is truncated", name))?;
    if start + 4 > end {
        return Err(format!("MP4 {} full box overread", name));
    }
    Ok((u32::from(bytes[1]) << 16) | (u32::from(bytes[2]) << 8) | u32::from(bytes[3]))
}

fn add_signed_u64(base: u64, offset: i32) -> Result<u64, String> {
    if offset >= 0 {
        base.checked_add(offset as u64)
            .ok_or_else(|| String::from("MP4 fragment data offset overflow"))
    } else {
        base.checked_sub(u64::from(offset.unsigned_abs()))
            .ok_or_else(|| String::from("MP4 fragment data offset underflow"))
    }
}

fn mp4_sample_offsets(track: &Mp4Track) -> Result<Vec<u64>, String> {
    if track.sample_to_chunk.is_empty() || track.chunk_offsets.is_empty() {
        return Err(String::from("MP4 sample chunk table is missing"));
    }
    let mut sample_offsets = Vec::new();
    let mut sample_index = 0usize;
    let mut stsc_index = 0usize;

    for (chunk_index, chunk_offset) in track.chunk_offsets.iter().enumerate() {
        let chunk_number = chunk_index as u32 + 1;
        while stsc_index + 1 < track.sample_to_chunk.len()
            && track.sample_to_chunk[stsc_index + 1].first_chunk <= chunk_number
        {
            stsc_index += 1;
        }
        let samples_per_chunk = track.sample_to_chunk[stsc_index].samples_per_chunk as usize;
        let mut offset = *chunk_offset;
        for _ in 0..samples_per_chunk {
            if sample_index >= track.sample_sizes.len() {
                return Ok(sample_offsets);
            }
            sample_offsets.push(offset);
            offset = offset
                .checked_add(track.sample_sizes[sample_index] as u64)
                .ok_or_else(|| String::from("MP4 sample offset overflow"))?;
            sample_index += 1;
        }
    }
    Ok(sample_offsets)
}

fn mp4_display_timing(track: &Mp4Track) -> Result<(Vec<usize>, Vec<u64>), String> {
    let sample_count = track.sample_sizes.len();
    if sample_count == 0 {
        return Ok((Vec::new(), Vec::new()));
    }
    if track.composition_offsets.len() > sample_count {
        return Err(String::from("MP4 composition offset table is too large"));
    }

    let mut decode_times = Vec::new();
    decode_times.resize(sample_count, 0u64);
    let mut sample_index = 0usize;
    let mut decode_time = 0u64;
    for entry in &track.time_to_sample {
        for _ in 0..entry.sample_count {
            if sample_index >= sample_count {
                break;
            }
            decode_times[sample_index] = decode_time;
            decode_time = decode_time
                .checked_add(u64::from(entry.sample_delta))
                .ok_or_else(|| String::from("MP4 decode timestamp overflow"))?;
            sample_index += 1;
        }
    }
    while sample_index < sample_count {
        decode_times[sample_index] = sample_index as u64;
        sample_index += 1;
    }

    let mut presentation_order = Vec::new();
    for (index, decode_time) in decode_times.iter().enumerate() {
        let composition_offset = track.composition_offsets.get(index).copied().unwrap_or(0);
        presentation_order.push((
            i128::from(*decode_time) + i128::from(composition_offset),
            index,
        ));
    }
    presentation_order.sort_by_key(|(presentation_time, index)| (*presentation_time, *index));

    let mut ranks = Vec::new();
    ranks.resize(sample_count, 0usize);
    for (rank, (_, sample_index)) in presentation_order.iter().enumerate() {
        ranks[*sample_index] = rank;
    }

    let first_presentation_time = presentation_order
        .first()
        .map(|(presentation_time, _)| *presentation_time)
        .unwrap_or(0);
    let timescale = u64::from(track.media_timescale).max(1);
    let mut presentation_times_us = Vec::new();
    presentation_times_us.resize(sample_count, 0u64);
    for (presentation_time, sample_index) in &presentation_order {
        let relative_time = presentation_time.saturating_sub(first_presentation_time);
        presentation_times_us[*sample_index] =
            (relative_time as u128 * 1_000_000 / u128::from(timescale)) as u64;
    }
    Ok((ranks, presentation_times_us))
}

fn avc_sample_to_annex_b(config: &AvcConfig, sample: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for parameter_set in &config.parameter_sets {
        append_annex_b_nal(&mut out, parameter_set);
    }

    let mut offset = 0usize;
    while offset < sample.len() {
        let nal_len = read_nal_length(sample, offset, config.nal_length_size)?;
        offset += config.nal_length_size;
        let end = offset
            .checked_add(nal_len)
            .ok_or_else(|| String::from("MP4 AVC NAL length overflow"))?;
        let nal = sample
            .get(offset..end)
            .ok_or_else(|| String::from("MP4 AVC sample NAL is truncated"))?;
        append_annex_b_nal(&mut out, nal);
        offset = end;
    }
    Ok(out)
}

fn av1_sample_to_scarlet(config: &Av1Config, sample: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    av1_sample_to_scarlet_into(config, sample, &mut out)?;
    Ok(out)
}

fn av1_sample_to_scarlet_into(
    config: &Av1Config,
    sample: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let config_len = u32::try_from(config.config_record.len())
        .map_err(|_| String::from("MP4 av1C configuration is too large"))?;
    let sample_len =
        u32::try_from(sample.len()).map_err(|_| String::from("MP4 AV1 sample is too large"))?;
    out.clear();
    let total_len = SCARLET_AV1_ACCESS_UNIT_MAGIC
        .len()
        .checked_add(16)
        .and_then(|len| len.checked_add(config.config_record.len()))
        .and_then(|len| len.checked_add(sample.len()))
        .ok_or_else(|| String::from("MP4 AV1 access unit length overflow"))?;
    out.reserve(total_len.saturating_sub(out.capacity()));
    out.extend_from_slice(SCARLET_AV1_ACCESS_UNIT_MAGIC);
    out.extend_from_slice(&config.width.to_le_bytes());
    out.extend_from_slice(&config.height.to_le_bytes());
    out.extend_from_slice(&config_len.to_le_bytes());
    out.extend_from_slice(&sample_len.to_le_bytes());
    out.extend_from_slice(&config.config_record);
    out.extend_from_slice(sample);
    Ok(())
}

fn read_nal_length(sample: &[u8], offset: usize, nal_length_size: usize) -> Result<usize, String> {
    let end = offset
        .checked_add(nal_length_size)
        .ok_or_else(|| String::from("MP4 AVC NAL length overflow"))?;
    let bytes = sample
        .get(offset..end)
        .ok_or_else(|| String::from("MP4 AVC NAL length is truncated"))?;
    let mut value = 0usize;
    for byte in bytes {
        value = (value << 8) | usize::from(*byte);
    }
    Ok(value)
}

struct RawNalUnit<'a> {
    nal_type: u8,
    nal_ref_idc: u8,
    offset: usize,
    bytes: &'a [u8],
}

impl<'a> RawNalUnit<'a> {
    fn is_vcl(&self) -> bool {
        (1..=5).contains(&self.nal_type)
    }

    fn starts_new_picture(&self) -> bool {
        if !self.is_vcl() {
            return false;
        }
        first_mb_in_slice(self.bytes) == Some(0)
    }
}

fn h264_access_unit_is_keyframe(access_unit: &[u8]) -> bool {
    parse_raw_annex_b(access_unit)
        .iter()
        .any(|nal| nal.nal_type == 5)
}

struct ScarletVideoFrame {
    width: u32,
    height: u32,
    pixel_format: u32,
    payload: ScarletVideoPayload,
}

enum ScarletVideoPayload {
    Owned(Vec<u8>),
    Mapped { ptr: *const u8, len: usize },
}

impl ScarletVideoFrame {
    fn payload(&self) -> &[u8] {
        match &self.payload {
            ScarletVideoPayload::Owned(payload) => payload,
            ScarletVideoPayload::Mapped { ptr, len } => {
                // SAFETY: mapped video frames point into the live /dev/video0
                // mmap owned by HardwareVideoDecoder. Frames are consumed before
                // the next decode submission overwrites the output buffer.
                unsafe { core::slice::from_raw_parts(*ptr, *len) }
            }
        }
    }

    fn into_owned(mut self) -> Self {
        if matches!(&self.payload, ScarletVideoPayload::Mapped { .. }) {
            self.payload = ScarletVideoPayload::Owned(self.payload().to_vec());
        }
        self
    }
}

#[derive(Clone, Copy)]
struct MappedVideoBuffer {
    stream_id: u32,
    session_commands: bool,
    ptr: *mut u8,
    mmap_len: usize,
    input_offset: usize,
    input_len: usize,
    output_offset: usize,
    output_len: usize,
}

impl MappedVideoBuffer {
    fn payload_ptr(&self, payload_offset: u64, payload_len: usize) -> Result<*const u8, String> {
        let payload_offset = payload_offset as usize;
        let end = payload_offset
            .checked_add(payload_len)
            .ok_or_else(|| String::from("hardware decoder mmap payload length overflow"))?;
        let output_start = self.output_offset;
        let output_end = self
            .output_offset
            .checked_add(self.output_len)
            .ok_or_else(|| String::from("hardware decoder mmap output length overflow"))?;
        if payload_offset < output_start || end > output_end || end > self.mmap_len {
            return Err(String::from(
                "hardware decoder returned invalid mmap payload range",
            ));
        }
        // SAFETY: the range was validated to lie within the live mmap.
        Ok(unsafe { self.ptr.add(payload_offset) as *const u8 })
    }
}

struct HardwareVideoDecoder {
    device: File,
    mapped: Option<MappedVideoBuffer>,
    caps: Option<ScarletVideoCapabilities>,
    h264_context: H264RequestContext,
}

impl HardwareVideoDecoder {
    fn open() -> Result<Self, String> {
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(VIDEO_DEVICE_PATH)
            .map_err(|_| format!("failed to open {}", VIDEO_DEVICE_PATH))?;
        let caps = Self::query_capabilities(&device);
        if let Some(caps) = caps {
            println!(
                "[{}] hardware decoder caps flags=0x{:x} stateful_h264={} stateful_av1={} stateless_h264={}",
                APP_NAME,
                caps.flags,
                caps.has_flag(SCARLET_VIDEO_CAP_STATEFUL_H264),
                caps.has_flag(SCARLET_VIDEO_CAP_STATEFUL_AV1),
                caps.has_flag(SCARLET_VIDEO_CAP_STATELESS_H264)
            );
        }
        let mapped = Self::map_video_buffer(&device, caps);
        if let Some(buffer) = &mapped {
            println!(
                "[{}] hardware decoder mmap input={} output={}",
                APP_NAME, buffer.input_len, buffer.output_len
            );
        }
        Ok(Self {
            device,
            mapped,
            caps,
            h264_context: H264RequestContext::default(),
        })
    }

    fn decode_access_unit(
        &mut self,
        codec: VideoCodec,
        access_unit: &[u8],
    ) -> Result<Option<ScarletVideoFrame>, String> {
        if access_unit.is_empty() {
            return Ok(None);
        }
        if !self.supports_decode_codec(codec) {
            return Err(format!(
                "hardware decoder does not support {}",
                codec.name()
            ));
        }
        if let Some(buffer) = &self.mapped {
            if access_unit.len() <= buffer.input_len {
                return self.decode_access_unit_mapped(codec, access_unit);
            }
        }
        if !self.supports_stateful_codec(codec) {
            return Err(format!(
                "hardware decoder stateless {} requires mmap input",
                codec.name()
            ));
        }
        if codec != VideoCodec::H264 {
            return Err(String::from(
                "hardware decoder mmap input overflow for non-H.264 access unit",
            ));
        }
        self.decode_access_unit_stream(access_unit)
    }

    fn supports_decode_codec(&self, codec: VideoCodec) -> bool {
        self.supports_stateful_codec(codec)
            || (codec == VideoCodec::H264 && self.supports_stateless_h264())
    }

    fn supports_stateful_codec(&self, codec: VideoCodec) -> bool {
        let Some(caps) = self.caps else {
            return true;
        };
        match codec {
            VideoCodec::H264 => caps.has_flag(SCARLET_VIDEO_CAP_STATEFUL_H264),
            VideoCodec::Av1 => caps.has_flag(SCARLET_VIDEO_CAP_STATEFUL_AV1),
        }
    }

    fn supports_stateless_h264(&self) -> bool {
        self.caps
            .map(|caps| caps.has_flag(SCARLET_VIDEO_CAP_STATELESS_H264))
            .unwrap_or(false)
    }

    fn decode_access_unit_stream(
        &mut self,
        access_unit: &[u8],
    ) -> Result<Option<ScarletVideoFrame>, String> {
        let written = match self.device.write(access_unit) {
            Ok(written) => written,
            Err(err) => {
                let status = self.read_decoder_status();
                return Err(format!("hardware decoder write failed: {err}{status}"));
            }
        };
        if written != access_unit.len() {
            return Err(format!(
                "hardware decoder accepted only {} of {} bytes",
                written,
                access_unit.len()
            ));
        }

        let mut header = [0u8; SCARLET_VIDEO_FRAME_HEADER_LEN];
        read_exact_file(&mut self.device, &mut header)?;
        if &header[0..4] != b"SVF1" {
            let text = core::str::from_utf8(&header).unwrap_or("");
            return Err(format!(
                "hardware decoder returned invalid frame magic {:02x} {:02x} {:02x} {:02x} {}",
                header[0], header[1], header[2], header[3], text
            ));
        }

        let width = read_u32_le(&header[4..8]);
        let height = read_u32_le(&header[8..12]);
        let pixel_format = read_u32_le(&header[12..16]);
        let payload_len = read_u32_le(&header[16..20]) as usize;
        if width == 0 || height == 0 {
            return Err(String::from("hardware decoder returned empty frame"));
        }

        let mut payload = Vec::new();
        payload.resize(payload_len, 0);
        read_exact_file(&mut self.device, &mut payload)?;
        Ok(Some(ScarletVideoFrame {
            width,
            height,
            pixel_format,
            payload: ScarletVideoPayload::Owned(payload),
        }))
    }

    fn decode_access_unit_mapped(
        &mut self,
        codec: VideoCodec,
        access_unit: &[u8],
    ) -> Result<Option<ScarletVideoFrame>, String> {
        let Some(buffer) = self.mapped else {
            return Ok(None);
        };
        let input_ptr = buffer.ptr;
        let input_offset = buffer.input_offset;
        let input_len = buffer.input_len;
        if access_unit.len() > input_len {
            return Err(String::from("hardware decoder mmap input overflow"));
        }

        // SAFETY: the mapped input buffer is writable for `input_len` bytes,
        // and `access_unit.len()` was validated to fit. The source slice does
        // not overlap the device mapping.
        unsafe {
            core::ptr::copy_nonoverlapping(
                access_unit.as_ptr(),
                input_ptr.add(input_offset),
                access_unit.len(),
            );
        }

        if codec == VideoCodec::H264 && self.supports_stateless_h264() {
            let h264 = self.h264_context.params_for_access_unit(access_unit)?;
            let params = &h264.params;
            let submit = ScarletVideoH264StatelessSubmit {
                stream_id: buffer.stream_id,
                input_len: access_unit.len() as u32,
                timestamp: h264.timestamp,
                params: ScarletVideoH264ParamPtrs {
                    sps: &params.sps as *const _ as usize as u64,
                    pps: &params.pps as *const _ as usize as u64,
                    scaling_matrix: &params.scaling_matrix as *const _ as usize as u64,
                    pred_weights: &params.pred_weights as *const _ as usize as u64,
                    slice_params: &params.slice_params as *const _ as usize as u64,
                    decode_params: &params.decode_params as *const _ as usize as u64,
                },
                flags: 0,
                padding: 0,
            };
            self.device
                .as_handle()
                .control(
                    SCARLET_VIDEO_SUBMIT_H264_STATELESS,
                    &submit as *const _ as usize,
                )
                .map_err(|_| {
                    let status = self.read_decoder_status();
                    format!("hardware decoder stateless H.264 submit failed{status}")
                })?;
        } else if buffer.session_commands {
            let submit = ScarletVideoSessionSubmit {
                stream_id: buffer.stream_id,
                input_len: access_unit.len() as u32,
                coded_format: codec.coded_format(),
                padding: 0,
                timestamp: 0,
            };
            self.device
                .as_handle()
                .control(SCARLET_VIDEO_SUBMIT_SESSION, &submit as *const _ as usize)
                .map_err(|_| {
                    let status = self.read_decoder_status();
                    format!("hardware decoder mmap submit failed{status}")
                })?;
        } else {
            let submit = ScarletVideoSubmit {
                input_len: access_unit.len() as u32,
                coded_format: codec.coded_format(),
                timestamp: 0,
            };
            self.device
                .as_handle()
                .control(SCARLET_VIDEO_SUBMIT, &submit as *const _ as usize)
                .map_err(|_| {
                    let status = self.read_decoder_status();
                    format!("hardware decoder mmap submit failed{status}")
                })?;
        }

        let mut empty_polls = 0usize;
        loop {
            let dequeue_result = if buffer.session_commands {
                let mut session_frame = ScarletVideoSessionDequeuedFrame {
                    stream_id: buffer.stream_id,
                    ..Default::default()
                };
                let result = self.device.as_handle().control(
                    SCARLET_VIDEO_DEQUEUE_SESSION,
                    &mut session_frame as *mut _ as usize,
                );
                result.map(|value| (value, session_frame.frame))
            } else {
                let mut frame = ScarletVideoDequeuedFrame::default();
                let result = self
                    .device
                    .as_handle()
                    .control(SCARLET_VIDEO_DEQUEUE, &mut frame as *mut _ as usize);
                result.map(|value| (value, frame))
            };
            match dequeue_result {
                Ok((1, frame)) => {
                    if frame.width == 0 || frame.height == 0 || frame.payload_len == 0 {
                        return Err(String::from("hardware decoder returned empty mmap frame"));
                    }
                    let Some(buffer) = self.mapped else {
                        return Err(String::from("hardware decoder mmap buffer disappeared"));
                    };
                    let payload_ptr =
                        buffer.payload_ptr(frame.payload_offset, frame.payload_len as usize)?;
                    return Ok(Some(ScarletVideoFrame {
                        width: frame.width,
                        height: frame.height,
                        pixel_format: frame.pixel_format,
                        payload: ScarletVideoPayload::Mapped {
                            ptr: payload_ptr,
                            len: frame.payload_len as usize,
                        },
                    }));
                }
                Ok((0, _)) => {
                    empty_polls += 1;
                    if empty_polls > 10_000 {
                        return Err(String::from(
                            "hardware decoder timed out before mmap frame was complete",
                        ));
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Ok((_, _)) => {
                    return Err(String::from(
                        "hardware decoder returned invalid dequeue result",
                    ));
                }
                Err(_) => {
                    let status = self.read_decoder_status();
                    return Err(format!("hardware decoder mmap dequeue failed{status}"));
                }
            }
        }
    }

    fn map_video_buffer(
        device: &File,
        caps: Option<ScarletVideoCapabilities>,
    ) -> Option<MappedVideoBuffer> {
        if caps
            .map(|caps| caps.has_flag(SCARLET_VIDEO_CAP_MAPPED_BUFFERS))
            .is_some_and(|has_mapped_buffers| !has_mapped_buffers)
        {
            return None;
        }
        let use_session_commands = caps
            .map(|caps| caps.has_flag(SCARLET_VIDEO_CAP_SESSIONS))
            .unwrap_or(true);
        let (stream_id, session_commands, info) = if use_session_commands {
            let mut session_info = ScarletVideoSessionInfo::default();
            if device
                .as_handle()
                .control(
                    SCARLET_VIDEO_CREATE_SESSION,
                    &mut session_info as *mut _ as usize,
                )
                .is_ok()
            {
                (session_info.stream_id, true, session_info.buffer)
            } else {
                let mut info = ScarletVideoBufferInfo::default();
                device
                    .as_handle()
                    .control(SCARLET_VIDEO_GET_BUFFER, &mut info as *mut _ as usize)
                    .ok()?;
                (1, false, info)
            }
        } else {
            let mut info = ScarletVideoBufferInfo::default();
            device
                .as_handle()
                .control(SCARLET_VIDEO_GET_BUFFER, &mut info as *mut _ as usize)
                .ok()?;
            (1, false, info)
        };
        let mapper = device.as_handle().as_memory_mapping().ok()?;
        let addr = mapper
            .mmap(
                0,
                info.mmap_len as usize,
                prot::READ | prot::WRITE,
                mmap_flags::SHARED,
                info.mmap_offset as usize,
            )
            .ok()?;
        Some(MappedVideoBuffer {
            stream_id,
            session_commands,
            ptr: addr as *mut u8,
            mmap_len: info.mmap_len as usize,
            input_offset: info.input_offset as usize,
            input_len: info.input_len as usize,
            output_offset: info.output_offset as usize,
            output_len: info.output_len as usize,
        })
    }

    fn query_capabilities(device: &File) -> Option<ScarletVideoCapabilities> {
        let mut caps = ScarletVideoCapabilities::default();
        device
            .as_handle()
            .control(SCARLET_VIDEO_GET_CAPS, &mut caps as *mut _ as usize)
            .ok()?;
        if caps.version == SCARLET_VIDEO_CAPS_VERSION {
            Some(caps)
        } else {
            None
        }
    }

    fn read_decoder_status(&mut self) -> String {
        let mut buffer = [0u8; 512];
        match self.device.read(&mut buffer) {
            Ok(0) | Err(_) => String::new(),
            Ok(read) => {
                let status = core::str::from_utf8(&buffer[..read]).unwrap_or("<non-utf8 status>");
                format!("; {status}")
            }
        }
    }
}

impl Drop for HardwareVideoDecoder {
    fn drop(&mut self) {
        if let Some(buffer) = self.mapped.take() {
            if buffer.session_commands {
                let info = ScarletVideoSessionInfo {
                    stream_id: buffer.stream_id,
                    padding: 0,
                    buffer: ScarletVideoBufferInfo::default(),
                };
                let _ = self
                    .device
                    .as_handle()
                    .control(SCARLET_VIDEO_DESTROY_SESSION, &info as *const _ as usize);
            }
            let _ = munmap(buffer.ptr as usize, buffer.mmap_len);
        }
    }
}

enum DecodedVideoFrame {
    Software(Frame),
    Hardware(ScarletVideoFrame),
}

enum DisplayItem {
    Frame {
        frame: DecodedVideoFrame,
        presentation_time_us: u64,
        display_index: usize,
        total_frames: u32,
        seek_epoch: u32,
    },
    EndOfPass {
        seek_epoch: u32,
    },
}

// SAFETY: `DisplayItem::frame` converts every hardware frame to an owned
// payload before enqueueing, so display-thread items never carry mmap pointers
// borrowed from the decoder thread's reusable hardware buffer.
unsafe impl Send for DisplayItem {}

impl DisplayItem {
    fn frame(
        frame: DecodedVideoFrame,
        presentation_time_us: u64,
        display_index: usize,
        total_frames: u32,
        seek_epoch: u32,
    ) -> Self {
        let frame = match frame {
            DecodedVideoFrame::Software(frame) => DecodedVideoFrame::Software(frame),
            DecodedVideoFrame::Hardware(frame) => DecodedVideoFrame::Hardware(frame.into_owned()),
        };
        Self::Frame {
            frame,
            presentation_time_us,
            display_index,
            total_frames,
            seek_epoch,
        }
    }

    fn seek_epoch(&self) -> u32 {
        match self {
            Self::Frame { seek_epoch, .. } | Self::EndOfPass { seek_epoch } => *seek_epoch,
        }
    }

    fn estimated_bytes(&self) -> usize {
        match self {
            Self::Frame { frame, .. } => match frame {
                DecodedVideoFrame::Software(frame) => {
                    frame.width as usize * frame.height as usize * 3 / 2
                }
                DecodedVideoFrame::Hardware(frame) => {
                    let payload_len = frame.payload().len();
                    if payload_len != 0 {
                        payload_len
                    } else {
                        frame.width as usize * frame.height as usize * 3 / 2
                    }
                }
            },
            Self::EndOfPass { .. } => 0,
        }
    }
}

enum QueuePush {
    Pushed,
    StaleEpoch,
    Closed,
}

struct DisplayQueue {
    inner: Mutex<DisplayQueueInner>,
    clock: Option<Arc<AudioClock>>,
}

struct DisplayQueueInner {
    items: VecDeque<DisplayItem>,
    bytes: usize,
    closed: bool,
}

impl DisplayQueue {
    fn new(clock: Option<Arc<AudioClock>>) -> Self {
        Self {
            inner: Mutex::new(DisplayQueueInner {
                items: VecDeque::new(),
                bytes: 0,
                closed: false,
            }),
            clock,
        }
    }

    fn push_frame(&self, item: DisplayItem, controls: &ControlsOverlay) -> QueuePush {
        let item_bytes = item.estimated_bytes();
        let seek_epoch = item.seek_epoch();
        let pts = match &item {
            DisplayItem::Frame {
                presentation_time_us,
                ..
            } => Some(*presentation_time_us),
            DisplayItem::EndOfPass { .. } => None,
        };
        loop {
            let mut inner = self.inner.lock();
            if inner.closed {
                return QueuePush::Closed;
            }
            if controls.current_seek_epoch() != seek_epoch {
                return QueuePush::StaleEpoch;
            }
            if Self::can_fit(&inner, item_bytes) {
                inner.items.push_back(item);
                inner.bytes = inner.bytes.saturating_add(item_bytes);
                drop(inner);
                if let (Some(presentation_time_us), Some(clock)) = (pts, self.clock.as_deref()) {
                    Self::pace_to_audio(clock, controls, seek_epoch, presentation_time_us);
                }
                return QueuePush::Pushed;
            }
            drop(inner);
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Keep decode a fixed lead (`DECODE_TARGET_LEAD_FRAMES`) ahead of audio
    /// time once the audio clock is running, instead of racing to fill the
    /// whole queue. Smooths the burst-then-block decode CPU pattern into a
    /// steady ~realtime duty cycle while leaving a cushion for GOP-boundary
    /// decode jitter. Inactive before audio starts (or if absent) so the
    /// initial buffer still builds at full speed.
    fn pace_to_audio(
        clock: &AudioClock,
        controls: &ControlsOverlay,
        seek_epoch: u32,
        presentation_time_us: u64,
    ) {
        let target_lead_us = DECODE_TARGET_LEAD_FRAMES as u64 * FRAME_INTERVAL_MS * 1_000;
        loop {
            if controls.current_seek_epoch() != seek_epoch {
                return;
            }
            if clock.is_unavailable() || clock.is_finished() {
                return;
            }
            let Some(audio_us) = clock.elapsed_us() else {
                return;
            };
            if presentation_time_us <= audio_us.saturating_add(target_lead_us) {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn pop(&self) -> Option<DisplayItem> {
        loop {
            let mut inner = self.inner.lock();
            if let Some(item) = inner.items.pop_front() {
                inner.bytes = inner.bytes.saturating_sub(item.estimated_bytes());
                return Some(item);
            }
            if inner.closed {
                return None;
            }
            drop(inner);
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.items.clear();
        inner.bytes = 0;
    }

    fn close(&self) {
        let mut inner = self.inner.lock();
        inner.closed = true;
    }

    fn can_fit(inner: &DisplayQueueInner, item_bytes: usize) -> bool {
        inner.items.len() < DISPLAY_QUEUE_MAX_FRAMES
            && inner.bytes.saturating_add(item_bytes) <= DISPLAY_QUEUE_MAX_BYTES
    }
}

struct FrameReorderBuffer {
    pending: Vec<(usize, u64, DecodedVideoFrame)>,
    next_rank: usize,
    total_frames: u32,
    published: usize,
}

impl FrameReorderBuffer {
    fn new(total_frames: u32) -> Self {
        Self::new_from(total_frames, 0)
    }

    fn new_from(total_frames: u32, start_rank: usize) -> Self {
        Self {
            pending: Vec::new(),
            next_rank: start_rank,
            total_frames,
            published: start_rank,
        }
    }

    fn can_publish_immediately(&self, display_rank: usize) -> bool {
        self.pending.is_empty() && display_rank == self.next_rank
    }

    fn publish_immediate(
        &mut self,
        controls: &ControlsOverlay,
        queue: &DisplayQueue,
        presentation_time_us: u64,
        seek_epoch: u32,
        frame: DecodedVideoFrame,
    ) -> Result<bool, String> {
        match queue.push_frame(
            DisplayItem::frame(
                frame,
                presentation_time_us,
                self.published,
                self.total_frames,
                seek_epoch,
            ),
            controls,
        ) {
            QueuePush::Pushed => {}
            QueuePush::StaleEpoch => return Ok(false),
            QueuePush::Closed => return Ok(false),
        }
        self.next_rank += 1;
        self.published += 1;
        Ok(true)
    }

    fn push(
        &mut self,
        display_rank: usize,
        presentation_time_us: u64,
        frame: DecodedVideoFrame,
    ) -> Result<(), String> {
        if self
            .pending
            .iter()
            .any(|(rank, _, _)| *rank == display_rank)
        {
            return Err(String::from("MP4 display order has duplicate frame rank"));
        }
        self.pending
            .push((display_rank, presentation_time_us, frame));
        Ok(())
    }

    fn publish_ready(
        &mut self,
        controls: &ControlsOverlay,
        queue: &DisplayQueue,
        seek_epoch: u32,
    ) -> Result<bool, String> {
        while let Some(index) = self
            .pending
            .iter()
            .position(|(rank, _, _)| *rank == self.next_rank)
        {
            let (_, presentation_time_us, frame) = self.pending.remove(index);
            match queue.push_frame(
                DisplayItem::frame(
                    frame,
                    presentation_time_us,
                    self.published,
                    self.total_frames,
                    seek_epoch,
                ),
                controls,
            ) {
                QueuePush::Pushed => {}
                QueuePush::StaleEpoch => return Ok(false),
                QueuePush::Closed => return Ok(false),
            }
            self.next_rank += 1;
            self.published += 1;
        }
        Ok(true)
    }

    fn finish(
        &mut self,
        controls: &ControlsOverlay,
        queue: &DisplayQueue,
        seek_epoch: u32,
    ) -> Result<bool, String> {
        self.pending.sort_by_key(|(rank, _, _)| *rank);
        while !self.pending.is_empty() {
            let (_, presentation_time_us, frame) = self.pending.remove(0);
            match queue.push_frame(
                DisplayItem::frame(
                    frame,
                    presentation_time_us,
                    self.published,
                    self.total_frames,
                    seek_epoch,
                ),
                controls,
            ) {
                QueuePush::Pushed => {}
                QueuePush::StaleEpoch => return Ok(false),
                QueuePush::Closed => return Ok(false),
            }
            self.published += 1;
        }
        Ok(true)
    }

    fn published(&self) -> usize {
        self.published
    }

    fn set_total_frames(&mut self, total_frames: u32) {
        self.total_frames = total_frames.max(1);
    }
}

fn publish_hardware_seek_preview(
    source: &VideoSource,
    mp4_data: Option<&[u8]>,
    seek_plan: &VideoSeekPlan,
    frame_store: &VideoFrameStore,
    paint_signal: &PaintSignal,
    controls: &ControlsOverlay,
    access_unit_scratch: &mut Vec<u8>,
    total_frames: u32,
    seek_epoch: u32,
) -> Result<bool, String> {
    let Some(preview_index) = seek_plan.preview_index else {
        return Ok(true);
    };
    if controls.current_seek_epoch() != seek_epoch || controls.is_video_ready_for_seek(seek_epoch) {
        return Ok(controls.current_seek_epoch() == seek_epoch);
    }
    let access_unit = &source.access_units[preview_index];
    if access_unit.presentation_time_us < seek_plan.publish_target_us {
        return Ok(true);
    }
    let access_unit_bytes = access_unit.bytes(mp4_data, access_unit_scratch)?;
    let mut decoder = HardwareVideoDecoder::open()?;
    let Some(frame) = decoder.decode_access_unit(access_unit.codec, access_unit_bytes)? else {
        return Ok(true);
    };
    publish_seek_preview(
        frame_store,
        paint_signal,
        controls,
        DecodedVideoFrame::Hardware(frame),
        access_unit.display_rank,
        total_frames,
        access_unit.presentation_time_us,
        seek_epoch,
    )
}

fn stream_total_frames(source: &VideoSource, decoded: usize, complete: bool) -> u32 {
    let available = source.access_units.len();
    let total = if complete {
        available
    } else if let Some(estimated) = source.estimated_total_frames {
        usize::try_from(estimated)
            .unwrap_or(usize::MAX)
            .max(available)
    } else {
        available.max(decoded + 1)
    };
    total.min(u32::MAX as usize).max(1) as u32
}

fn stream_seek_target_available(source: &VideoSource, target_us: u64) -> bool {
    source
        .access_units
        .iter()
        .any(|unit| unit.presentation_time_us >= target_us)
}

fn stream_start_buffer_ready(source: &VideoSource) -> bool {
    if source.access_units.len() < STREAM_START_BUFFER_SAMPLES {
        return false;
    }
    let Some(first) = source.access_units.first() else {
        return false;
    };
    let Some(last) = source.access_units.last() else {
        return false;
    };
    last.presentation_time_us
        .saturating_sub(first.presentation_time_us)
        >= STREAM_START_BUFFER_US
}

fn parse_raw_annex_b(data: &[u8]) -> Vec<RawNalUnit<'_>> {
    let mut nals = Vec::new();
    let Some((mut nal_start, _)) = find_start_code(data, 0) else {
        return nals;
    };

    loop {
        if nal_start >= data.len() {
            break;
        }

        let (mut nal_end, next_start) = match find_start_code(data, nal_start) {
            Some((next_nal_start, next_code_start)) => (next_code_start, Some(next_nal_start)),
            None => (data.len(), None),
        };
        while nal_end > nal_start && data[nal_end - 1] == 0 {
            nal_end -= 1;
        }

        if nal_start < nal_end {
            let header = data[nal_start];
            if header & 0x80 == 0 {
                nals.push(RawNalUnit {
                    nal_type: header & 0x1f,
                    nal_ref_idc: (header >> 5) & 0x3,
                    offset: nal_start,
                    bytes: &data[nal_start..nal_end],
                });
            }
        }

        let Some(next_start) = next_start else {
            break;
        };
        nal_start = next_start;
    }

    nals
}

fn find_start_code(data: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut index = from;
    while index + 3 <= data.len() {
        if index + 3 <= data.len()
            && data[index] == 0
            && data[index + 1] == 0
            && data[index + 2] == 1
        {
            return Some((index + 3, index));
        }
        if index + 4 <= data.len()
            && data[index] == 0
            && data[index + 1] == 0
            && data[index + 2] == 0
            && data[index + 3] == 1
        {
            return Some((index + 4, index));
        }
        index += 1;
    }
    None
}

fn append_annex_b_nal(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
}

fn first_mb_in_slice(nal: &[u8]) -> Option<u32> {
    if nal.len() < 2 {
        return None;
    }
    let mut reader = EbspBitReader::new(&nal[1..]);
    reader.read_ue()
}

#[derive(Clone)]
struct EbspBitReader<'a> {
    bytes: &'a [u8],
    byte_index: usize,
    bit_index: u8,
    zero_count: u8,
}

impl<'a> EbspBitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            byte_index: 0,
            bit_index: 0,
            zero_count: 0,
        }
    }

    fn read_bit(&mut self) -> Option<u8> {
        loop {
            let byte = *self.bytes.get(self.byte_index)?;
            if self.zero_count >= 2 && byte == 0x03 {
                self.byte_index += 1;
                self.bit_index = 0;
                self.zero_count = 0;
                continue;
            }

            let bit = (byte >> (7 - self.bit_index)) & 1;
            self.bit_index += 1;
            if self.bit_index == 8 {
                self.byte_index += 1;
                self.bit_index = 0;
                if byte == 0 {
                    self.zero_count = self.zero_count.saturating_add(1);
                } else {
                    self.zero_count = 0;
                }
            }
            return Some(bit);
        }
    }

    fn read_ue(&mut self) -> Option<u32> {
        let mut leading_zero_bits = 0u32;
        while self.read_bit()? == 0 {
            leading_zero_bits += 1;
            if leading_zero_bits >= 32 {
                return None;
            }
        }

        let mut value = 1u32;
        for _ in 0..leading_zero_bits {
            value = (value << 1) | u32::from(self.read_bit()?);
        }
        Some(value - 1)
    }

    fn read_bits(&mut self, bits: u8) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..bits {
            value = (value << 1) | u32::from(self.read_bit()?);
        }
        Some(value)
    }

    fn read_se(&mut self) -> Option<i32> {
        let code_num = self.read_ue()?;
        let magnitude = code_num.div_ceil(2) as i32;
        if code_num & 1 == 0 {
            Some(-magnitude)
        } else {
            Some(magnitude)
        }
    }

    fn position_bits(&self) -> usize {
        self.byte_index * 8 + usize::from(self.bit_index)
    }
}

#[derive(Default)]
struct H264RequestContext {
    // The kernel ABI is stateless per request; userspace still keeps the
    // stream context needed to build those requests.
    sps: Option<ScarletVideoH264Sps>,
    pps: Option<ScarletVideoH264Pps>,
    scaling_matrix: ScarletVideoH264ScalingMatrix,
    pred_weights: ScarletVideoH264PredWeights,
    dpb: Vec<H264DpbFrame>,
    poc: H264PocState,
    next_timestamp: u64,
}

struct H264PreparedAccessUnit {
    params: ScarletVideoH264StatelessParams,
    timestamp: u64,
}

#[derive(Clone, Copy)]
struct H264DpbFrame {
    reference_ts: u64,
    pic_num: i32,
    frame_num: u16,
    top_field_order_cnt: i32,
    bottom_field_order_cnt: i32,
    long_term: bool,
}

#[derive(Clone, Copy, Default)]
struct H264PocState {
    prev_pic_order_cnt_msb: i32,
    prev_pic_order_cnt_lsb: u16,
}

#[derive(Clone, Copy)]
enum H264RefListModification {
    ShortTermSubtract(u32),
    ShortTermAdd(u32),
    LongTerm(u32),
}

#[derive(Default)]
struct H264RefPicMarking {
    idr_long_term: bool,
    adaptive: bool,
    operations: Vec<H264MemoryManagementControl>,
}

impl H264RefPicMarking {
    fn resets_poc(&self) -> bool {
        self.operations
            .iter()
            .any(|operation| matches!(operation, H264MemoryManagementControl::Reset))
    }
}

#[derive(Clone, Copy)]
enum H264MemoryManagementControl {
    ShortTermUnused {
        difference_of_pic_nums_minus1: u32,
    },
    LongTermUnused {
        long_term_pic_num: u32,
    },
    ShortTermToLongTerm {
        difference_of_pic_nums_minus1: u32,
        long_term_frame_idx: u32,
    },
    MaxLongTermFrameIdx {
        max_long_term_frame_idx_plus1: u32,
    },
    Reset,
    CurrentToLongTerm {
        long_term_frame_idx: u32,
    },
}

impl H264RequestContext {
    fn params_for_access_unit(
        &mut self,
        access_unit: &[u8],
    ) -> Result<H264PreparedAccessUnit, String> {
        let nals = parse_raw_annex_b(access_unit);
        let mut slice = None;
        let mut decode = None;
        let mut ref_pic_marking = None;

        for nal in &nals {
            match nal.nal_type {
                7 => {
                    self.sps = Some(parse_h264_sps(nal.bytes)?);
                }
                8 => {
                    self.pps = Some(parse_h264_pps(nal.bytes)?);
                }
                1 | 5 if slice.is_none() => {
                    let sps = self
                        .sps
                        .ok_or_else(|| String::from("H.264 stateless submit missing SPS"))?;
                    let pps = self
                        .pps
                        .ok_or_else(|| String::from("H.264 stateless submit missing PPS"))?;
                    let (slice_params, decode_params, marking) = parse_h264_slice(
                        nal,
                        &sps,
                        &pps,
                        &self.dpb,
                        &mut self.pred_weights,
                        &mut self.poc,
                    )?;
                    slice = Some(slice_params);
                    decode = Some(decode_params);
                    ref_pic_marking = Some(marking);
                }
                _ => {}
            }
        }

        let params = ScarletVideoH264StatelessParams {
            sps: self
                .sps
                .ok_or_else(|| String::from("H.264 stateless submit missing SPS"))?,
            pps: self
                .pps
                .ok_or_else(|| String::from("H.264 stateless submit missing PPS"))?,
            scaling_matrix: self.scaling_matrix,
            pred_weights: self.pred_weights,
            slice_params: slice
                .ok_or_else(|| String::from("H.264 stateless submit has no slice"))?,
            decode_params: decode
                .ok_or_else(|| String::from("H.264 stateless submit has no decode params"))?,
        };
        let timestamp = self.next_submit_timestamp();
        self.update_dpb_after_submit(
            &params,
            &ref_pic_marking.unwrap_or_else(H264RefPicMarking::default),
            timestamp,
        );
        Ok(H264PreparedAccessUnit { params, timestamp })
    }

    fn update_dpb_after_submit(
        &mut self,
        params: &ScarletVideoH264StatelessParams,
        marking: &H264RefPicMarking,
        timestamp: u64,
    ) {
        let is_idr = params.decode_params.flags & SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR != 0;
        let max_frame_num = h264_max_frame_num(&params.sps);
        let current_frame_num = params.decode_params.frame_num;
        let current_long_term_idx = if is_idr {
            self.dpb.clear();
            marking.idr_long_term.then_some(0)
        } else {
            self.apply_ref_pic_marking(marking, current_frame_num, max_frame_num)
        };

        if params.decode_params.nal_ref_idc == 0 {
            return;
        }
        self.dpb.push(H264DpbFrame {
            reference_ts: timestamp,
            pic_num: current_long_term_idx
                .and_then(|index| i32::try_from(index).ok())
                .unwrap_or_else(|| i32::from(params.decode_params.frame_num)),
            frame_num: params.decode_params.frame_num,
            top_field_order_cnt: params.decode_params.top_field_order_cnt,
            bottom_field_order_cnt: params.decode_params.bottom_field_order_cnt,
            long_term: current_long_term_idx.is_some(),
        });
        let max_refs = usize::from(params.sps.max_num_ref_frames).max(1);
        if !is_idr && marking.adaptive {
            self.cap_dpb(max_refs);
            return;
        }
        self.cap_dpb(max_refs);
    }

    fn apply_ref_pic_marking(
        &mut self,
        marking: &H264RefPicMarking,
        current_frame_num: u16,
        max_frame_num: u32,
    ) -> Option<u32> {
        if !marking.adaptive {
            return None;
        }

        let mut current_long_term_idx = None;
        for operation in &marking.operations {
            match *operation {
                H264MemoryManagementControl::ShortTermUnused {
                    difference_of_pic_nums_minus1,
                } => {
                    let target = h264_mmco_short_pic_num(
                        current_frame_num,
                        max_frame_num,
                        difference_of_pic_nums_minus1,
                    );
                    self.dpb.retain(|frame| {
                        frame.long_term
                            || h264_short_pic_num(frame, current_frame_num, max_frame_num) != target
                    });
                }
                H264MemoryManagementControl::LongTermUnused { long_term_pic_num } => {
                    self.dpb.retain(|frame| {
                        !frame.long_term
                            || frame.pic_num != i32::try_from(long_term_pic_num).unwrap_or(-1)
                    });
                }
                H264MemoryManagementControl::ShortTermToLongTerm {
                    difference_of_pic_nums_minus1,
                    long_term_frame_idx,
                } => {
                    let target = h264_mmco_short_pic_num(
                        current_frame_num,
                        max_frame_num,
                        difference_of_pic_nums_minus1,
                    );
                    self.dpb.retain(|frame| {
                        !frame.long_term
                            || frame.pic_num != i32::try_from(long_term_frame_idx).unwrap_or(-1)
                    });
                    for frame in &mut self.dpb {
                        if !frame.long_term
                            && h264_short_pic_num(frame, current_frame_num, max_frame_num) == target
                        {
                            frame.long_term = true;
                            frame.pic_num = i32::try_from(long_term_frame_idx).unwrap_or(-1);
                            break;
                        }
                    }
                }
                H264MemoryManagementControl::MaxLongTermFrameIdx {
                    max_long_term_frame_idx_plus1,
                } => {
                    if max_long_term_frame_idx_plus1 == 0 {
                        self.dpb.retain(|frame| !frame.long_term);
                    } else {
                        let max_idx =
                            i32::try_from(max_long_term_frame_idx_plus1 - 1).unwrap_or(i32::MAX);
                        self.dpb
                            .retain(|frame| !frame.long_term || frame.pic_num <= max_idx);
                    }
                }
                H264MemoryManagementControl::Reset => {
                    self.dpb.clear();
                    current_long_term_idx = None;
                }
                H264MemoryManagementControl::CurrentToLongTerm {
                    long_term_frame_idx,
                } => {
                    self.dpb.retain(|frame| {
                        !frame.long_term
                            || frame.pic_num != i32::try_from(long_term_frame_idx).unwrap_or(-1)
                    });
                    current_long_term_idx = Some(long_term_frame_idx);
                }
            }
        }
        current_long_term_idx
    }

    fn cap_dpb(&mut self, max_refs: usize) {
        while self.dpb.len() > max_refs {
            self.dpb.remove(0);
        }
    }

    fn next_submit_timestamp(&mut self) -> u64 {
        if self.next_timestamp == 0 {
            self.next_timestamp = 1;
        }
        let timestamp = self.next_timestamp;
        self.next_timestamp = self.next_timestamp.wrapping_add(1);
        if self.next_timestamp == 0 {
            self.next_timestamp = 1;
        }
        timestamp
    }
}

fn parse_h264_sps(nal: &[u8]) -> Result<ScarletVideoH264Sps, String> {
    if nal.len() < 2 {
        return Err(String::from("H.264 SPS is truncated"));
    }
    let mut reader = EbspBitReader::new(&nal[1..]);
    let profile_idc = read_u8_bits(&mut reader, 8, "H.264 SPS profile_idc")?;
    let constraint_set_flags = read_u8_bits(&mut reader, 8, "H.264 SPS constraint flags")?;
    let level_idc = read_u8_bits(&mut reader, 8, "H.264 SPS level_idc")?;
    let seq_parameter_set_id = read_u8_ue(&mut reader, "H.264 SPS id")?;

    let mut chroma_format_idc = 1u8;
    let mut bit_depth_luma_minus8 = 0u8;
    let mut bit_depth_chroma_minus8 = 0u8;
    let mut flags = 0u32;
    if is_h264_high_profile(profile_idc) {
        chroma_format_idc = read_u8_ue(&mut reader, "H.264 chroma_format_idc")?;
        if chroma_format_idc == 3 {
            if reader
                .read_bit()
                .ok_or_else(|| String::from("H.264 SPS separate_colour_plane_flag missing"))?
                != 0
            {
                flags |= SCARLET_VIDEO_H264_SPS_FLAG_SEPARATE_COLOUR_PLANE;
            }
        }
        bit_depth_luma_minus8 = read_u8_ue(&mut reader, "H.264 bit_depth_luma_minus8")?;
        bit_depth_chroma_minus8 = read_u8_ue(&mut reader, "H.264 bit_depth_chroma_minus8")?;
        if reader
            .read_bit()
            .ok_or_else(|| String::from("H.264 SPS transform bypass flag missing"))?
            != 0
        {
            flags |= SCARLET_VIDEO_H264_SPS_FLAG_QPPRIME_Y_ZERO_TRANSFORM_BYPASS;
        }
        let scaling_matrix_present = reader
            .read_bit()
            .ok_or_else(|| String::from("H.264 SPS scaling matrix flag missing"))?
            != 0;
        if scaling_matrix_present {
            let count = if chroma_format_idc != 3 { 8 } else { 12 };
            for index in 0..count {
                let present = reader
                    .read_bit()
                    .ok_or_else(|| String::from("H.264 SPS scaling list flag missing"))?
                    != 0;
                if present {
                    skip_h264_scaling_list(&mut reader, if index < 6 { 16 } else { 64 })?;
                }
            }
        }
    }

    let log2_max_frame_num_minus4 = read_u8_ue(&mut reader, "H.264 log2_max_frame_num_minus4")?;
    let pic_order_cnt_type = read_u8_ue(&mut reader, "H.264 pic_order_cnt_type")?;
    let mut log2_max_pic_order_cnt_lsb_minus4 = 0u8;
    let mut offset_for_ref_frame = [0i32; 255];
    let mut offset_for_non_ref_pic = 0i32;
    let mut offset_for_top_to_bottom_field = 0i32;
    let mut num_ref_frames_in_pic_order_cnt_cycle = 0u8;
    match pic_order_cnt_type {
        0 => {
            log2_max_pic_order_cnt_lsb_minus4 =
                read_u8_ue(&mut reader, "H.264 log2_max_pic_order_cnt_lsb_minus4")?;
        }
        1 => {
            if reader
                .read_bit()
                .ok_or_else(|| String::from("H.264 delta_pic_order_always_zero_flag missing"))?
                != 0
            {
                flags |= SCARLET_VIDEO_H264_SPS_FLAG_DELTA_PIC_ORDER_ALWAYS_ZERO;
            }
            offset_for_non_ref_pic = reader
                .read_se()
                .ok_or_else(|| String::from("H.264 offset_for_non_ref_pic missing"))?;
            offset_for_top_to_bottom_field = reader
                .read_se()
                .ok_or_else(|| String::from("H.264 offset_for_top_to_bottom_field missing"))?;
            let count = read_u8_ue(&mut reader, "H.264 num_ref_frames_in_pic_order_cnt_cycle")?;
            num_ref_frames_in_pic_order_cnt_cycle = count;
            for index in 0..usize::from(count) {
                offset_for_ref_frame[index] = reader
                    .read_se()
                    .ok_or_else(|| String::from("H.264 offset_for_ref_frame missing"))?;
            }
        }
        _ => return Err(String::from("H.264 pic_order_cnt_type is unsupported")),
    }
    let max_num_ref_frames = read_u8_ue(&mut reader, "H.264 max_num_ref_frames")?;
    if reader
        .read_bit()
        .ok_or_else(|| String::from("H.264 gaps_in_frame_num_value_allowed_flag missing"))?
        != 0
    {
        flags |= SCARLET_VIDEO_H264_SPS_FLAG_GAPS_IN_FRAME_NUM_VALUE_ALLOWED;
    }
    let pic_width_in_mbs_minus1 = read_u16_ue(&mut reader, "H.264 pic_width_in_mbs_minus1")?;
    let pic_height_in_map_units_minus1 =
        read_u16_ue(&mut reader, "H.264 pic_height_in_map_units_minus1")?;
    if reader
        .read_bit()
        .ok_or_else(|| String::from("H.264 frame_mbs_only_flag missing"))?
        != 0
    {
        flags |= SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY;
    } else if reader
        .read_bit()
        .ok_or_else(|| String::from("H.264 mb_adaptive_frame_field_flag missing"))?
        != 0
    {
        flags |= SCARLET_VIDEO_H264_SPS_FLAG_MB_ADAPTIVE_FRAME_FIELD;
    }
    if reader
        .read_bit()
        .ok_or_else(|| String::from("H.264 direct_8x8_inference_flag missing"))?
        != 0
    {
        flags |= SCARLET_VIDEO_H264_SPS_FLAG_DIRECT_8X8_INFERENCE;
    }
    let mut frame_crop_left_offset = 0;
    let mut frame_crop_right_offset = 0;
    let mut frame_crop_top_offset = 0;
    let mut frame_crop_bottom_offset = 0;
    if reader
        .read_bit()
        .ok_or_else(|| String::from("H.264 frame_cropping_flag missing"))?
        != 0
    {
        flags |= SCARLET_VIDEO_H264_SPS_FLAG_FRAME_CROPPING;
        frame_crop_left_offset = read_u32_ue(&mut reader, "H.264 frame_crop_left_offset")?;
        frame_crop_right_offset = read_u32_ue(&mut reader, "H.264 frame_crop_right_offset")?;
        frame_crop_top_offset = read_u32_ue(&mut reader, "H.264 frame_crop_top_offset")?;
        frame_crop_bottom_offset = read_u32_ue(&mut reader, "H.264 frame_crop_bottom_offset")?;
    }

    Ok(ScarletVideoH264Sps {
        profile_idc,
        constraint_set_flags,
        level_idc,
        seq_parameter_set_id,
        chroma_format_idc,
        bit_depth_luma_minus8,
        bit_depth_chroma_minus8,
        log2_max_frame_num_minus4,
        pic_order_cnt_type,
        log2_max_pic_order_cnt_lsb_minus4,
        max_num_ref_frames,
        num_ref_frames_in_pic_order_cnt_cycle,
        offset_for_ref_frame,
        offset_for_non_ref_pic,
        offset_for_top_to_bottom_field,
        pic_width_in_mbs_minus1,
        pic_height_in_map_units_minus1,
        frame_crop_left_offset,
        frame_crop_right_offset,
        frame_crop_top_offset,
        frame_crop_bottom_offset,
        flags,
    })
}

fn parse_h264_pps(nal: &[u8]) -> Result<ScarletVideoH264Pps, String> {
    if nal.len() < 2 {
        return Err(String::from("H.264 PPS is truncated"));
    }
    let mut reader = EbspBitReader::new(&nal[1..]);
    let pic_parameter_set_id = read_u8_ue(&mut reader, "H.264 PPS id")?;
    let seq_parameter_set_id = read_u8_ue(&mut reader, "H.264 PPS SPS id")?;
    let mut flags = 0u16;
    if read_bool(&mut reader, "H.264 entropy_coding_mode_flag")? {
        flags |= SCARLET_VIDEO_H264_PPS_FLAG_ENTROPY_CODING_MODE;
    }
    if read_bool(
        &mut reader,
        "H.264 bottom_field_pic_order_in_frame_present_flag",
    )? {
        flags |= SCARLET_VIDEO_H264_PPS_FLAG_BOTTOM_FIELD_PIC_ORDER_IN_FRAME_PRESENT;
    }
    let num_slice_groups_minus1 = read_u8_ue(&mut reader, "H.264 num_slice_groups_minus1")?;
    if num_slice_groups_minus1 != 0 {
        return Err(String::from("H.264 slice groups are unsupported"));
    }
    let num_ref_idx_l0_default_active_minus1 =
        read_u8_ue(&mut reader, "H.264 num_ref_idx_l0_default_active_minus1")?;
    let num_ref_idx_l1_default_active_minus1 =
        read_u8_ue(&mut reader, "H.264 num_ref_idx_l1_default_active_minus1")?;
    if read_bool(&mut reader, "H.264 weighted_pred_flag")? {
        flags |= SCARLET_VIDEO_H264_PPS_FLAG_WEIGHTED_PRED;
    }
    let weighted_bipred_idc = read_u8_bits(&mut reader, 2, "H.264 weighted_bipred_idc")?;
    let pic_init_qp_minus26 = read_i8_se(&mut reader, "H.264 pic_init_qp_minus26")?;
    let pic_init_qs_minus26 = read_i8_se(&mut reader, "H.264 pic_init_qs_minus26")?;
    let chroma_qp_index_offset = read_i8_se(&mut reader, "H.264 chroma_qp_index_offset")?;
    if read_bool(&mut reader, "H.264 deblocking_filter_control_present_flag")? {
        flags |= SCARLET_VIDEO_H264_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT;
    }
    if read_bool(&mut reader, "H.264 constrained_intra_pred_flag")? {
        flags |= SCARLET_VIDEO_H264_PPS_FLAG_CONSTRAINED_INTRA_PRED;
    }
    if read_bool(&mut reader, "H.264 redundant_pic_cnt_present_flag")? {
        flags |= SCARLET_VIDEO_H264_PPS_FLAG_REDUNDANT_PIC_CNT_PRESENT;
    }
    if h264_more_rbsp_data(&reader) {
        if read_bool(&mut reader, "H.264 transform_8x8_mode_flag")? {
            flags |= SCARLET_VIDEO_H264_PPS_FLAG_TRANSFORM_8X8_MODE;
        }
        if read_bool(&mut reader, "H.264 pic_scaling_matrix_present_flag")? {
            return Err(String::from("H.264 PPS scaling matrices are unsupported"));
        }
        if h264_more_rbsp_data(&reader) {
            let second_chroma_qp_index_offset =
                read_i8_se(&mut reader, "H.264 second_chroma_qp_index_offset")?;
            return Ok(ScarletVideoH264Pps {
                pic_parameter_set_id,
                seq_parameter_set_id,
                num_slice_groups_minus1,
                num_ref_idx_l0_default_active_minus1,
                num_ref_idx_l1_default_active_minus1,
                weighted_bipred_idc,
                pic_init_qp_minus26,
                pic_init_qs_minus26,
                chroma_qp_index_offset,
                second_chroma_qp_index_offset,
                flags,
            });
        }
    }

    Ok(ScarletVideoH264Pps {
        pic_parameter_set_id,
        seq_parameter_set_id,
        num_slice_groups_minus1,
        num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1,
        weighted_bipred_idc,
        pic_init_qp_minus26,
        pic_init_qs_minus26,
        chroma_qp_index_offset,
        second_chroma_qp_index_offset: chroma_qp_index_offset,
        flags,
    })
}

fn parse_h264_slice(
    nal: &RawNalUnit<'_>,
    sps: &ScarletVideoH264Sps,
    pps: &ScarletVideoH264Pps,
    dpb: &[H264DpbFrame],
    pred_weights: &mut ScarletVideoH264PredWeights,
    poc_state: &mut H264PocState,
) -> Result<
    (
        ScarletVideoH264SliceParams,
        ScarletVideoH264DecodeParams,
        H264RefPicMarking,
    ),
    String,
> {
    if nal.bytes.len() < 2 {
        return Err(String::from("H.264 slice is truncated"));
    }
    let mut reader = EbspBitReader::new(&nal.bytes[1..]);
    let first_mb_in_slice = read_u32_ue(&mut reader, "H.264 first_mb_in_slice")?;
    let slice_type = read_u8_ue(&mut reader, "H.264 slice_type")?;
    let pic_parameter_set_id = read_u8_ue(&mut reader, "H.264 slice PPS id")?;
    if pic_parameter_set_id != pps.pic_parameter_set_id {
        return Err(String::from("H.264 slice references unknown PPS"));
    }

    let mut colour_plane_id = 0;
    if sps.flags & SCARLET_VIDEO_H264_SPS_FLAG_SEPARATE_COLOUR_PLANE != 0 {
        colour_plane_id = read_u8_bits(&mut reader, 2, "H.264 colour_plane_id")?;
    }
    let frame_num_bits = sps.log2_max_frame_num_minus4.saturating_add(4);
    let frame_num = read_u16_bits(&mut reader, frame_num_bits, "H.264 frame_num")?;
    if sps.flags & SCARLET_VIDEO_H264_SPS_FLAG_FRAME_MBS_ONLY == 0 {
        return Err(String::from("H.264 field pictures are unsupported"));
    }

    let mut decode_params = ScarletVideoH264DecodeParams {
        nal_ref_idc: u16::from(nal.nal_ref_idc),
        frame_num,
        ..Default::default()
    };
    let max_frame_num = h264_max_frame_num(sps);
    fill_h264_decode_dpb(&mut decode_params, dpb, frame_num, max_frame_num);
    if nal.nal_type == 5 {
        decode_params.flags |= SCARLET_VIDEO_H264_DECODE_PARAM_FLAG_IDR;
        decode_params.idr_pic_id = read_u16_ue(&mut reader, "H.264 idr_pic_id")?;
    }
    if sps.pic_order_cnt_type == 0 {
        let poc_bits = sps.log2_max_pic_order_cnt_lsb_minus4.saturating_add(4);
        decode_params.pic_order_cnt_lsb =
            read_u16_bits(&mut reader, poc_bits, "H.264 pic_order_cnt_lsb")?;
        let pic_order_cnt_msb =
            h264_pic_order_cnt_msb(sps, nal, decode_params.pic_order_cnt_lsb, poc_state);
        decode_params.top_field_order_cnt =
            pic_order_cnt_msb.saturating_add(i32::from(decode_params.pic_order_cnt_lsb));
        decode_params.bottom_field_order_cnt = decode_params.top_field_order_cnt;
        if pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_BOTTOM_FIELD_PIC_ORDER_IN_FRAME_PRESENT != 0 {
            decode_params.delta_pic_order_cnt_bottom = reader
                .read_se()
                .ok_or_else(|| String::from("H.264 delta_pic_order_cnt_bottom missing"))?;
            decode_params.bottom_field_order_cnt = decode_params
                .top_field_order_cnt
                .saturating_add(decode_params.delta_pic_order_cnt_bottom);
        }
    } else if sps.pic_order_cnt_type == 1
        && sps.flags & SCARLET_VIDEO_H264_SPS_FLAG_DELTA_PIC_ORDER_ALWAYS_ZERO == 0
    {
        decode_params.delta_pic_order_cnt0 = reader
            .read_se()
            .ok_or_else(|| String::from("H.264 delta_pic_order_cnt0 missing"))?;
        if pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_BOTTOM_FIELD_PIC_ORDER_IN_FRAME_PRESENT != 0 {
            decode_params.delta_pic_order_cnt1 = reader
                .read_se()
                .ok_or_else(|| String::from("H.264 delta_pic_order_cnt1 missing"))?;
        }
        decode_params.top_field_order_cnt = decode_params.delta_pic_order_cnt0;
        decode_params.bottom_field_order_cnt = decode_params
            .top_field_order_cnt
            .saturating_add(decode_params.delta_pic_order_cnt1);
    }
    let mut redundant_pic_cnt = 0;
    if pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_REDUNDANT_PIC_CNT_PRESENT != 0 {
        redundant_pic_cnt = read_u8_ue(&mut reader, "H.264 redundant_pic_cnt")?;
    }

    let slice_class = slice_type % 5;
    let mut num_ref_idx_l0_active_minus1 = pps.num_ref_idx_l0_default_active_minus1;
    let mut num_ref_idx_l1_active_minus1 = pps.num_ref_idx_l1_default_active_minus1;
    let mut slice_flags = 0u32;
    if slice_class == 1 {
        if read_bool(&mut reader, "H.264 direct_spatial_mv_pred_flag")? {
            slice_flags |= SCARLET_VIDEO_H264_SLICE_FLAG_DIRECT_SPATIAL_MV_PRED;
        }
    }
    if slice_class == 0 || slice_class == 1 || slice_class == 3 {
        let override_refs = read_bool(&mut reader, "H.264 num_ref_idx_active_override_flag")?;
        if override_refs {
            num_ref_idx_l0_active_minus1 =
                read_u8_ue(&mut reader, "H.264 num_ref_idx_l0_active_minus1")?;
            if slice_class == 1 {
                num_ref_idx_l1_active_minus1 =
                    read_u8_ue(&mut reader, "H.264 num_ref_idx_l1_active_minus1")?;
            }
        }
    }
    let (list0_modifications, list1_modifications) =
        parse_h264_ref_pic_list_modification(&mut reader, slice_class)?;
    let (ref_pic_list0, ref_pic_list1) = build_h264_ref_pic_lists(
        dpb,
        &decode_params,
        sps,
        slice_class,
        num_ref_idx_l0_active_minus1,
        num_ref_idx_l1_active_minus1,
        &list0_modifications,
        &list1_modifications,
    );
    if slice_class == 0 || slice_class == 1 || slice_class == 3 {
        slice_flags |= SCARLET_VIDEO_H264_SLICE_FLAG_REF_LISTS_PRESENT;
    }
    parse_h264_pred_weight_table(
        &mut reader,
        sps,
        pps,
        slice_class,
        num_ref_idx_l0_active_minus1,
        num_ref_idx_l1_active_minus1,
        pred_weights,
    )?;

    let dec_ref_start_bits = reader.position_bits();
    let ref_pic_marking = if nal.nal_ref_idc != 0 {
        parse_h264_dec_ref_pic_marking(&mut reader, nal.nal_type)?
    } else {
        H264RefPicMarking::default()
    };
    decode_params.dec_ref_pic_marking_bit_size =
        reader.position_bits().saturating_sub(dec_ref_start_bits) as u32;

    let mut cabac_init_idc = 0;
    if pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_ENTROPY_CODING_MODE != 0
        && slice_class != 2
        && slice_class != 4
    {
        cabac_init_idc = read_u8_ue(&mut reader, "H.264 cabac_init_idc")?;
    }
    let slice_qp_delta = read_i8_se(&mut reader, "H.264 slice_qp_delta")?;
    let mut slice_qs_delta = 0;
    if slice_class == 3 || slice_class == 4 {
        if slice_class == 3 {
            let _sp_for_switch_flag = read_bool(&mut reader, "H.264 sp_for_switch_flag")?;
        }
        slice_qs_delta = read_i8_se(&mut reader, "H.264 slice_qs_delta")?;
    }

    let mut disable_deblocking_filter_idc = 0;
    let mut slice_alpha_c0_offset_div2 = 0;
    let mut slice_beta_offset_div2 = 0;
    if pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_DEBLOCKING_FILTER_CONTROL_PRESENT != 0 {
        disable_deblocking_filter_idc =
            read_u8_ue(&mut reader, "H.264 disable_deblocking_filter_idc")?;
        if disable_deblocking_filter_idc != 1 {
            slice_alpha_c0_offset_div2 =
                read_i8_se(&mut reader, "H.264 slice_alpha_c0_offset_div2")?;
            slice_beta_offset_div2 = read_i8_se(&mut reader, "H.264 slice_beta_offset_div2")?;
        }
    }

    if sps.pic_order_cnt_type == 0 && nal.nal_ref_idc != 0 {
        if ref_pic_marking.resets_poc() {
            poc_state.prev_pic_order_cnt_msb = 0;
            poc_state.prev_pic_order_cnt_lsb = 0;
        } else {
            poc_state.prev_pic_order_cnt_msb = decode_params
                .top_field_order_cnt
                .saturating_sub(i32::from(decode_params.pic_order_cnt_lsb));
            poc_state.prev_pic_order_cnt_lsb = decode_params.pic_order_cnt_lsb;
        }
    }

    let mut slice_params = ScarletVideoH264SliceParams {
        header_bit_size: reader.position_bits() as u32,
        nal_offset: nal.offset as u32,
        nal_len: nal.bytes.len() as u32,
        first_mb_in_slice,
        slice_type,
        pic_parameter_set_id,
        colour_plane_id,
        redundant_pic_cnt,
        cabac_init_idc,
        slice_qp_delta,
        slice_qs_delta,
        disable_deblocking_filter_idc,
        slice_alpha_c0_offset_div2,
        slice_beta_offset_div2,
        num_ref_idx_l0_active_minus1,
        num_ref_idx_l1_active_minus1,
        flags: slice_flags,
        ..Default::default()
    };
    slice_params.ref_pic_list0 = ref_pic_list0;
    slice_params.ref_pic_list1 = ref_pic_list1;
    Ok((slice_params, decode_params, ref_pic_marking))
}

fn h264_pic_order_cnt_msb(
    sps: &ScarletVideoH264Sps,
    nal: &RawNalUnit<'_>,
    pic_order_cnt_lsb: u16,
    poc_state: &H264PocState,
) -> i32 {
    let poc_bits = u32::from(sps.log2_max_pic_order_cnt_lsb_minus4.saturating_add(4));
    let max_pic_order_cnt_lsb = 1i32.checked_shl(poc_bits).unwrap_or(i32::MAX);
    if nal.nal_type == 5 || max_pic_order_cnt_lsb <= 0 {
        return 0;
    }

    let pic_order_cnt_lsb = i32::from(pic_order_cnt_lsb);
    let prev_pic_order_cnt_lsb = i32::from(poc_state.prev_pic_order_cnt_lsb);
    let prev_pic_order_cnt_msb = poc_state.prev_pic_order_cnt_msb;
    let half_range = max_pic_order_cnt_lsb / 2;

    if pic_order_cnt_lsb < prev_pic_order_cnt_lsb
        && prev_pic_order_cnt_lsb - pic_order_cnt_lsb >= half_range
    {
        prev_pic_order_cnt_msb.saturating_add(max_pic_order_cnt_lsb)
    } else if pic_order_cnt_lsb > prev_pic_order_cnt_lsb
        && pic_order_cnt_lsb - prev_pic_order_cnt_lsb > half_range
    {
        prev_pic_order_cnt_msb.saturating_sub(max_pic_order_cnt_lsb)
    } else {
        prev_pic_order_cnt_msb
    }
}

fn fill_h264_decode_dpb(
    decode: &mut ScarletVideoH264DecodeParams,
    dpb: &[H264DpbFrame],
    current_frame_num: u16,
    max_frame_num: u32,
) {
    for (index, frame) in dpb.iter().take(16).enumerate() {
        decode.dpb[index] = ScarletVideoH264DpbEntry {
            reference_ts: frame.reference_ts,
            pic_num: if frame.long_term {
                frame.pic_num
            } else {
                h264_short_pic_num(frame, current_frame_num, max_frame_num)
            },
            frame_num: frame.frame_num,
            fields: 3,
            reserved: [0; 5],
            top_field_order_cnt: frame.top_field_order_cnt,
            bottom_field_order_cnt: frame.bottom_field_order_cnt,
            flags: SCARLET_VIDEO_H264_DPB_FLAG_VALID
                | if frame.long_term {
                    SCARLET_VIDEO_H264_DPB_FLAG_LONG_TERM
                } else {
                    0
                },
        };
    }
}

fn parse_h264_ref_pic_list_modification(
    reader: &mut EbspBitReader<'_>,
    slice_class: u8,
) -> Result<(Vec<H264RefListModification>, Vec<H264RefListModification>), String> {
    if slice_class == 2 || slice_class == 4 {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut list0 = Vec::new();
    if read_bool(reader, "H.264 ref_pic_list_modification_flag_l0")? {
        loop {
            let idc = read_u32_ue(reader, "H.264 modification_of_pic_nums_idc_l0")?;
            match idc {
                0 | 1 => {
                    let abs_diff_pic_num_minus1 =
                        read_u32_ue(reader, "H.264 abs_diff_pic_num_minus1_l0")?;
                    list0.push(if idc == 0 {
                        H264RefListModification::ShortTermSubtract(abs_diff_pic_num_minus1)
                    } else {
                        H264RefListModification::ShortTermAdd(abs_diff_pic_num_minus1)
                    });
                }
                2 => {
                    let long_term_pic_num = read_u32_ue(reader, "H.264 long_term_pic_num_l0")?;
                    list0.push(H264RefListModification::LongTerm(long_term_pic_num));
                }
                3 => break,
                _ => return Err(String::from("H.264 invalid ref list modification idc")),
            }
        }
    }

    let mut list1 = Vec::new();
    if slice_class == 1 && read_bool(reader, "H.264 ref_pic_list_modification_flag_l1")? {
        loop {
            let idc = read_u32_ue(reader, "H.264 modification_of_pic_nums_idc_l1")?;
            match idc {
                0 | 1 => {
                    let abs_diff_pic_num_minus1 =
                        read_u32_ue(reader, "H.264 abs_diff_pic_num_minus1_l1")?;
                    list1.push(if idc == 0 {
                        H264RefListModification::ShortTermSubtract(abs_diff_pic_num_minus1)
                    } else {
                        H264RefListModification::ShortTermAdd(abs_diff_pic_num_minus1)
                    });
                }
                2 => {
                    let long_term_pic_num = read_u32_ue(reader, "H.264 long_term_pic_num_l1")?;
                    list1.push(H264RefListModification::LongTerm(long_term_pic_num));
                }
                3 => break,
                _ => return Err(String::from("H.264 invalid ref list modification idc")),
            }
        }
    }

    Ok((list0, list1))
}

fn build_h264_ref_pic_lists(
    dpb: &[H264DpbFrame],
    decode: &ScarletVideoH264DecodeParams,
    sps: &ScarletVideoH264Sps,
    slice_class: u8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    list0_modifications: &[H264RefListModification],
    list1_modifications: &[H264RefListModification],
) -> (
    [ScarletVideoH264Reference; 32],
    [ScarletVideoH264Reference; 32],
) {
    let mut list0 = Vec::new();
    let mut list1 = Vec::new();
    let l0_active = h264_active_count(num_ref_idx_l0_active_minus1);
    let l1_active = h264_active_count(num_ref_idx_l1_active_minus1);
    let max_frame_num = h264_max_frame_num(sps);

    match slice_class {
        0 | 3 => {
            list0 = h264_default_p_ref_list(dpb, decode.frame_num, max_frame_num);
        }
        1 => {
            let (default_l0, default_l1) =
                h264_default_b_ref_lists(dpb, decode.top_field_order_cnt);
            list0 = default_l0;
            list1 = default_l1;
        }
        _ => {}
    }

    apply_h264_ref_list_modifications(
        &mut list0,
        list0_modifications,
        dpb,
        decode.frame_num,
        max_frame_num,
        l0_active,
    );
    if slice_class == 1 {
        apply_h264_ref_list_modifications(
            &mut list1,
            list1_modifications,
            dpb,
            decode.frame_num,
            max_frame_num,
            l1_active,
        );
    }

    (
        write_h264_ref_list(&list0, l0_active),
        write_h264_ref_list(&list1, l1_active),
    )
}

fn h264_default_p_ref_list(
    dpb: &[H264DpbFrame],
    current_frame_num: u16,
    max_frame_num: u32,
) -> Vec<usize> {
    let mut refs = Vec::new();
    for (index, frame) in dpb.iter().enumerate().rev() {
        if !frame.long_term {
            refs.push(index);
        }
    }
    refs.sort_by(|left, right| {
        h264_short_pic_num(&dpb[*right], current_frame_num, max_frame_num).cmp(&h264_short_pic_num(
            &dpb[*left],
            current_frame_num,
            max_frame_num,
        ))
    });
    for (index, frame) in dpb.iter().enumerate().rev() {
        if frame.long_term {
            refs.push(index);
        }
    }
    refs
}

fn h264_default_b_ref_lists(dpb: &[H264DpbFrame], current_poc: i32) -> (Vec<usize>, Vec<usize>) {
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut long_term = Vec::new();
    for (index, frame) in dpb.iter().enumerate() {
        if frame.long_term {
            long_term.push(index);
        } else if frame.top_field_order_cnt < current_poc {
            before.push(index);
        } else {
            after.push(index);
        }
    }

    before.sort_by(|left, right| {
        dpb[*right]
            .top_field_order_cnt
            .cmp(&dpb[*left].top_field_order_cnt)
    });
    after.sort_by(|left, right| {
        dpb[*left]
            .top_field_order_cnt
            .cmp(&dpb[*right].top_field_order_cnt)
    });
    long_term.sort_by(|left, right| dpb[*left].pic_num.cmp(&dpb[*right].pic_num));

    let mut list0 = Vec::new();
    list0.extend(before.iter().copied());
    list0.extend(after.iter().copied());
    list0.extend(long_term.iter().copied());

    let mut list1 = Vec::new();
    list1.extend(after.iter().copied());
    list1.extend(before.iter().copied());
    list1.extend(long_term.iter().copied());
    if list0 == list1 && list1.len() > 1 {
        list1.swap(0, 1);
    }

    (list0, list1)
}

fn apply_h264_ref_list_modifications(
    list: &mut Vec<usize>,
    modifications: &[H264RefListModification],
    dpb: &[H264DpbFrame],
    current_frame_num: u16,
    max_frame_num: u32,
    active_count: usize,
) {
    if active_count == 0 {
        list.clear();
        return;
    }

    let max_frame_num_u32 = max_frame_num.max(1);
    let mut pic_num_pred = i64::from(current_frame_num);
    let max_frame_num = i64::from(max_frame_num_u32);
    let mut ref_idx = 0usize;
    for modification in modifications {
        let dpb_index = match *modification {
            H264RefListModification::ShortTermSubtract(abs_diff_pic_num_minus1) => {
                let diff = i64::from(abs_diff_pic_num_minus1) + 1;
                let mut pic_num_no_wrap = pic_num_pred - diff;
                if pic_num_no_wrap < 0 {
                    pic_num_no_wrap += max_frame_num;
                }
                pic_num_pred = pic_num_no_wrap;
                let pic_num = if pic_num_no_wrap > i64::from(current_frame_num) {
                    pic_num_no_wrap - max_frame_num
                } else {
                    pic_num_no_wrap
                };
                find_h264_short_ref_by_pic_num(dpb, current_frame_num, max_frame_num_u32, pic_num)
            }
            H264RefListModification::ShortTermAdd(abs_diff_pic_num_minus1) => {
                let diff = i64::from(abs_diff_pic_num_minus1) + 1;
                let mut pic_num_no_wrap = pic_num_pred + diff;
                if pic_num_no_wrap >= max_frame_num {
                    pic_num_no_wrap -= max_frame_num;
                }
                pic_num_pred = pic_num_no_wrap;
                let pic_num = if pic_num_no_wrap > i64::from(current_frame_num) {
                    pic_num_no_wrap - max_frame_num
                } else {
                    pic_num_no_wrap
                };
                find_h264_short_ref_by_pic_num(dpb, current_frame_num, max_frame_num_u32, pic_num)
            }
            H264RefListModification::LongTerm(long_term_pic_num) => dpb.iter().position(|frame| {
                frame.long_term && frame.pic_num == i32::try_from(long_term_pic_num).unwrap_or(-1)
            }),
        };

        let Some(dpb_index) = dpb_index else {
            continue;
        };
        if ref_idx > list.len() {
            list.push(dpb_index);
        } else {
            list.insert(ref_idx, dpb_index);
        }
        let mut scan = ref_idx + 1;
        while scan < list.len() {
            if list[scan] == dpb_index {
                list.remove(scan);
            } else {
                scan += 1;
            }
        }
        ref_idx = ref_idx.saturating_add(1).min(active_count);
    }

    if list.is_empty() {
        return;
    }
    while list.len() < active_count {
        let last = *list.last().unwrap_or(&0);
        list.push(last);
    }
    list.truncate(active_count);
}

fn find_h264_short_ref_by_pic_num(
    dpb: &[H264DpbFrame],
    current_frame_num: u16,
    max_frame_num: u32,
    pic_num: i64,
) -> Option<usize> {
    dpb.iter().position(|frame| {
        !frame.long_term
            && i64::from(h264_short_pic_num(frame, current_frame_num, max_frame_num)) == pic_num
    })
}

fn write_h264_ref_list(list: &[usize], active_count: usize) -> [ScarletVideoH264Reference; 32] {
    let mut refs = [ScarletVideoH264Reference::default(); 32];
    for (output_index, dpb_index) in list.iter().copied().take(active_count.min(32)).enumerate() {
        refs[output_index] = ScarletVideoH264Reference {
            fields: 3,
            index: dpb_index as u8,
        };
    }
    refs
}

fn h264_active_count(active_minus1: u8) -> usize {
    usize::from(active_minus1).saturating_add(1).min(32)
}

fn h264_max_frame_num(sps: &ScarletVideoH264Sps) -> u32 {
    1u32.checked_shl(u32::from(sps.log2_max_frame_num_minus4.saturating_add(4)))
        .unwrap_or(u32::MAX)
}

fn h264_short_pic_num(frame: &H264DpbFrame, current_frame_num: u16, max_frame_num: u32) -> i32 {
    let max_frame_num = i32::try_from(max_frame_num).unwrap_or(i32::MAX);
    let frame_num = i32::from(frame.frame_num);
    if frame.frame_num > current_frame_num {
        frame_num.saturating_sub(max_frame_num)
    } else {
        frame_num
    }
}

fn h264_mmco_short_pic_num(
    current_frame_num: u16,
    _max_frame_num: u32,
    difference_of_pic_nums_minus1: u32,
) -> i32 {
    i32::from(current_frame_num)
        .saturating_sub(i32::try_from(difference_of_pic_nums_minus1).unwrap_or(i32::MAX))
        .saturating_sub(1)
}

fn parse_h264_pred_weight_table(
    reader: &mut EbspBitReader<'_>,
    sps: &ScarletVideoH264Sps,
    pps: &ScarletVideoH264Pps,
    slice_class: u8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    pred_weights: &mut ScarletVideoH264PredWeights,
) -> Result<(), String> {
    let weighted_p = pps.flags & SCARLET_VIDEO_H264_PPS_FLAG_WEIGHTED_PRED != 0
        && (slice_class == 0 || slice_class == 3);
    let weighted_b = pps.weighted_bipred_idc == 1 && slice_class == 1;
    *pred_weights = ScarletVideoH264PredWeights::default();
    if !weighted_p && !weighted_b {
        return Ok(());
    }

    let luma_denom = read_u16_ue(reader, "H.264 luma_log2_weight_denom")?;
    let chroma_denom = if sps.chroma_format_idc != 0 {
        read_u16_ue(reader, "H.264 chroma_log2_weight_denom")?
    } else {
        0
    };
    pred_weights.luma_log2_weight_denom = luma_denom;
    pred_weights.chroma_log2_weight_denom = chroma_denom;

    let list_count = if slice_class == 1 { 2 } else { 1 };
    for list in 0..list_count {
        let active = if list == 0 {
            num_ref_idx_l0_active_minus1
        } else {
            num_ref_idx_l1_active_minus1
        };
        for index in 0..=usize::from(active) {
            pred_weights.weight_factors[list].luma_weight[index] =
                1i16.checked_shl(u32::from(luma_denom)).unwrap_or(0);
            if read_bool(reader, "H.264 luma_weight_lX_flag")? {
                pred_weights.weight_factors[list].luma_weight[index] =
                    read_i16_se(reader, "H.264 luma_weight_lX")?;
                pred_weights.weight_factors[list].luma_offset[index] =
                    read_i16_se(reader, "H.264 luma_offset_lX")?;
            }

            if sps.chroma_format_idc != 0 {
                for component in 0..2 {
                    pred_weights.weight_factors[list].chroma_weight[index][component] =
                        1i16.checked_shl(u32::from(chroma_denom)).unwrap_or(0);
                }
                if read_bool(reader, "H.264 chroma_weight_lX_flag")? {
                    for component in 0..2 {
                        pred_weights.weight_factors[list].chroma_weight[index][component] =
                            read_i16_se(reader, "H.264 chroma_weight_lX")?;
                        pred_weights.weight_factors[list].chroma_offset[index][component] =
                            read_i16_se(reader, "H.264 chroma_offset_lX")?;
                    }
                }
            }
        }
    }

    Ok(())
}

fn parse_h264_dec_ref_pic_marking(
    reader: &mut EbspBitReader<'_>,
    nal_type: u8,
) -> Result<H264RefPicMarking, String> {
    if nal_type == 5 {
        let _no_output_of_prior_pics_flag =
            read_bool(reader, "H.264 no_output_of_prior_pics_flag")?;
        let long_term_reference_flag = read_bool(reader, "H.264 long_term_reference_flag")?;
        return Ok(H264RefPicMarking {
            idr_long_term: long_term_reference_flag,
            adaptive: false,
            operations: Vec::new(),
        });
    }

    if !read_bool(reader, "H.264 adaptive_ref_pic_marking_mode_flag")? {
        return Ok(H264RefPicMarking::default());
    }
    let mut marking = H264RefPicMarking {
        idr_long_term: false,
        adaptive: true,
        operations: Vec::new(),
    };
    loop {
        let op = read_u32_ue(reader, "H.264 memory_management_control_operation")?;
        match op {
            0 => break,
            1 => {
                let difference_of_pic_nums_minus1 =
                    read_u32_ue(reader, "H.264 difference_of_pic_nums_minus1")?;
                marking
                    .operations
                    .push(H264MemoryManagementControl::ShortTermUnused {
                        difference_of_pic_nums_minus1,
                    });
            }
            2 => {
                let long_term_pic_num = read_u32_ue(reader, "H.264 long_term_pic_num")?;
                marking
                    .operations
                    .push(H264MemoryManagementControl::LongTermUnused { long_term_pic_num });
            }
            3 => {
                let difference_of_pic_nums_minus1 =
                    read_u32_ue(reader, "H.264 difference_of_pic_nums_minus1")?;
                let long_term_frame_idx = read_u32_ue(reader, "H.264 long_term_frame_idx")?;
                marking
                    .operations
                    .push(H264MemoryManagementControl::ShortTermToLongTerm {
                        difference_of_pic_nums_minus1,
                        long_term_frame_idx,
                    });
            }
            4 => {
                let max_long_term_frame_idx_plus1 =
                    read_u32_ue(reader, "H.264 max_long_term_frame_idx_plus1")?;
                marking
                    .operations
                    .push(H264MemoryManagementControl::MaxLongTermFrameIdx {
                        max_long_term_frame_idx_plus1,
                    });
            }
            5 => {
                marking.operations.push(H264MemoryManagementControl::Reset);
            }
            6 => {
                let long_term_frame_idx = read_u32_ue(reader, "H.264 long_term_frame_idx")?;
                marking
                    .operations
                    .push(H264MemoryManagementControl::CurrentToLongTerm {
                        long_term_frame_idx,
                    });
            }
            _ => return Err(String::from("H.264 invalid MMCO")),
        }
    }
    Ok(marking)
}

fn h264_more_rbsp_data(reader: &EbspBitReader<'_>) -> bool {
    let mut clone = reader.clone();
    let Some(first) = clone.read_bit() else {
        return false;
    };
    if first == 0 {
        return true;
    }
    while let Some(bit) = clone.read_bit() {
        if bit != 0 {
            return true;
        }
    }
    false
}

fn read_bool(reader: &mut EbspBitReader<'_>, name: &'static str) -> Result<bool, String> {
    reader
        .read_bit()
        .map(|bit| bit != 0)
        .ok_or_else(|| format!("{name} missing"))
}

fn read_u8_bits(
    reader: &mut EbspBitReader<'_>,
    bits: u8,
    name: &'static str,
) -> Result<u8, String> {
    let value = reader
        .read_bits(bits)
        .ok_or_else(|| format!("{name} missing"))?;
    u8::try_from(value).map_err(|_| format!("{name} overflows u8"))
}

fn read_u16_bits(
    reader: &mut EbspBitReader<'_>,
    bits: u8,
    name: &'static str,
) -> Result<u16, String> {
    let value = reader
        .read_bits(bits)
        .ok_or_else(|| format!("{name} missing"))?;
    u16::try_from(value).map_err(|_| format!("{name} overflows u16"))
}

fn read_u8_ue(reader: &mut EbspBitReader<'_>, name: &'static str) -> Result<u8, String> {
    let value = read_u32_ue(reader, name)?;
    u8::try_from(value).map_err(|_| format!("{name} overflows u8"))
}

fn read_u16_ue(reader: &mut EbspBitReader<'_>, name: &'static str) -> Result<u16, String> {
    let value = read_u32_ue(reader, name)?;
    u16::try_from(value).map_err(|_| format!("{name} overflows u16"))
}

fn read_u32_ue(reader: &mut EbspBitReader<'_>, name: &'static str) -> Result<u32, String> {
    reader.read_ue().ok_or_else(|| format!("{name} missing"))
}

fn read_i8_se(reader: &mut EbspBitReader<'_>, name: &'static str) -> Result<i8, String> {
    let value = reader.read_se().ok_or_else(|| format!("{name} missing"))?;
    i8::try_from(value).map_err(|_| format!("{name} overflows i8"))
}

fn read_i16_se(reader: &mut EbspBitReader<'_>, name: &'static str) -> Result<i16, String> {
    let value = reader.read_se().ok_or_else(|| format!("{name} missing"))?;
    i16::try_from(value).map_err(|_| format!("{name} overflows i16"))
}

fn skip_h264_scaling_list(reader: &mut EbspBitReader<'_>, count: usize) -> Result<(), String> {
    let mut last_scale = 8i32;
    let mut next_scale = 8i32;
    for _ in 0..count {
        if next_scale != 0 {
            let delta_scale = reader
                .read_se()
                .ok_or_else(|| String::from("H.264 scaling list is truncated"))?;
            next_scale = (last_scale + delta_scale + 256) % 256;
        }
        last_scale = if next_scale == 0 {
            last_scale
        } else {
            next_scale
        };
    }
    Ok(())
}

fn is_h264_high_profile(profile_idc: u8) -> bool {
    matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    )
}

fn start_audio_thread(
    source: PlayerAudioSource,
    clock: Arc<AudioClock>,
    controls: Arc<ControlsOverlay>,
    _loop_playback: bool,
) {
    thread::spawn(move || {
        let source = match materialize_audio_source(source) {
            Ok(source) => source,
            Err(err) => {
                clock.mark_unavailable();
                println!("[{}] audio: {}", APP_NAME, err);
                return;
            }
        };

        if let Some(duration_us) = audio_source_duration_us(&source) {
            clock.set_loop_duration_us(duration_us);
            controls.set_media_duration_us(duration_us);
        }

        let mut seek_epoch = controls.current_seek_epoch();
        let mut start_us = controls.current_seek_target_us();
        loop {
            if seek_epoch == 0 && start_us == 0 {
                if !clock.wait_until_video_ready() {
                    return;
                }
            } else if !controls.wait_for_video_seek_ready(seek_epoch) {
                seek_epoch = controls.current_seek_epoch();
                start_us = controls.current_seek_target_us();
                clock.reset_for_replay_audio();
                continue;
            }

            match play_audio_source_sas(&source, &clock, &controls, start_us, seek_epoch) {
                Ok(AudioPlaybackStatus::Completed) => {}
                Ok(AudioPlaybackStatus::Interrupted) => {
                    seek_epoch = controls.current_seek_epoch();
                    start_us = controls.current_seek_target_us();
                    clock.reset_for_replay_audio();
                    continue;
                }
                Err(err) => {
                    clock.mark_unavailable();
                    println!("[{}] audio: {}", APP_NAME, err);
                    return;
                }
            }
            if controls.is_loop_enabled() {
                start_us = 0;
                seek_epoch = controls.current_seek_epoch();
                clock.reset_for_replay_audio();
                continue;
            }
            let replay_epoch = controls.current_replay_epoch();
            let previous_seek_epoch = controls.current_seek_epoch();
            loop {
                if controls.current_seek_epoch() != previous_seek_epoch {
                    seek_epoch = controls.current_seek_epoch();
                    start_us = controls.current_seek_target_us();
                    clock.reset_for_replay_audio();
                    break;
                }
                if controls.current_replay_epoch() != replay_epoch {
                    seek_epoch = controls.current_seek_epoch();
                    start_us = 0;
                    clock.reset_for_replay_audio();
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    });
}

fn materialize_audio_source(source: PlayerAudioSource) -> Result<PlayerAudioSource, String> {
    match source {
        PlayerAudioSource::StreamingMp4Aac {
            path,
            complete_path,
        } => {
            if let Some(complete_path) = complete_path {
                wait_for_marker(&complete_path);
            }
            let data = read_file(&path)?;
            Ok(PlayerAudioSource::Mp4Aac(Arc::new(data)))
        }
        PlayerAudioSource::StreamingMp4AacSocket { socket_path } => {
            let data = read_socket_to_end(&socket_path)?;
            Ok(PlayerAudioSource::Mp4Aac(Arc::new(data)))
        }
        source => Ok(source),
    }
}

fn audio_source_duration_us(source: &PlayerAudioSource) -> Option<u64> {
    match source {
        PlayerAudioSource::Wav(path) => {
            let bytes = read_file(path).ok()?;
            let wav = parse_wav(&bytes).ok()?;
            let frame_bytes =
                usize::from(wav.channels).checked_mul(usize::from(wav.bits_per_sample / 8))?;
            if frame_bytes == 0 || wav.sample_rate == 0 {
                return None;
            }
            let frames = wav.data_len / frame_bytes;
            Some((frames as u128 * 1_000_000 / u128::from(wav.sample_rate)) as u64)
        }
        PlayerAudioSource::Mp4Aac(data) => {
            #[cfg(feature = "mp4-aac")]
            {
                let source = load_mp4_aac_audio_source(data.clone()).ok()?;
                if source.config.sample_rate == 0 {
                    return None;
                }
                let frames = source.samples.len() as u128 * 1024;
                Some((frames * 1_000_000 / u128::from(source.config.sample_rate)) as u64)
            }

            #[cfg(not(feature = "mp4-aac"))]
            {
                let _ = data;
                None
            }
        }
        PlayerAudioSource::StreamingMp4Aac { .. }
        | PlayerAudioSource::StreamingMp4AacSocket { .. } => None,
    }
}

fn play_audio_source_sas(
    source: &PlayerAudioSource,
    clock: &AudioClock,
    controls: &ControlsOverlay,
    start_us: u64,
    seek_epoch: u32,
) -> Result<AudioPlaybackStatus, String> {
    match source {
        PlayerAudioSource::Wav(path) => play_wav_sas(path, clock, controls, start_us, seek_epoch),
        PlayerAudioSource::Mp4Aac(data) => {
            play_mp4_aac_sas(data.clone(), clock, controls, start_us, seek_epoch)
        }
        PlayerAudioSource::StreamingMp4Aac {
            path,
            complete_path,
        } => {
            if let Some(complete_path) = complete_path {
                wait_for_marker(complete_path);
            }
            let data = read_file(path)?;
            play_mp4_aac_sas(Arc::new(data), clock, controls, start_us, seek_epoch)
        }
        PlayerAudioSource::StreamingMp4AacSocket { socket_path } => {
            let data = read_socket_to_end(socket_path)?;
            play_mp4_aac_sas(Arc::new(data), clock, controls, start_us, seek_epoch)
        }
    }
}

fn play_wav_sas(
    path: &str,
    clock: &AudioClock,
    controls: &ControlsOverlay,
    start_us: u64,
    seek_epoch: u32,
) -> Result<AudioPlaybackStatus, String> {
    let bytes = read_file(path)?;
    let wav = parse_wav(&bytes)?;
    if wav.audio_format != 1 || wav.bits_per_sample != 16 {
        return Err(String::from("SAS accepts only PCM S16LE WAV files"));
    }
    let data = &bytes[wav.data_offset..wav.data_offset + wav.data_len];
    play_sas_pcm_s16le(
        data,
        wav.sample_rate,
        wav.channels,
        clock,
        controls,
        start_us,
        seek_epoch,
    )
}

fn play_mp4_aac_sas(
    data: Arc<Vec<u8>>,
    clock: &AudioClock,
    controls: &ControlsOverlay,
    start_us: u64,
    seek_epoch: u32,
) -> Result<AudioPlaybackStatus, String> {
    #[cfg(not(feature = "mp4-aac"))]
    {
        let _ = (data, clock, controls, start_us, seek_epoch);
        Err(String::from("MP4/AAC audio support is not built"))
    }

    #[cfg(feature = "mp4-aac")]
    {
        let source = load_mp4_aac_audio_source(data)?;
        println!(
            "[{}] audio AAC: {} samples rate={} channels={}",
            APP_NAME,
            source.samples.len(),
            source.config.sample_rate,
            source.config.channels
        );
        play_aac_source_sas(&source, clock, controls, start_us, seek_epoch)
    }
}

fn play_sas_pcm_s16le(
    data: &[u8],
    sample_rate: u32,
    channels: u16,
    clock: &AudioClock,
    controls: &ControlsOverlay,
    start_us: u64,
    seek_epoch: u32,
) -> Result<AudioPlaybackStatus, String> {
    if sample_rate == 0 {
        return Err(String::from("audio sample rate is zero"));
    }
    if channels == 0 {
        return Err(String::from("audio channel count is zero"));
    }
    let frame_bytes = channels as usize * 2;
    let total_data_frames = data.len() / frame_bytes;
    let start_frame = (u128::from(start_us) * u128::from(sample_rate) / 1_000_000)
        .min(total_data_frames as u128) as usize;
    let start_offset = start_frame * frame_bytes;
    clock.set_start_position_us(start_us, sample_rate);
    if data.len() < frame_bytes {
        clock.mark_started(sample_rate);
        clock.mark_finished();
        return Ok(AudioPlaybackStatus::Completed);
    }

    let mut writer = SasPcmWriter::new(sample_rate, channels, frame_bytes, clock)?;
    if writer.write_bytes(&data[start_offset..], controls, clock, seek_epoch)? {
        writer.close();
        return Ok(AudioPlaybackStatus::Interrupted);
    }
    writer.drain_close(clock)?;
    clock.advance_base_frames(total_data_frames.saturating_sub(start_frame) as u64);
    clock.mark_finished();
    Ok(AudioPlaybackStatus::Completed)
}

struct SasPcmWriter {
    client: SasClient,
    stream: SasStream,
    frame_bytes: usize,
}

impl SasPcmWriter {
    fn new(
        sample_rate: u32,
        channels: u16,
        frame_bytes: usize,
        clock: &AudioClock,
    ) -> Result<Self, String> {
        let mut client =
            SasClient::connect().map_err(|_| String::from("failed to connect to SAS"))?;

        let period_frames = (sample_rate / 100).max(64);
        let buffer_frames = (sample_rate / 5).max(period_frames * 4);
        let config = StreamConfig {
            format: AUDIO_PCM_FORMAT_S16LE,
            rate: sample_rate,
            channels,
            period_frames,
            buffer_frames,
        };
        let stream = client
            .configure(&config)
            .map_err(|_| String::from("failed to configure SAS stream"))?;

        clock.mark_started(sample_rate);

        Ok(Self {
            client,
            stream,
            frame_bytes,
        })
    }

    fn write_bytes(
        &mut self,
        data: &[u8],
        controls: &ControlsOverlay,
        clock: &AudioClock,
        seek_epoch: u32,
    ) -> Result<bool, String> {
        let total_data_frames = data.len() / self.frame_bytes;
        let mut pos_frame = 0usize;

        while pos_frame < total_data_frames {
            if self.stream.is_closed() {
                return Err(String::from("SAS stream closed"));
            }
            if controls.current_seek_epoch() != seek_epoch {
                return Ok(true);
            }
            clock.update_read_frames(self.stream.read_frames());
            while controls.is_paused() {
                if self.stream.is_closed() {
                    return Err(String::from("SAS stream closed"));
                }
                if controls.current_seek_epoch() != seek_epoch {
                    return Ok(true);
                }
                clock.update_read_frames(self.stream.read_frames());
                thread::sleep(Duration::from_millis(10));
            }
            let frames = self
                .stream
                .writable_frames()
                .min(total_data_frames - pos_frame);
            if frames == 0 {
                if self.stream.is_closed() {
                    return Err(String::from("SAS stream closed"));
                }
                thread::sleep(Duration::from_millis(2));
                continue;
            }

            let data_offset = pos_frame * self.frame_bytes;
            if controls.current_seek_epoch() != seek_epoch {
                return Ok(true);
            }
            let written = self
                .stream
                .write(&data[data_offset..data_offset + frames * self.frame_bytes]);
            pos_frame += written;
            if controls.current_seek_epoch() != seek_epoch {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn drain_close(mut self, clock: &AudioClock) -> Result<(), String> {
        if self.stream.is_closed() {
            return Err(String::from("SAS stream closed"));
        }
        self.client
            .drain()
            .map_err(|_| String::from("SAS drain failed"))?;
        while !self.stream.is_empty() {
            if self.stream.is_closed() {
                return Err(String::from("SAS stream closed"));
            }
            clock.update_read_frames(self.stream.read_frames());
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.client.close();
        Ok(())
    }

    fn close(mut self) {
        self.stream.reset();
        let _ = self.client.close();
    }
}

#[cfg(feature = "mp4-aac")]
fn play_aac_source_sas(
    source: &Mp4AacAudioSource,
    clock: &AudioClock,
    controls: &ControlsOverlay,
    start_us: u64,
    seek_epoch: u32,
) -> Result<AudioPlaybackStatus, String> {
    let frame_bytes = source.config.channels as usize * 2;
    if frame_bytes == 0 {
        return Err(String::from("AAC channel count is zero"));
    }
    let start_frame = u128::from(start_us) * u128::from(source.config.sample_rate) / 1_000_000;
    let start_sample = (start_frame / 1024).min(source.samples.len() as u128) as usize;
    let start_sample_frame = start_sample as u64 * 1024;
    let start_sample_us =
        start_sample_frame.saturating_mul(1_000_000) / u64::from(source.config.sample_rate).max(1);
    clock.set_start_position_us(start_sample_us, source.config.sample_rate);
    let mut writer = SasPcmWriter::new(
        source.config.sample_rate,
        source.config.channels,
        frame_bytes,
        clock,
    )?;
    let mut decoder = create_aac_decoder(source)?;
    let mut samples = Vec::<i16>::new();
    let mut bytes = Vec::<u8>::new();
    let mut pts = start_sample_frame as i64;
    let mut written_frames = start_sample_frame;
    for range in source.samples.iter().skip(start_sample) {
        if controls.current_seek_epoch() != seek_epoch {
            writer.close();
            return Ok(AudioPlaybackStatus::Interrupted);
        }
        let sample_end = range
            .offset
            .checked_add(range.size)
            .ok_or_else(|| String::from("MP4 AAC sample range overflow"))?;
        let sample = source
            .data
            .get(range.offset..sample_end)
            .ok_or_else(|| String::from("MP4 AAC sample range is invalid"))?;
        let packet = PacketRef::new(
            0,
            AudioTimestamp::new(pts),
            AudioDuration::new(1024),
            sample,
        );
        pts = pts.saturating_add(1024);
        samples.clear();
        let decoded = decoder
            .decode_ref(&packet)
            .map_err(|_| String::from("AAC frame decode failed"))?;
        decoded.copy_to_vec_interleaved::<i16>(&mut samples);
        bytes.clear();
        bytes.reserve(samples.len() * 2);
        for sample in &samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        written_frames =
            written_frames.saturating_add((samples.len() / source.config.channels as usize) as u64);
        if writer.write_bytes(&bytes, controls, clock, seek_epoch)? {
            writer.close();
            return Ok(AudioPlaybackStatus::Interrupted);
        }
    }
    writer.drain_close(clock)?;
    clock.advance_base_frames(written_frames.saturating_sub(start_sample_frame));
    clock.mark_finished();
    Ok(AudioPlaybackStatus::Completed)
}

#[cfg(feature = "mp4-aac")]
fn create_aac_decoder(source: &Mp4AacAudioSource) -> Result<AacDecoder, String> {
    let channels = match source.config.channels {
        1 => layouts::CHANNEL_LAYOUT_MONO,
        2 => layouts::CHANNEL_LAYOUT_STEREO,
        _ => return Err(String::from("AAC decoder supports only mono/stereo output")),
    };
    let mut params = AudioCodecParameters::new();
    params
        .for_codec(CODEC_ID_AAC)
        .with_sample_rate(source.config.sample_rate)
        .with_channels(channels)
        .with_extra_data(
            source
                .config
                .audio_specific_config
                .clone()
                .into_boxed_slice(),
        );
    AacDecoder::try_new(&params, &AudioDecoderOptions::default())
        .map_err(|_| String::from("AAC decoder initialization failed"))
}

enum PaceDecision {
    Present { sync_time_us: Option<u64> },
    Drop { sync_time_us: Option<u64> },
    Stale,
}

fn pace_frame(
    controls: &ControlsOverlay,
    presentation_time_us: u64,
    seek_epoch: u32,
    clock: Option<&AudioClock>,
) -> PaceDecision {
    if !wait_while_paused_epoch(controls, seek_epoch) {
        return PaceDecision::Stale;
    }
    if controls.current_seek_epoch() != seek_epoch {
        return PaceDecision::Stale;
    }
    let mut sync_time_us = None;
    if let Some(clock) = clock {
        if controls.is_video_ready_for_seek(seek_epoch) {
            let mut logged_audio_wait = false;
            let mut audio_wait_ms = 0u64;
            loop {
                if controls.current_seek_epoch() != seek_epoch {
                    return PaceDecision::Stale;
                }
                if !wait_while_paused_epoch(controls, seek_epoch) {
                    return PaceDecision::Stale;
                }
                let Some(audio_time_us) = clock.elapsed_us() else {
                    if clock.is_unavailable() {
                        thread::sleep(Duration::from_millis(FRAME_INTERVAL_MS));
                        break;
                    }
                    if !logged_audio_wait {
                        println!("[{}] waiting for audio clock", APP_NAME);
                        logged_audio_wait = true;
                    }
                    thread::sleep(Duration::from_millis(1));
                    audio_wait_ms += 1;
                    if audio_wait_ms >= AUDIO_CLOCK_START_TIMEOUT_MS {
                        println!("[{}] audio clock start timed out", APP_NAME);
                        clock.mark_unavailable();
                        break;
                    }
                    continue;
                };
                sync_time_us = Some(audio_time_us);
                if audio_time_us > presentation_time_us.saturating_add(LATE_VIDEO_DROP_THRESHOLD_US)
                {
                    return PaceDecision::Drop { sync_time_us };
                }
                if audio_time_us >= presentation_time_us {
                    break;
                }
                if clock.is_finished() {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
        }
    } else {
        thread::sleep(Duration::from_millis(FRAME_INTERVAL_MS));
        if controls.current_seek_epoch() != seek_epoch {
            return PaceDecision::Stale;
        }
    }
    if controls.current_seek_epoch() != seek_epoch {
        return PaceDecision::Stale;
    }
    PaceDecision::Present { sync_time_us }
}

fn publish_seek_preview(
    frame_store: &VideoFrameStore,
    paint_signal: &PaintSignal,
    controls: &ControlsOverlay,
    frame: DecodedVideoFrame,
    display_index: usize,
    total_frames: u32,
    presentation_time_us: u64,
    seek_epoch: u32,
) -> Result<bool, String> {
    if controls.current_seek_epoch() != seek_epoch {
        return Ok(false);
    }
    publish_frame(
        frame_store,
        paint_signal,
        controls,
        frame,
        display_index,
        total_frames,
    )?;
    controls.record_preview_frame(presentation_time_us);
    Ok(controls.current_seek_epoch() == seek_epoch)
}

fn publish_frame(
    frame_store: &VideoFrameStore,
    paint_signal: &PaintSignal,
    controls: &ControlsOverlay,
    frame: DecodedVideoFrame,
    display_index: usize,
    total_frames: u32,
) -> Result<(), String> {
    wait_while_paused(controls);
    let current_frame = (display_index + 1).min(u32::MAX as usize) as u32;
    match frame {
        DecodedVideoFrame::Software(frame) => {
            frame_store.update_from_frame(&frame, current_frame, total_frames);
        }
        DecodedVideoFrame::Hardware(frame) => {
            frame_store.update_from_nv12(&frame, current_frame, total_frames)?;
        }
    }
    paint_signal.notify();
    Ok(())
}

fn wait_while_paused(controls: &ControlsOverlay) {
    while controls.is_paused() {
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_while_paused_epoch(controls: &ControlsOverlay, seek_epoch: u32) -> bool {
    while controls.is_paused() {
        if controls.current_seek_epoch() != seek_epoch {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    controls.current_seek_epoch() == seek_epoch
}

fn wait_for_replay_request(controls: &ControlsOverlay) -> Option<u64> {
    controls.mark_finished();
    wait_for_replay_or_seek_request(controls)
}

fn wait_for_replay_or_seek_request(controls: &ControlsOverlay) -> Option<u64> {
    let replay_epoch = controls.current_replay_epoch();
    let seek_epoch = controls.current_seek_epoch();
    loop {
        if controls.current_replay_epoch() != replay_epoch {
            return None;
        }
        if controls.current_seek_epoch() != seek_epoch {
            return Some(controls.current_seek_target_us());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn read_file(path: &str) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|_| format!("open failed: {path}"))?;
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];

    loop {
        let read = file.read(&mut buffer).map_err(|_| format!("read failed"))?;
        if read == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..read]);
    }

    Ok(data)
}

fn connect_local_socket_blocking(path: &str) -> Result<Socket, String> {
    for _ in 0..400 {
        let socket = Socket::new().map_err(|_| format!("failed to create local socket: {path}"))?;
        match socket.connect(path) {
            Ok(()) => return Ok(socket),
            Err(_) => {
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    Err(format!("timed out connecting local socket: {path}"))
}

struct StreamSocketState {
    data: Vec<u8>,
    complete: bool,
    error: Option<String>,
}

fn start_stream_socket_reader(path: String) -> Arc<Mutex<StreamSocketState>> {
    let state = Arc::new(Mutex::new(StreamSocketState {
        data: Vec::new(),
        complete: false,
        error: None,
    }));
    let reader_state = state.clone();
    thread::spawn(move || {
        if let Err(error) = read_stream_socket_into_state(&path, &reader_state) {
            let mut state = reader_state.lock();
            state.error = Some(error);
            state.complete = true;
        }
    });
    state
}

fn read_stream_socket_into_state(
    path: &str,
    state: &Arc<Mutex<StreamSocketState>>,
) -> Result<(), String> {
    let mut socket = connect_local_socket_blocking(path)?;
    socket
        .set_nonblocking(true)
        .map_err(|_| format!("failed to set stream socket nonblocking: {path}"))?;
    let mut buffer = [0u8; 32 * 1024];

    loop {
        match socket.read(&mut buffer) {
            Ok(0) => {
                let mut state = state.lock();
                state.complete = true;
                break;
            }
            Ok(read) => {
                let mut state = state.lock();
                state.data.extend_from_slice(&buffer[..read]);
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
            }
            Err(err) if err.kind() == ErrorKind::UnexpectedEof => {
                let mut state = state.lock();
                state.complete = true;
                break;
            }
            Err(err) => return Err(format!("stream socket read failed: {err}")),
        }
    }

    Ok(())
}

fn read_socket_to_end(path: &str) -> Result<Vec<u8>, String> {
    let mut socket = connect_local_socket_blocking(path)?;
    let mut data = Vec::new();
    let mut buffer = [0u8; 32 * 1024];

    loop {
        match socket.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => data.extend_from_slice(&buffer[..read]),
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
            }
            Err(err) if err.kind() == ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(format!("audio stream socket read failed: {err}")),
        }
    }

    Ok(data)
}

fn append_growing_file(path: &str, data: &mut Vec<u8>) -> Result<usize, String> {
    let mut file = File::open(path).map_err(|_| format!("open failed: {path}"))?;
    let available_len = file
        .seek(SeekFrom::End(0))
        .map_err(|_| format!("seek failed: {path}"))? as usize;
    if available_len <= data.len() {
        return Ok(0);
    }
    file.seek(SeekFrom::Start(data.len() as u64))
        .map_err(|_| format!("seek failed: {path}"))?;
    let before = data.len();
    let mut buffer = [0u8; 16 * 1024];
    let mut remaining = available_len - data.len();

    while remaining > 0 {
        let read_len = remaining.min(buffer.len());
        let read = file
            .read(&mut buffer[..read_len])
            .map_err(|_| format!("read failed"))?;
        if read == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..read]);
        remaining -= read;
    }

    Ok(data.len().saturating_sub(before))
}

fn marker_exists(path: &str) -> bool {
    File::open(path).is_ok()
}

fn wait_for_marker(path: &str) {
    while !marker_exists(path) {
        thread::sleep(Duration::from_millis(STREAM_POLL_INTERVAL_MS));
    }
}

fn read_exact_file(file: &mut File, out: &mut [u8]) -> Result<(), String> {
    let mut read = 0usize;
    let mut empty_reads = 0usize;
    while read < out.len() {
        let n = file
            .read(&mut out[read..])
            .map_err(|_| String::from("hardware decoder read failed"))?;
        if n == 0 {
            empty_reads += 1;
            if empty_reads > 10_000 {
                return Err(String::from(
                    "hardware decoder timed out before frame was complete",
                ));
            }
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        empty_reads = 0;
        read += n;
    }
    Ok(())
}

struct WavInfo {
    audio_format: u16,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    data_offset: usize,
    data_len: usize,
}

fn parse_wav(bytes: &[u8]) -> Result<WavInfo, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(String::from("not a RIFF/WAVE file"));
    }

    let mut cursor = 12usize;
    let mut audio_format = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u16;
    let mut data_offset = None;
    let mut data_len = 0usize;

    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let len = read_u32_le(&bytes[cursor + 4..cursor + 8]) as usize;
        cursor += 8;
        if cursor + len > bytes.len() {
            return Err(String::from("truncated WAV chunk"));
        }

        if id == b"fmt " {
            if len < 16 {
                return Err(String::from("invalid WAV fmt chunk"));
            }
            audio_format = read_u16_le(&bytes[cursor..cursor + 2]);
            channels = read_u16_le(&bytes[cursor + 2..cursor + 4]);
            sample_rate = read_u32_le(&bytes[cursor + 4..cursor + 8]);
            bits_per_sample = read_u16_le(&bytes[cursor + 14..cursor + 16]);
        } else if id == b"data" {
            data_offset = Some(cursor);
            data_len = len;
        }

        cursor += (len + 1) & !1;
    }

    Ok(WavInfo {
        audio_format,
        channels,
        sample_rate,
        bits_per_sample,
        data_offset: data_offset.ok_or_else(|| String::from("WAV data chunk not found"))?,
        data_len,
    })
}

fn read_u16_le(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn read_u32_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn read_u16_be(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

fn read_u32_be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn read_i32_be(bytes: &[u8]) -> i32 {
    i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn read_u64_be(bytes: &[u8]) -> u64 {
    u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

fn yuv420_to_bgra(frame: &Frame, pixels: &mut [u8]) {
    yuv420_to_bgra_simd(frame, pixels);
}

fn nv12_to_bgra(width: u32, height: u32, nv12: &[u8], pixels: &mut [u8]) {
    nv12_to_bgra_simd(width, height, nv12, pixels);
}

fn nv12_to_bgra_simd(width: u32, height: u32, nv12: &[u8], pixels: &mut [u8]) {
    const LANES: usize = 8;

    let width = width as usize;
    let height = height as usize;
    let y_plane_len = width * height;
    let uv_plane = &nv12[y_plane_len..];

    for y in 0..height {
        let y_row = y * width;
        let uv_row = (y / 2) * width;
        let mut x = 0usize;

        while x + LANES <= width {
            let y_values =
                Simd::<u8, LANES>::from_slice(&nv12[y_row + x..y_row + x + LANES]).cast::<i32>();
            let uv_base = uv_row + (x & !1);
            let u_values = Simd::<i32, LANES>::from_array([
                uv_plane[uv_base] as i32,
                uv_plane[uv_base] as i32,
                uv_plane[uv_base + 2] as i32,
                uv_plane[uv_base + 2] as i32,
                uv_plane[uv_base + 4] as i32,
                uv_plane[uv_base + 4] as i32,
                uv_plane[uv_base + 6] as i32,
                uv_plane[uv_base + 6] as i32,
            ]);
            let v_values = Simd::<i32, LANES>::from_array([
                uv_plane[uv_base + 1] as i32,
                uv_plane[uv_base + 1] as i32,
                uv_plane[uv_base + 3] as i32,
                uv_plane[uv_base + 3] as i32,
                uv_plane[uv_base + 5] as i32,
                uv_plane[uv_base + 5] as i32,
                uv_plane[uv_base + 7] as i32,
                uv_plane[uv_base + 7] as i32,
            ]);

            let (r, g, b) = yuv_to_rgb_simd(y_values, u_values, v_values);
            store_bgra8(pixels, (y_row + x) * 4, r, g, b);

            x += LANES;
        }

        while x < width {
            let y_value = nv12[y_row + x] as i32;
            let uv_offset = uv_row + (x & !1);
            let u_value = uv_plane[uv_offset] as i32;
            let v_value = uv_plane[uv_offset + 1] as i32;
            let (r, g, b) = yuv_to_rgb(y_value, u_value, v_value);
            let offset = (y_row + x) * 4;
            pixels[offset] = b;
            pixels[offset + 1] = g;
            pixels[offset + 2] = r;
            pixels[offset + 3] = 255;
            x += 1;
        }
    }
}

fn yuv420_to_bgra_simd(frame: &Frame, pixels: &mut [u8]) {
    const LANES: usize = 8;

    let width = frame.width as usize;
    let height = frame.height as usize;
    let chroma_width = width / 2;

    for y in 0..height {
        let y_row = y * width;
        let uv_row = (y / 2) * chroma_width;
        let mut x = 0usize;

        while x + LANES <= width {
            let y_values =
                Simd::<u8, LANES>::from_slice(&frame.y[y_row + x..y_row + x + LANES]).cast::<i32>();
            let u_base = uv_row + x / 2;
            let v_base = uv_row + x / 2;
            let u_values = Simd::<i32, LANES>::from_array([
                frame.u[u_base] as i32,
                frame.u[u_base] as i32,
                frame.u[u_base + 1] as i32,
                frame.u[u_base + 1] as i32,
                frame.u[u_base + 2] as i32,
                frame.u[u_base + 2] as i32,
                frame.u[u_base + 3] as i32,
                frame.u[u_base + 3] as i32,
            ]);
            let v_values = Simd::<i32, LANES>::from_array([
                frame.v[v_base] as i32,
                frame.v[v_base] as i32,
                frame.v[v_base + 1] as i32,
                frame.v[v_base + 1] as i32,
                frame.v[v_base + 2] as i32,
                frame.v[v_base + 2] as i32,
                frame.v[v_base + 3] as i32,
                frame.v[v_base + 3] as i32,
            ]);

            let (r, g, b) = yuv_to_rgb_simd(y_values, u_values, v_values);
            store_bgra8(pixels, (y_row + x) * 4, r, g, b);

            x += LANES;
        }

        while x < width {
            let y_value = frame.y[y_row + x] as i32;
            let u_value = frame.u[uv_row + x / 2] as i32;
            let v_value = frame.v[uv_row + x / 2] as i32;
            let (r, g, b) = yuv_to_rgb(y_value, u_value, v_value);
            let offset = (y_row + x) * 4;
            pixels[offset] = b;
            pixels[offset + 1] = g;
            pixels[offset + 2] = r;
            pixels[offset + 3] = 255;
            x += 1;
        }
    }
}

fn store_bgra8(pixels: &mut [u8], offset: usize, r: Simd<u8, 8>, g: Simd<u8, 8>, b: Simd<u8, 8>) {
    let packed = b.cast::<u32>()
        | (g.cast::<u32>() << Simd::splat(8))
        | (r.cast::<u32>() << Simd::splat(16))
        | Simd::splat(0xff00_0000);
    let packed = packed.to_array();

    for (lane, pixel) in packed.iter().enumerate() {
        // SAFETY: callers pass an offset for 8 BGRA pixels inside `pixels`.
        // `pixels` is byte-aligned, so each packed pixel is written unaligned.
        unsafe {
            (pixels.as_mut_ptr().add(offset + lane * 4) as *mut u32).write_unaligned(*pixel);
        }
    }
}

fn yuv_to_rgb_simd(
    y: Simd<i32, 8>,
    u: Simd<i32, 8>,
    v: Simd<i32, 8>,
) -> (Simd<u8, 8>, Simd<u8, 8>, Simd<u8, 8>) {
    let c = (y - Simd::splat(16)).simd_max(Simd::splat(0));
    let d = u - Simd::splat(128);
    let e = v - Simd::splat(128);
    let rounding = Simd::splat(128);

    let r = (Simd::splat(298) * c + Simd::splat(409) * e + rounding) >> Simd::splat(8);
    let g = (Simd::splat(298) * c - Simd::splat(100) * d - Simd::splat(208) * e + rounding)
        >> Simd::splat(8);
    let b = (Simd::splat(298) * c + Simd::splat(516) * d + rounding) >> Simd::splat(8);

    (clamp_u8_simd(r), clamp_u8_simd(g), clamp_u8_simd(b))
}

fn clamp_u8_simd(value: Simd<i32, 8>) -> Simd<u8, 8> {
    value
        .simd_clamp(Simd::splat(0), Simd::splat(255))
        .cast::<u8>()
}

fn yuv_to_rgb(y: i32, u: i32, v: i32) -> (u8, u8, u8) {
    let c = (y - 16).max(0);
    let d = u - 128;
    let e = v - 128;
    let r = (298 * c + 409 * e + 128) >> 8;
    let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
    let b = (298 * c + 516 * d + 128) >> 8;
    (clamp_u8(r), clamp_u8(g), clamp_u8(b))
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

fn draw_video_frame(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    frame_store: &VideoFrameStore,
    controls: &ControlsOverlay,
) {
    let ui_scale = UiScale::current();
    let logical_canvas_width = ui_scale.logical_len(canvas_width);
    let logical_canvas_height = ui_scale.logical_len(canvas_height);
    controls.update_canvas_size(logical_canvas_width, logical_canvas_height);
    let frame = frame_store.data.lock();
    let frame_width = frame.width;
    let frame_height = frame.height;

    if frame_width == 0 || frame_height == 0 || canvas_width == 0 || canvas_height == 0 {
        fill_bgra(buffer, [0, 0, 0, 255]);
        return;
    }

    let (out_width, out_height) = fit_size(frame_width, frame_height, canvas_width, canvas_height);
    let x_offset = (canvas_width - out_width) / 2;
    let y_offset = (canvas_height - out_height) / 2;
    let source = frame.pixels.as_slice();
    let canvas_stride = canvas_width as usize * 4;
    let source_stride = frame_width as usize * 4;

    if out_width == frame_width
        && out_height == frame_height
        && out_width == canvas_width
        && out_height == canvas_height
    {
        let copy_len = buffer.len().min(source.len());
        buffer[..copy_len].copy_from_slice(&source[..copy_len]);
        draw_seek_bar(
            buffer,
            canvas_width,
            canvas_height,
            logical_canvas_width,
            logical_canvas_height,
            &frame,
            controls,
            ui_scale,
        );
        draw_debug_overlay(
            buffer,
            canvas_width,
            canvas_height,
            logical_canvas_width,
            logical_canvas_height,
            &frame,
            controls,
            ui_scale,
        );
        return;
    }

    let top_bytes = y_offset as usize * canvas_stride;
    fill_bgra(&mut buffer[..top_bytes], [0, 0, 0, 255]);

    let bottom_start = (y_offset + out_height) as usize * canvas_stride;
    fill_bgra(&mut buffer[bottom_start..], [0, 0, 0, 255]);

    for y in 0..out_height {
        let src_y = (u64::from(y) * u64::from(frame_height) / u64::from(out_height)) as usize;
        let dst_y = (y + y_offset) as usize;
        let row_start = dst_y * canvas_stride;

        if x_offset != 0 {
            let left_end = row_start + x_offset as usize * 4;
            fill_bgra(&mut buffer[row_start..left_end], [0, 0, 0, 255]);

            let right_start = row_start + (x_offset + out_width) as usize * 4;
            let row_end = row_start + canvas_stride;
            fill_bgra(&mut buffer[right_start..row_end], [0, 0, 0, 255]);
        }

        let dst = row_start + x_offset as usize * 4;
        if out_width == frame_width && out_height == frame_height {
            let src = src_y * source_stride;
            let bytes = out_width as usize * 4;
            buffer[dst..dst + bytes].copy_from_slice(&source[src..src + bytes]);
        } else {
            for x in 0..out_width {
                let src_x = (u64::from(x) * u64::from(frame_width) / u64::from(out_width)) as usize;
                let src = src_y * source_stride + src_x * 4;
                let dst = dst + x as usize * 4;
                buffer[dst..dst + 4].copy_from_slice(&source[src..src + 4]);
            }
        }
    }

    draw_seek_bar(
        buffer,
        canvas_width,
        canvas_height,
        logical_canvas_width,
        logical_canvas_height,
        &frame,
        controls,
        ui_scale,
    );
    draw_debug_overlay(
        buffer,
        canvas_width,
        canvas_height,
        logical_canvas_width,
        logical_canvas_height,
        &frame,
        controls,
        ui_scale,
    );
}

fn handle_canvas_event(
    event: &Event,
    controls: &ControlsOverlay,
    paint_signal: &PaintSignal,
) -> bool {
    match event {
        Event::Mouse(MouseEvent::Entered { .. }) => controls.show_for_mouse_activity(),
        Event::Mouse(MouseEvent::Moved { x, y }) => {
            controls.show_for_mouse_activity();
            if controls.is_scrubbing() {
                let target_us = seek_target_from_track_x(controls, *x);
                controls.request_seek_to_us(target_us);
                paint_signal.notify();
                return true;
            }
            let _ = y;
            false
        }
        Event::Mouse(MouseEvent::Exited { .. }) => controls.hide(),
        Event::Mouse(MouseEvent::ButtonPressed {
            button: MouseButton::Left,
            x,
            y,
            ..
        }) => {
            controls.show_for_mouse_activity();
            if seekbar_hit_region_contains(controls, *x, *y) {
                controls.set_scrubbing(true);
                let target_us = seek_target_from_track_x(controls, *x);
                controls.request_seek_to_us(target_us);
                paint_signal.notify();
                true
            } else {
                false
            }
        }
        Event::Mouse(MouseEvent::ButtonReleased {
            button: MouseButton::Left,
            x,
            y,
            ..
        }) => {
            controls.show_for_mouse_activity();
            if controls.is_scrubbing() {
                controls.set_scrubbing(false);
                let target_us = seek_target_from_track_x(controls, *x);
                controls.request_seek_to_us(target_us);
                paint_signal.notify();
                return true;
            }
            controls.set_scrubbing(false);
            if controls.play_pause_button_contains(*x, *y) {
                if controls.is_finished() {
                    controls.request_replay();
                } else {
                    controls.toggle_paused();
                }
                paint_signal.notify();
                true
            } else if controls.loop_button_contains(*x, *y) {
                controls.toggle_loop();
                paint_signal.notify();
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

fn handle_key_event(
    event: KeyEvent,
    controls: &ControlsOverlay,
    paint_signal: &PaintSignal,
) -> bool {
    match event {
        KeyEvent::Pressed {
            keycode: KeyCode::Space,
            ..
        } => {
            activate_play_pause(controls);
            paint_signal.notify();
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Escape,
            ..
        }
        | KeyEvent::Char { c: 'q' | 'Q' } => {
            exit(0);
        }
        KeyEvent::Char { c: 'p' | 'P' } => {
            activate_play_pause(controls);
            paint_signal.notify();
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Left,
            ..
        } => {
            controls.request_relative_seek_ms(-5_000);
            paint_signal.notify();
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Right,
            ..
        } => {
            controls.request_relative_seek_ms(5_000);
            paint_signal.notify();
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Down,
            ..
        } => {
            controls.request_relative_seek_ms(-60_000);
            paint_signal.notify();
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Up,
            ..
        } => {
            controls.request_relative_seek_ms(60_000);
            paint_signal.notify();
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::Home,
            ..
        } => {
            controls.request_seek_to_us(0);
            paint_signal.notify();
            true
        }
        KeyEvent::Pressed {
            keycode: KeyCode::End,
            ..
        } => {
            controls.request_seek_to_us(controls.media_duration_us().saturating_sub(1));
            paint_signal.notify();
            true
        }
        KeyEvent::Char { c: 'd' | 'D' } => {
            controls.toggle_debug();
            paint_signal.notify();
            true
        }
        KeyEvent::Char { c: 'l' | 'L' } => {
            controls.toggle_loop();
            paint_signal.notify();
            true
        }
        _ => false,
    }
}

fn activate_play_pause(controls: &ControlsOverlay) {
    if controls.is_finished() {
        controls.request_replay();
    } else {
        controls.toggle_paused();
    }
}

fn seekbar_hit_region_contains(controls: &ControlsOverlay, x: i32, y: i32) -> bool {
    let Ok(x) = u32::try_from(x) else {
        return false;
    };
    let Ok(y) = u32::try_from(y) else {
        return false;
    };
    let width = controls.canvas_width.load(Ordering::Acquire);
    let height = controls.canvas_height.load(Ordering::Acquire);
    if width < 180 || controls.media_duration_us() == 0 {
        return false;
    }
    let track_x = SEEK_TRACK_LEFT_INSET;
    let right_inset = SEEK_TRACK_RIGHT_INSET.min(width / 8);
    let track_width = width.saturating_sub(track_x + right_inset).max(1);
    let track_y = height.saturating_sub(SEEK_TRACK_BOTTOM_INSET);
    let hit_x0 = track_x.saturating_sub(SEEK_TRACK_HIT_INSET);
    let hit_x1 = track_x
        .saturating_add(track_width)
        .saturating_add(SEEK_TRACK_HIT_INSET);
    let hit_y0 = track_y.saturating_sub(SEEK_TRACK_HIT_INSET.max(SEEK_KNOB_HEIGHT));
    let hit_y1 = track_y.saturating_add(SEEK_TRACK_HIT_INSET.max(SEEK_KNOB_HEIGHT));
    x >= hit_x0 && x <= hit_x1 && y >= hit_y0 && y <= hit_y1
}

fn seek_target_from_track_x(controls: &ControlsOverlay, x: i32) -> u64 {
    let width = controls.canvas_width.load(Ordering::Acquire);
    let track_x = SEEK_TRACK_LEFT_INSET;
    let right_inset = SEEK_TRACK_RIGHT_INSET.min(width / 8);
    let track_width = width.saturating_sub(track_x + right_inset).max(1);
    let duration_us = controls.media_duration_us();
    if duration_us == 0 {
        return 0;
    }
    let relative_x = if x <= track_x as i32 {
        0
    } else {
        u32::try_from(x - track_x as i32)
            .unwrap_or(u32::MAX)
            .min(track_width)
    };
    u64::from(relative_x).saturating_mul(duration_us) / u64::from(track_width)
}

fn draw_debug_overlay(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    logical_canvas_width: u32,
    logical_canvas_height: u32,
    frame: &VideoFrameData,
    controls: &ControlsOverlay,
    ui_scale: UiScale,
) {
    if !controls.is_debug_visible() || logical_canvas_width < 180 || logical_canvas_height < 80 {
        return;
    }

    let presented = controls.presented_frames.load(Ordering::Acquire);
    let dropped = controls.dropped_frames.load(Ordering::Acquire);
    let last_video_pts_us = controls.last_video_pts_us.load(Ordering::Acquire);
    let lag_ms = controls.last_lag_us.load(Ordering::Acquire) / 1_000;
    let total_frames = frame.total_frames.max(frame.current_frame).max(1);
    let current_frame = frame.current_frame.min(total_frames);
    let fps_x10 = controls.fps_display_x10.load(Ordering::Acquire);

    let panel_width = logical_canvas_width.min(360);
    let panel_height = 86u32.min(logical_canvas_height);
    blend_rect_scaled(
        buffer,
        canvas_width,
        canvas_height,
        8,
        8,
        panel_width.saturating_sub(16),
        panel_height.saturating_sub(16),
        [0, 0, 0, 176],
        ui_scale,
    );

    let line1 = format!(
        "fps {}.{}  shown {}  drop {}",
        fps_x10 / 10,
        fps_x10 % 10,
        presented,
        dropped
    );
    let line2 = format!(
        "frame {}/{}  pts {}ms",
        current_frame,
        total_frames,
        last_video_pts_us / 1_000
    );
    let line3 = format!("lag {}ms  {}x{}", lag_ms, frame.width, frame.height);

    let mut canvas = Canvas::new(buffer, canvas_width, canvas_height);
    let text_color = Color::rgb(232, 236, 240);
    canvas.draw_text_sized(
        ui_scale.physical_i32(18),
        ui_scale.physical_i32(18),
        &line1,
        text_color,
        ui_scale.physical_font(14.0),
    );
    canvas.draw_text_sized(
        ui_scale.physical_i32(18),
        ui_scale.physical_i32(38),
        &line2,
        text_color,
        ui_scale.physical_font(14.0),
    );
    canvas.draw_text_sized(
        ui_scale.physical_i32(18),
        ui_scale.physical_i32(58),
        &line3,
        text_color,
        ui_scale.physical_font(14.0),
    );
}

fn draw_seek_bar(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    logical_canvas_width: u32,
    logical_canvas_height: u32,
    frame: &VideoFrameData,
    controls: &ControlsOverlay,
    ui_scale: UiScale,
) {
    if !controls.is_visible() {
        return;
    }

    let Some((button_x, button_y)) =
        play_pause_button_origin(logical_canvas_width, logical_canvas_height)
    else {
        return;
    };
    let panel_y = logical_canvas_height.saturating_sub(CONTROLS_PANEL_HEIGHT);

    blend_rect_scaled(
        buffer,
        canvas_width,
        canvas_height,
        0,
        panel_y,
        logical_canvas_width,
        logical_canvas_height - panel_y,
        [0, 0, 0, 112],
        ui_scale,
    );

    if logical_canvas_width < 180 {
        draw_play_pause_button(
            buffer,
            canvas_width,
            canvas_height,
            button_x,
            button_y,
            controls.is_paused() || controls.is_finished(),
            ui_scale,
        );
        return;
    }

    let track_x = SEEK_TRACK_LEFT_INSET;
    let right_inset = SEEK_TRACK_RIGHT_INSET.min(logical_canvas_width / 8);
    let track_width = logical_canvas_width
        .saturating_sub(track_x + right_inset)
        .max(1);
    let track_height = SEEK_TRACK_HEIGHT;
    let track_y = logical_canvas_height.saturating_sub(SEEK_TRACK_BOTTOM_INSET);
    let duration_us = controls.media_duration_us();
    let (buffered_width, progress_width) = if duration_us != 0 {
        let buffered_us = controls
            .buffered_position_us
            .load(Ordering::Acquire)
            .min(duration_us);
        let position_us = controls
            .desired_position_us
            .load(Ordering::Acquire)
            .min(duration_us);
        (
            (u128::from(track_width) * u128::from(buffered_us) / u128::from(duration_us)) as u32,
            (u128::from(track_width) * u128::from(position_us) / u128::from(duration_us)) as u32,
        )
    } else {
        let total_frames = frame.total_frames.max(frame.current_frame).max(1);
        let current_frame = frame.current_frame.min(total_frames);
        let progress_width =
            (u64::from(track_width) * u64::from(current_frame) / u64::from(total_frames)) as u32;
        (track_width, progress_width)
    };
    let knob_x = track_x + progress_width.saturating_sub(1).min(track_width - 1);

    blend_rect_scaled(
        buffer,
        canvas_width,
        canvas_height,
        track_x,
        track_y,
        track_width,
        track_height,
        [88, 88, 88, 192],
        ui_scale,
    );
    draw_rect_scaled(
        buffer,
        canvas_width,
        canvas_height,
        track_x,
        track_y,
        buffered_width,
        track_height,
        [150, 150, 150, 220],
        ui_scale,
    );
    draw_rect_scaled(
        buffer,
        canvas_width,
        canvas_height,
        track_x,
        track_y,
        progress_width,
        track_height,
        [238, 238, 238, 255],
        ui_scale,
    );
    draw_rect_scaled(
        buffer,
        canvas_width,
        canvas_height,
        knob_x.saturating_sub(SEEK_KNOB_WIDTH / 2),
        track_y.saturating_sub((SEEK_KNOB_HEIGHT - SEEK_TRACK_HEIGHT) / 2),
        SEEK_KNOB_WIDTH,
        SEEK_KNOB_HEIGHT,
        [255, 255, 255, 255],
        ui_scale,
    );
    draw_play_pause_button(
        buffer,
        canvas_width,
        canvas_height,
        button_x,
        button_y,
        controls.is_paused() || controls.is_finished(),
        ui_scale,
    );
    if let Some((loop_x, loop_y)) = loop_button_origin(logical_canvas_width, logical_canvas_height)
    {
        draw_loop_button(
            buffer,
            canvas_width,
            canvas_height,
            loop_x,
            loop_y,
            controls.is_loop_enabled(),
            ui_scale,
        );
    }
}

fn play_pause_button_origin(canvas_width: u32, canvas_height: u32) -> Option<(u32, u32)> {
    if canvas_width < CONTROLS_MIN_WIDTH || canvas_height < CONTROLS_MIN_HEIGHT {
        return None;
    }

    let panel_y = canvas_height.saturating_sub(CONTROLS_PANEL_HEIGHT);
    let button_x = PLAY_BUTTON_LEFT_INSET.min(canvas_width.saturating_sub(PLAY_BUTTON_SIZE));
    let button_y = panel_y + PLAY_BUTTON_TOP_INSET;
    Some((button_x, button_y))
}

fn loop_button_origin(canvas_width: u32, canvas_height: u32) -> Option<(u32, u32)> {
    if canvas_width < CONTROLS_MIN_WIDTH + LOOP_BUTTON_WIDTH || canvas_height < CONTROLS_MIN_HEIGHT
    {
        return None;
    }

    let panel_y = canvas_height.saturating_sub(CONTROLS_PANEL_HEIGHT);
    let button_x = LOOP_BUTTON_LEFT_INSET.min(canvas_width.saturating_sub(LOOP_BUTTON_WIDTH));
    let button_y = panel_y + PLAY_BUTTON_TOP_INSET;
    Some((button_x, button_y))
}

fn draw_loop_button(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: u32,
    y: u32,
    enabled: bool,
    ui_scale: UiScale,
) {
    let fill = if enabled {
        [52, 132, 220, 184]
    } else {
        [0, 0, 0, 96]
    };
    blend_rect_scaled(
        buffer,
        canvas_width,
        canvas_height,
        x,
        y,
        LOOP_BUTTON_WIDTH,
        LOOP_BUTTON_HEIGHT,
        fill,
        ui_scale,
    );

    let icon = if enabled {
        [245, 247, 250, 255]
    } else {
        [185, 190, 198, 220]
    };

    draw_loop_icon(
        buffer,
        canvas_width,
        canvas_height,
        x + 3,
        y + 2,
        icon,
        ui_scale,
    );
}

fn draw_loop_icon(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: u32,
    y: u32,
    color: [u8; 4],
    ui_scale: UiScale,
) {
    draw_rect_scaled(
        buffer,
        canvas_width,
        canvas_height,
        x + 3,
        y + 4,
        11,
        2,
        color,
        ui_scale,
    );
    draw_rect_scaled(
        buffer,
        canvas_width,
        canvas_height,
        x + 2,
        y + 4,
        2,
        6,
        color,
        ui_scale,
    );
    draw_right_arrowhead_scaled(
        buffer,
        canvas_width,
        canvas_height,
        x + 13,
        y + 1,
        color,
        ui_scale,
    );

    draw_rect_scaled(
        buffer,
        canvas_width,
        canvas_height,
        x + 5,
        y + 14,
        11,
        2,
        color,
        ui_scale,
    );
    draw_rect_scaled(
        buffer,
        canvas_width,
        canvas_height,
        x + 16,
        y + 10,
        2,
        6,
        color,
        ui_scale,
    );
    draw_left_arrowhead_scaled(
        buffer,
        canvas_width,
        canvas_height,
        x + 1,
        y + 11,
        color,
        ui_scale,
    );
}

fn draw_right_arrowhead_scaled(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: u32,
    y: u32,
    color: [u8; 4],
    ui_scale: UiScale,
) {
    const SIZE: u32 = 7;
    const MID: u32 = SIZE / 2;

    for row in 0..SIZE {
        let distance = row.abs_diff(MID);
        let width = (SIZE - distance * 2).max(1);
        draw_rect_scaled(
            buffer,
            canvas_width,
            canvas_height,
            x,
            y + row,
            width,
            1,
            color,
            ui_scale,
        );
    }
}

fn draw_left_arrowhead_scaled(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: u32,
    y: u32,
    color: [u8; 4],
    ui_scale: UiScale,
) {
    const SIZE: u32 = 7;
    const MID: u32 = SIZE / 2;

    for row in 0..SIZE {
        let distance = row.abs_diff(MID);
        let width = (SIZE - distance * 2).max(1);
        draw_rect_scaled(
            buffer,
            canvas_width,
            canvas_height,
            x + SIZE - width,
            y + row,
            width,
            1,
            color,
            ui_scale,
        );
    }
}

fn draw_play_pause_button(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: u32,
    y: u32,
    paused: bool,
    ui_scale: UiScale,
) {
    blend_rect_scaled(
        buffer,
        canvas_width,
        canvas_height,
        x,
        y,
        PLAY_BUTTON_SIZE,
        PLAY_BUTTON_SIZE,
        [0, 0, 0, 96],
        ui_scale,
    );

    if paused {
        draw_play_triangle(
            buffer,
            canvas_width,
            canvas_height,
            x + 8,
            y + 6,
            [255, 255, 255, 255],
            ui_scale,
        );
    } else {
        draw_rect_scaled(
            buffer,
            canvas_width,
            canvas_height,
            x + 7,
            y + 6,
            2,
            11,
            [255, 255, 255, 255],
            ui_scale,
        );
        draw_rect_scaled(
            buffer,
            canvas_width,
            canvas_height,
            x + 13,
            y + 6,
            2,
            11,
            [255, 255, 255, 255],
            ui_scale,
        );
    }
}

fn draw_play_triangle(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: u32,
    y: u32,
    color: [u8; 4],
    ui_scale: UiScale,
) {
    const HEIGHT: u32 = 11;
    const WIDTH: u32 = 10;
    const MID: u32 = HEIGHT / 2;

    for row in 0..HEIGHT {
        let distance = row.abs_diff(MID);
        let width = 1 + (WIDTH - 1) * (MID.saturating_sub(distance)) / MID;
        let row_y = y + row;
        draw_rect_scaled(
            buffer,
            canvas_width,
            canvas_height,
            x,
            row_y,
            width,
            1,
            color,
            ui_scale,
        );
    }
}

fn draw_rect_scaled(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: [u8; 4],
    ui_scale: UiScale,
) {
    draw_rect_bgra(
        buffer,
        canvas_width,
        canvas_height,
        ui_scale.physical_pos(x),
        ui_scale.physical_pos(y),
        ui_scale.physical_len(width),
        ui_scale.physical_len(height),
        color,
    );
}

fn blend_rect_scaled(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: [u8; 4],
    ui_scale: UiScale,
) {
    blend_rect_bgra(
        buffer,
        canvas_width,
        canvas_height,
        ui_scale.physical_pos(x),
        ui_scale.physical_pos(y),
        ui_scale.physical_len(width),
        ui_scale.physical_len(height),
        color,
    );
}

fn draw_rect_bgra(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: [u8; 4],
) {
    let x_end = x.saturating_add(width).min(canvas_width);
    let y_end = y.saturating_add(height).min(canvas_height);
    if x >= x_end || y >= y_end {
        return;
    }

    let stride = canvas_width as usize * 4;

    for row in y..y_end {
        let start = row as usize * stride + x as usize * 4;
        let end = row as usize * stride + x_end as usize * 4;
        fill_bgra(&mut buffer[start..end], color);
    }
}

fn blend_rect_bgra(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: [u8; 4],
) {
    let x_end = x.saturating_add(width).min(canvas_width);
    let y_end = y.saturating_add(height).min(canvas_height);
    let alpha = color[3] as u32;
    if alpha == 0 || x >= x_end || y >= y_end {
        return;
    }
    if alpha == 255 {
        draw_rect_bgra(
            buffer,
            canvas_width,
            canvas_height,
            x,
            y,
            width,
            height,
            color,
        );
        return;
    }

    let inv_alpha = 255 - alpha;
    let stride = canvas_width as usize * 4;
    for row in y..y_end {
        let row_start = row as usize * stride;
        for col in x..x_end {
            let offset = row_start + col as usize * 4;
            buffer[offset] = blend_channel(buffer[offset], color[0], alpha, inv_alpha);
            buffer[offset + 1] = blend_channel(buffer[offset + 1], color[1], alpha, inv_alpha);
            buffer[offset + 2] = blend_channel(buffer[offset + 2], color[2], alpha, inv_alpha);
            buffer[offset + 3] = 255;
        }
    }
}

fn blend_channel(dst: u8, src: u8, alpha: u32, inv_alpha: u32) -> u8 {
    ((u32::from(src) * alpha + u32::from(dst) * inv_alpha + 127) / 255) as u8
}

fn fit_size(source_width: u32, source_height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    if source_width == 0 || source_height == 0 || max_width == 0 || max_height == 0 {
        return (0, 0);
    }

    if u64::from(max_width) * u64::from(source_height)
        <= u64::from(max_height) * u64::from(source_width)
    {
        let height =
            (u64::from(max_width) * u64::from(source_height) / u64::from(source_width)) as u32;
        (max_width, height.max(1))
    } else {
        let width =
            (u64::from(max_height) * u64::from(source_width) / u64::from(source_height)) as u32;
        (width.max(1), max_height)
    }
}

fn fill_bgra(buffer: &mut [u8], color: [u8; 4]) {
    const LANES: usize = 64;

    let mut repeated = [0u8; LANES];
    for pixel in repeated.chunks_exact_mut(4) {
        pixel.copy_from_slice(&color);
    }
    let block = Simd::<u8, LANES>::from_array(repeated);

    let mut offset = 0usize;
    while offset + LANES <= buffer.len() {
        block.copy_to_slice(&mut buffer[offset..offset + LANES]);
        offset += LANES;
    }

    while offset < buffer.len() {
        buffer[offset] = color[offset & 3];
        offset += 1;
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    let args = parse_args(std::env::args().collect());
    let video_path = args.video_path;
    let window_title = video_window_title(args.title.as_deref(), &video_path);
    println!("[{}] playing {}", APP_NAME, video_path);
    if args.hardware_decode {
        println!("[{}] hardware decoder {}", APP_NAME, VIDEO_DEVICE_PATH);
    }
    let mp4_data = if is_mp4_path(&video_path) && !args.streaming {
        println!("[{}] loading MP4 {}", APP_NAME, video_path);
        Some(Arc::new(read_file(&video_path).unwrap_or_else(|_| {
            println!("[{}] Application error: failed to read MP4 file", APP_NAME);
            Vec::new()
        })))
    } else {
        None
    };
    if matches!(mp4_data.as_ref().map(|data| data.is_empty()), Some(true)) {
        return 1;
    }
    let audio_source = if let Some(socket_path) = args.audio_socket_path {
        println!("[{}] audio local socket {}", APP_NAME, socket_path);
        Some(PlayerAudioSource::StreamingMp4AacSocket { socket_path })
    } else if let Some(path) = args.audio_path {
        println!("[{}] audio {}", APP_NAME, path);
        if is_mp4_path(&path) {
            if args.streaming {
                Some(PlayerAudioSource::StreamingMp4Aac {
                    path,
                    complete_path: args.audio_complete_path,
                })
            } else {
                match read_file(&path) {
                    Ok(data) => Some(PlayerAudioSource::Mp4Aac(Arc::new(data))),
                    Err(_) => {
                        println!(
                            "[{}] Application error: failed to read audio file",
                            APP_NAME
                        );
                        return 1;
                    }
                }
            }
        } else {
            Some(PlayerAudioSource::Wav(path))
        }
    } else if let Some(data) = &mp4_data {
        println!("[{}] audio MP4/AAC", APP_NAME);
        Some(PlayerAudioSource::Mp4Aac(data.clone()))
    } else {
        None
    };

    let mut app = VideoPlayerApp::new(
        video_path,
        window_title,
        mp4_data,
        audio_source,
        args.hardware_decode,
        args.streaming,
        args.loop_playback,
        args.stream_complete_path,
        args.stream_socket_path,
    );
    match app.run() {
        Ok(()) => 0,
        Err(error) => {
            println!("[{}] Application error: {}", APP_NAME, error);
            1
        }
    }
}

fn is_mp4_path(path: &str) -> bool {
    path.ends_with(".mp4") || path.ends_with(".m4v") || path.ends_with(".m4a")
}

struct PlayerArgs {
    video_path: String,
    title: Option<String>,
    audio_path: Option<String>,
    audio_complete_path: Option<String>,
    hardware_decode: bool,
    streaming: bool,
    loop_playback: bool,
    stream_complete_path: Option<String>,
    stream_socket_path: Option<String>,
    audio_socket_path: Option<String>,
}

fn parse_args(args: Vec<String>) -> PlayerArgs {
    let mut positional = Vec::new();
    let mut title = None;
    let mut audio_path = None;
    let mut audio_complete_path = None;
    let mut hardware_decode = false;
    let mut streaming = false;
    let mut loop_playback = false;
    let mut stream_complete_path = None;
    let mut stream_socket_path = None;
    let mut audio_socket_path = None;

    let mut args = args.into_iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--hwdc" || arg == "--hwdec" {
            hardware_decode = true;
        } else if arg == "--software" || arg == "--swdec" {
            hardware_decode = false;
        } else if arg == "--loop" {
            loop_playback = true;
        } else if arg == "--title" {
            title = args.next();
        } else if let Some(value) = arg.strip_prefix("--title=") {
            title = Some(String::from(value));
        } else if arg == "--audio" {
            audio_path = args.next();
        } else if let Some(path) = arg.strip_prefix("--audio=") {
            audio_path = Some(String::from(path));
        } else if arg == "--audio-complete" {
            audio_complete_path = args.next();
        } else if let Some(path) = arg.strip_prefix("--audio-complete=") {
            audio_complete_path = Some(String::from(path));
        } else if arg == "--audio-socket" {
            audio_socket_path = args.next();
        } else if let Some(path) = arg.strip_prefix("--audio-socket=") {
            audio_socket_path = Some(String::from(path));
        } else if arg == "--stream" || arg == "--streaming" {
            streaming = true;
        } else if arg == "--stream-socket" {
            streaming = true;
            stream_socket_path = args.next();
        } else if let Some(path) = arg.strip_prefix("--stream-socket=") {
            streaming = true;
            stream_socket_path = Some(String::from(path));
        } else if arg == "--stream-complete" {
            stream_complete_path = args.next();
        } else if let Some(path) = arg.strip_prefix("--stream-complete=") {
            stream_complete_path = Some(String::from(path));
        } else {
            positional.push(arg);
        }
    }

    let video_path = positional
        .first()
        .cloned()
        .unwrap_or_else(|| String::from(DEFAULT_VIDEO_PATH));

    PlayerArgs {
        video_path,
        title,
        audio_path,
        audio_complete_path,
        hardware_decode,
        streaming,
        loop_playback,
        stream_complete_path,
        stream_socket_path,
        audio_socket_path,
    }
}

fn video_window_title(title: Option<&str>, path: &str) -> String {
    let display_title = title
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| file_name_for_title(path));
    format!("Video Player - {}", display_title)
}

fn file_name_for_title(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return path;
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}
