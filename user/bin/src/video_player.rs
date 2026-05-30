#![no_std]
#![no_main]
#![feature(portable_simd)]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
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
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering, compiler_fence};
use core::time::Duration;

use rust_h264::decoder::{Decoder, Frame};
use rust_h264::nal::parse_annex_b;
use scarlet_ui::{
    Application, CanvasView, ComponentElement, Element, Event, InvalidationKind, KeyCode, KeyEvent,
    Listenable, MouseButton, MouseEvent, Size, SubscriptionId, View, ViewExt, Window,
};
use std::audio::AUDIO_PCM_FORMAT_S16LE;
use std::fs::{File, OpenOptions};
use std::handle::capability::memory_mapping::{flags as mmap_flags, munmap, prot};
use std::io::{Read, Write};
use std::ipc::SharedMemory;
use std::socket::Socket;
use std::sync::Mutex;
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
use userprogram::sas_protocol as protocol;

const DEFAULT_VIDEO_PATH: &str = "/root/media/bad_apple.h264";
const APP_NAME: &str = env!("CARGO_BIN_NAME");
const VIDEO_WIDTH: u32 = 480;
const VIDEO_HEIGHT: u32 = 360;
const DISPLAY_WIDTH: u32 = 640;
const DISPLAY_HEIGHT: u32 = 360;
const FRAME_INTERVAL_MS: u64 = 33;
const CONTROLS_HIDE_INTERVAL_MS: u64 = 250;
const CONTROLS_HIDE_IDLE_TICKS: u32 = 6;
const CONTROLS_MIN_WIDTH: u32 = 96;
const CONTROLS_MIN_HEIGHT: u32 = 48;
const CONTROLS_PANEL_HEIGHT: u32 = 56;
const PLAY_BUTTON_SIZE: u32 = 32;
const PLAY_BUTTON_LEFT_INSET: u32 = 16;
const PLAY_BUTTON_TOP_INSET: u32 = 12;
const VVIDEO_DEVICE_PATH: &str = "/dev/vvideo0";
const SCARLET_VIDEO_FRAME_HEADER_LEN: usize = 20;
const NV12_VIDEO_RANGE_PIXEL_FORMAT: u32 = 0x3432_3076;
const VVIDEO_GET_BUFFER: u32 = 0x5600;
const VVIDEO_SUBMIT: u32 = 0x5601;
const VVIDEO_DEQUEUE: u32 = 0x5602;
const VVIDEO_CREATE_SESSION: u32 = 0x5603;
const VVIDEO_SUBMIT_SESSION: u32 = 0x5604;
const VVIDEO_DEQUEUE_SESSION: u32 = 0x5605;
const VVIDEO_DESTROY_SESSION: u32 = 0x5606;
const VIRTIO_VIDEO_FORMAT_H264: u32 = 4098;
const VIRTIO_VIDEO_FORMAT_AV1: u32 = 4103;
const SCARLET_AV1_ACCESS_UNIT_MAGIC: &[u8; 4] = b"SVA1";

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
}

struct ControlsOverlay {
    visible: AtomicBool,
    activity_epoch: AtomicU32,
    paused: AtomicBool,
    canvas_width: AtomicU32,
    canvas_height: AtomicU32,
}

impl ControlsOverlay {
    fn new() -> Self {
        Self {
            visible: AtomicBool::new(false),
            activity_epoch: AtomicU32::new(0),
            paused: AtomicBool::new(false),
            canvas_width: AtomicU32::new(DISPLAY_WIDTH),
            canvas_height: AtomicU32::new(DISPLAY_HEIGHT),
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

    fn toggle_paused(&self) {
        let paused = !self.paused.load(Ordering::Acquire);
        self.paused.store(paused, Ordering::Release);
        self.show_for_mouse_activity();
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
    mp4_data: Option<Arc<Vec<u8>>>,
    audio_source: Option<PlayerAudioSource>,
    hardware_decode: bool,
    frame_store: Arc<VideoFrameStore>,
    controls: Arc<ControlsOverlay>,
    paint_signal: Arc<PaintSignal>,
    clock: Arc<AudioClock>,
}

#[derive(Clone)]
enum PlayerAudioSource {
    Wav(String),
    Mp4Aac(Arc<Vec<u8>>),
}

impl VideoPlayerApp {
    fn new(
        path: String,
        mp4_data: Option<Arc<Vec<u8>>>,
        audio_source: Option<PlayerAudioSource>,
        hardware_decode: bool,
    ) -> Self {
        Self {
            path,
            mp4_data,
            audio_source,
            hardware_decode,
            frame_store: Arc::new(VideoFrameStore::new()),
            controls: Arc::new(ControlsOverlay::new()),
            paint_signal: Arc::new(PaintSignal::new()),
            clock: Arc::new(AudioClock::new()),
        }
    }
}

struct AudioClock {
    started: AtomicBool,
    unavailable: AtomicBool,
    sample_rate: AtomicU64,
    read_frames: AtomicU64,
}

impl AudioClock {
    fn new() -> Self {
        Self {
            started: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
            sample_rate: AtomicU64::new(48_000),
            read_frames: AtomicU64::new(0),
        }
    }

    fn mark_started(&self, sample_rate: u32) {
        self.sample_rate
            .store(u64::from(sample_rate), Ordering::Release);
        self.started.store(true, Ordering::Release);
    }

    fn mark_unavailable(&self) {
        self.unavailable.store(true, Ordering::Release);
    }

    fn is_unavailable(&self) -> bool {
        self.unavailable.load(Ordering::Acquire)
    }

    fn update_read_frames(&self, read_frames: u64) {
        self.read_frames.store(read_frames, Ordering::Release);
    }

    fn elapsed_us(&self) -> Option<u64> {
        if !self.started.load(Ordering::Acquire) {
            return None;
        }
        let rate = self.sample_rate.load(Ordering::Acquire).max(1);
        let audio_frames = self.read_frames.load(Ordering::Acquire);
        Some(audio_frames.saturating_mul(1_000_000) / rate)
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
    fn body(&self) -> impl View {
        let frame_store = self.frame_store.clone();
        let controls = self.controls.clone();
        let controls_for_event = self.controls.clone();
        let controls_for_key = self.controls.clone();
        let paint_signal_for_key = self.paint_signal.clone();
        Window::new(
            "Video Player",
            CanvasView::new(
                DISPLAY_WIDTH as f32,
                DISPLAY_HEIGHT as f32,
                Rc::new(move |buffer, width, height| {
                    draw_video_frame(buffer, width, height, &frame_store, &controls);
                }),
            )
            .on_event(move |event| handle_canvas_event(event, &controls_for_event))
            .on_key(move |event| handle_key_event(event, &controls_for_key, &paint_signal_for_key)),
        )
        .app_id("org.scarlet-os.video-player")
        .size(Size::new(DISPLAY_WIDTH as f32, DISPLAY_HEIGHT as f32))
    }

    fn init(&mut self) {
        start_controls_thread(self.controls.clone(), self.paint_signal.clone());
        if let Some(audio_source) = self.audio_source.clone() {
            start_audio_thread(audio_source, self.clock.clone(), self.controls.clone());
        }
        start_decoder_thread(
            self.path.clone(),
            self.mp4_data.clone(),
            self.frame_store.clone(),
            self.paint_signal.clone(),
            self.controls.clone(),
            self.audio_source.is_some().then(|| self.clock.clone()),
            self.hardware_decode,
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
) {
    thread::spawn(move || {
        let result = if hardware_decode {
            decode_loop_hardware(
                &path,
                mp4_data.as_deref().map(Vec::as_slice),
                &frame_store,
                &paint_signal,
                &controls,
                clock.as_deref(),
            )
        } else {
            decode_loop_software(
                &path,
                mp4_data.as_deref().map(Vec::as_slice),
                &frame_store,
                &paint_signal,
                &controls,
                clock.as_deref(),
            )
        };

        if let Err(err) = result {
            println!("[{}] {}", APP_NAME, err);
        }
    });
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
    frame_store: &VideoFrameStore,
    paint_signal: &PaintSignal,
    controls: &ControlsOverlay,
    clock: Option<&AudioClock>,
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
    let mut decoder = Decoder::new();
    let mut display_index = 0usize;
    let mut access_unit_scratch = Vec::new();
    println!(
        "[{}] software decode: {} {} access units",
        APP_NAME,
        source.description(),
        source.access_units.len()
    );

    for access_unit in &source.access_units {
        wait_while_paused(controls);
        let access_unit_bytes = access_unit.bytes(mp4_data, &mut access_unit_scratch)?;
        let nals = parse_annex_b(access_unit_bytes);
        for nal in &nals {
            match decoder.decode_nal(nal) {
                Ok(Some(frame)) => {
                    publish_frame_synced(
                        frame_store,
                        paint_signal,
                        controls,
                        DecodedVideoFrame::Software(frame),
                        display_index,
                        total_frames,
                        access_unit.presentation_time_us,
                        clock,
                    )?;
                    display_index += 1;
                }
                Ok(None) => {}
                Err(err) => return Err(format!("decode failed: {err}")),
            }
        }
    }

    if let Some(frame) = decoder.flush() {
        publish_frame_synced(
            frame_store,
            paint_signal,
            controls,
            DecodedVideoFrame::Software(frame),
            display_index,
            total_frames,
            display_index as u64 * FRAME_INTERVAL_MS * 1_000,
            clock,
        )?;
        display_index += 1;
    }

    frame_store.mark_complete();
    paint_signal.notify();
    println!("[{}] finished: {} frames", APP_NAME, display_index);

    Ok(())
}

fn decode_loop_hardware(
    path: &str,
    mp4_data: Option<&[u8]>,
    frame_store: &VideoFrameStore,
    paint_signal: &PaintSignal,
    controls: &ControlsOverlay,
    clock: Option<&AudioClock>,
) -> Result<(), String> {
    let source = load_video_source(path, mp4_data)?;
    let total_frames = source.access_units.len().max(1) as u32;
    let mut decoder = HardwareVideoDecoder::open()?;
    let mut reorder = FrameReorderBuffer::new(total_frames);
    let mut access_unit_scratch = Vec::new();
    println!(
        "[{}] hardware decode: {} {} access units",
        APP_NAME,
        source.description(),
        source.access_units.len()
    );

    for access_unit in &source.access_units {
        wait_while_paused(controls);
        let access_unit_bytes = access_unit.bytes(mp4_data, &mut access_unit_scratch)?;
        let Some(frame) = decoder.decode_access_unit(access_unit.codec, access_unit_bytes)? else {
            return Err(String::from("hardware decoder produced no frame"));
        };
        if reorder.can_publish_immediately(access_unit.display_rank) {
            reorder.publish_immediate(
                frame_store,
                paint_signal,
                controls,
                clock,
                access_unit.presentation_time_us,
                DecodedVideoFrame::Hardware(frame),
            )?;
        } else {
            reorder.push(
                access_unit.display_rank,
                access_unit.presentation_time_us,
                DecodedVideoFrame::Hardware(frame.into_owned()),
            )?;
            reorder.publish_ready(frame_store, paint_signal, controls, clock)?;
        }
    }
    reorder.finish(frame_store, paint_signal, controls, clock)?;

    frame_store.mark_complete();
    paint_signal.notify();
    println!("[{}] finished: {} frames", APP_NAME, reorder.published());

    Ok(())
}

struct VideoSource {
    format: VideoContainerFormat,
    access_units: Vec<VideoAccessUnit>,
}

struct VideoAccessUnit {
    payload: VideoAccessUnitPayload,
    codec: VideoCodec,
    display_rank: usize,
    presentation_time_us: u64,
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
        access_units: annex_b_access_units(&data)
            .into_iter()
            .enumerate()
            .map(|(display_rank, bytes)| VideoAccessUnit {
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
    data_start: usize,
    data_end: usize,
}

#[derive(Default)]
struct Mp4Track {
    is_video: bool,
    is_audio: bool,
    avcc: Option<AvcConfig>,
    av1: Option<Av1Config>,
    aac: Option<AacConfig>,
    media_timescale: u32,
    sample_sizes: Vec<u32>,
    time_to_sample: Vec<TimeToSampleEntry>,
    composition_offsets: Vec<i64>,
    sample_to_chunk: Vec<SampleToChunkEntry>,
    chunk_offsets: Vec<u64>,
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
    let sample_offsets = mp4_sample_offsets(&track)?;
    if sample_offsets.len() != track.sample_sizes.len() {
        return Err(String::from("MP4 sample table is inconsistent"));
    }
    let (display_ranks, presentation_times_us) = mp4_display_timing(&track)?;

    let mut access_units = Vec::new();
    for (index, offset) in sample_offsets.iter().enumerate() {
        let offset = *offset as usize;
        let size = track.sample_sizes[index] as usize;
        let end = offset
            .checked_add(size)
            .ok_or_else(|| String::from("MP4 sample offset overflow"))?;
        let sample = data
            .get(offset..end)
            .ok_or_else(|| String::from("MP4 sample points outside file"))?;
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
        access_units.push(VideoAccessUnit {
            payload,
            codec,
            display_rank: display_ranks[index],
            presentation_time_us: presentation_times_us[index],
        });
    }

    Ok(VideoSource {
        format: video_format,
        access_units,
    })
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
    let sample_offsets = mp4_sample_offsets(&track)?;
    if sample_offsets.len() != track.sample_sizes.len() {
        return Err(String::from("MP4 audio sample table is inconsistent"));
    }

    let mut samples = Vec::new();
    for (index, offset) in sample_offsets.iter().enumerate() {
        let offset = *offset as usize;
        let size = track.sample_sizes[index] as usize;
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
            b"hdlr" => parse_hdlr(data, mp4_box.data_start, mp4_box.data_end, track)?,
            b"mdhd" => track.media_timescale = parse_mdhd(data, mp4_box.data_start)?,
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
        data_start,
        data_end,
    })
}

fn parse_hdlr(data: &[u8], start: usize, _end: usize, track: &mut Mp4Track) -> Result<(), String> {
    let handler = data
        .get(start + 8..start + 12)
        .ok_or_else(|| String::from("MP4 hdlr box is truncated"))?;
    track.is_video = handler == b"vide";
    track.is_audio = handler == b"soun";
    Ok(())
}

fn parse_mdhd(data: &[u8], start: usize) -> Result<u32, String> {
    let version = *data
        .get(start)
        .ok_or_else(|| String::from("MP4 mdhd box is truncated"))?;
    let timescale_offset = if version == 1 {
        start
            .checked_add(20)
            .ok_or_else(|| String::from("MP4 mdhd offset overflow"))?
    } else {
        start
            .checked_add(12)
            .ok_or_else(|| String::from("MP4 mdhd offset overflow"))?
    };
    let timescale = read_u32_be(
        data.get(timescale_offset..timescale_offset + 4)
            .ok_or_else(|| String::from("MP4 mdhd timescale is truncated"))?,
    );
    Ok(timescale)
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
                // SAFETY: mapped video frames point into the live /dev/vvideo0
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
}

impl HardwareVideoDecoder {
    fn open() -> Result<Self, String> {
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(VVIDEO_DEVICE_PATH)
            .map_err(|_| format!("failed to open {}", VVIDEO_DEVICE_PATH))?;
        let mapped = Self::map_video_buffer(&device);
        if let Some(buffer) = &mapped {
            println!(
                "[{}] hardware decoder mmap input={} output={}",
                APP_NAME, buffer.input_len, buffer.output_len
            );
        }
        Ok(Self { device, mapped })
    }

    fn decode_access_unit(
        &mut self,
        codec: VideoCodec,
        access_unit: &[u8],
    ) -> Result<Option<ScarletVideoFrame>, String> {
        if access_unit.is_empty() {
            return Ok(None);
        }
        if let Some(buffer) = &self.mapped {
            if access_unit.len() <= buffer.input_len {
                return self.decode_access_unit_mapped(codec, access_unit);
            }
        }
        if codec != VideoCodec::H264 {
            return Err(String::from(
                "hardware decoder mmap input overflow for non-H.264 access unit",
            ));
        }
        self.decode_access_unit_stream(access_unit)
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

        if buffer.session_commands {
            let submit = ScarletVideoSessionSubmit {
                stream_id: buffer.stream_id,
                input_len: access_unit.len() as u32,
                coded_format: codec.coded_format(),
                padding: 0,
                timestamp: 0,
            };
            self.device
                .as_handle()
                .control(VVIDEO_SUBMIT_SESSION, &submit as *const _ as usize)
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
                .control(VVIDEO_SUBMIT, &submit as *const _ as usize)
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
                    VVIDEO_DEQUEUE_SESSION,
                    &mut session_frame as *mut _ as usize,
                );
                result.map(|value| (value, session_frame.frame))
            } else {
                let mut frame = ScarletVideoDequeuedFrame::default();
                let result = self
                    .device
                    .as_handle()
                    .control(VVIDEO_DEQUEUE, &mut frame as *mut _ as usize);
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

    fn map_video_buffer(device: &File) -> Option<MappedVideoBuffer> {
        let mut session_info = ScarletVideoSessionInfo::default();
        let (stream_id, session_commands, info) = if device
            .as_handle()
            .control(VVIDEO_CREATE_SESSION, &mut session_info as *mut _ as usize)
            .is_ok()
        {
            (session_info.stream_id, true, session_info.buffer)
        } else {
            let mut info = ScarletVideoBufferInfo::default();
            device
                .as_handle()
                .control(VVIDEO_GET_BUFFER, &mut info as *mut _ as usize)
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
            let _ = munmap(buffer.ptr as usize, buffer.mmap_len);
            if buffer.session_commands {
                let info = ScarletVideoSessionInfo {
                    stream_id: buffer.stream_id,
                    padding: 0,
                    buffer: ScarletVideoBufferInfo::default(),
                };
                let _ = self
                    .device
                    .as_handle()
                    .control(VVIDEO_DESTROY_SESSION, &info as *const _ as usize);
            }
        }
    }
}

enum DecodedVideoFrame {
    Software(Frame),
    Hardware(ScarletVideoFrame),
}

struct FrameReorderBuffer {
    pending: Vec<(usize, u64, DecodedVideoFrame)>,
    next_rank: usize,
    total_frames: u32,
    published: usize,
}

impl FrameReorderBuffer {
    fn new(total_frames: u32) -> Self {
        Self {
            pending: Vec::new(),
            next_rank: 0,
            total_frames,
            published: 0,
        }
    }

    fn can_publish_immediately(&self, display_rank: usize) -> bool {
        self.pending.is_empty() && display_rank == self.next_rank
    }

    fn publish_immediate(
        &mut self,
        frame_store: &VideoFrameStore,
        paint_signal: &PaintSignal,
        controls: &ControlsOverlay,
        clock: Option<&AudioClock>,
        presentation_time_us: u64,
        frame: DecodedVideoFrame,
    ) -> Result<(), String> {
        publish_frame_synced(
            frame_store,
            paint_signal,
            controls,
            frame,
            self.published,
            self.total_frames,
            presentation_time_us,
            clock,
        )?;
        self.next_rank += 1;
        self.published += 1;
        Ok(())
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
        frame_store: &VideoFrameStore,
        paint_signal: &PaintSignal,
        controls: &ControlsOverlay,
        clock: Option<&AudioClock>,
    ) -> Result<(), String> {
        while let Some(index) = self
            .pending
            .iter()
            .position(|(rank, _, _)| *rank == self.next_rank)
        {
            let (_, presentation_time_us, frame) = self.pending.remove(index);
            publish_frame_synced(
                frame_store,
                paint_signal,
                controls,
                frame,
                self.published,
                self.total_frames,
                presentation_time_us,
                clock,
            )?;
            self.next_rank += 1;
            self.published += 1;
        }
        Ok(())
    }

    fn finish(
        &mut self,
        frame_store: &VideoFrameStore,
        paint_signal: &PaintSignal,
        controls: &ControlsOverlay,
        clock: Option<&AudioClock>,
    ) -> Result<(), String> {
        self.pending.sort_by_key(|(rank, _, _)| *rank);
        while !self.pending.is_empty() {
            let (_, presentation_time_us, frame) = self.pending.remove(0);
            publish_frame_synced(
                frame_store,
                paint_signal,
                controls,
                frame,
                self.published,
                self.total_frames,
                presentation_time_us,
                clock,
            )?;
            self.published += 1;
        }
        Ok(())
    }

    fn published(&self) -> usize {
        self.published
    }
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
}

fn start_audio_thread(
    source: PlayerAudioSource,
    clock: Arc<AudioClock>,
    controls: Arc<ControlsOverlay>,
) {
    thread::spawn(move || {
        if let Err(err) = play_audio_source_sas(&source, &clock, &controls) {
            clock.mark_unavailable();
            println!("[{}] audio: {}", APP_NAME, err);
        }
    });
}

fn play_audio_source_sas(
    source: &PlayerAudioSource,
    clock: &AudioClock,
    controls: &ControlsOverlay,
) -> Result<(), String> {
    match source {
        PlayerAudioSource::Wav(path) => play_wav_sas(path, clock, controls),
        PlayerAudioSource::Mp4Aac(data) => play_mp4_aac_sas(data.clone(), clock, controls),
    }
}

fn play_wav_sas(path: &str, clock: &AudioClock, controls: &ControlsOverlay) -> Result<(), String> {
    let bytes = read_file(path)?;
    let wav = parse_wav(&bytes)?;
    if wav.audio_format != 1 || wav.bits_per_sample != 16 {
        return Err(String::from("SAS accepts only PCM S16LE WAV files"));
    }
    let data = &bytes[wav.data_offset..wav.data_offset + wav.data_len];
    play_sas_pcm_s16le(data, wav.sample_rate, wav.channels, clock, controls)
}

fn play_mp4_aac_sas(
    data: Arc<Vec<u8>>,
    clock: &AudioClock,
    controls: &ControlsOverlay,
) -> Result<(), String> {
    #[cfg(not(feature = "mp4-aac"))]
    {
        let _ = (data, clock, controls);
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
        play_aac_source_sas(&source, clock, controls)
    }
}

fn play_sas_pcm_s16le(
    data: &[u8],
    sample_rate: u32,
    channels: u16,
    clock: &AudioClock,
    controls: &ControlsOverlay,
) -> Result<(), String> {
    if sample_rate == 0 {
        return Err(String::from("audio sample rate is zero"));
    }
    if channels == 0 {
        return Err(String::from("audio channel count is zero"));
    }
    let frame_bytes = channels as usize * 2;
    if data.len() < frame_bytes {
        clock.mark_started(sample_rate);
        return Ok(());
    }

    let mut writer = SasPcmWriter::new(sample_rate, channels, frame_bytes, clock)?;
    writer.write_bytes(data, controls, clock)?;
    writer.drain_close(clock)
}

struct SasPcmWriter {
    socket: Socket,
    ring_addr: usize,
    frame_bytes: usize,
}

impl SasPcmWriter {
    fn new(
        sample_rate: u32,
        channels: u16,
        frame_bytes: usize,
        clock: &AudioClock,
    ) -> Result<Self, String> {
        let mut socket = Socket::new().map_err(|_| format!("failed to create SAS socket"))?;
        socket
            .connect(protocol::SOCKET_PATH)
            .map_err(|_| format!("failed to connect to SAS"))?;

        let period_frames = (sample_rate / 100).max(64);
        let buffer_frames = (sample_rate / 5).max(period_frames * 4);
        let config = protocol::Config {
            format: AUDIO_PCM_FORMAT_S16LE,
            rate: sample_rate,
            channels,
            reserved: 0,
            period_frames,
            buffer_frames,
        };
        write_sas_frame(&mut socket, protocol::MSG_CONFIGURE, &config.to_le_bytes())?;
        read_sas_ok(&mut socket)?;
        let shm_handle = socket
            .recv_handle()
            .map_err(|_| format!("failed to receive SAS shared ring"))?;
        let shm = SharedMemory::from_handle(shm_handle).map_err(|_| format!("invalid SAS ring"))?;
        let ring_size = protocol::RING_HEADER_SIZE + config.buffer_frames as usize * frame_bytes;
        let mapper = shm
            .as_handle()
            .as_memory_mapping()
            .map_err(|_| format!("SAS shared ring is not mappable"))?;
        let ring_addr = mapper
            .mmap(
                0,
                ring_size,
                prot::READ | prot::WRITE,
                mmap_flags::SHARED,
                0,
            )
            .map_err(|_| format!("failed to map SAS shared ring"))?;

        clock.mark_started(sample_rate);

        Ok(Self {
            socket,
            ring_addr,
            frame_bytes,
        })
    }

    fn write_bytes(
        &mut self,
        data: &[u8],
        controls: &ControlsOverlay,
        clock: &AudioClock,
    ) -> Result<(), String> {
        let total_data_frames = data.len() / self.frame_bytes;
        let mut pos_frame = 0usize;

        while pos_frame < total_data_frames {
            // SAFETY: `ring_addr` is the SAS shared ring mmap returned by SAS.
            clock.update_read_frames(unsafe { sas_ring_read_frames(self.ring_addr) });
            while controls.is_paused() {
                // SAFETY: `ring_addr` remains mapped while audio playback is active.
                clock.update_read_frames(unsafe { sas_ring_read_frames(self.ring_addr) });
                thread::sleep(Duration::from_millis(10));
            }
            // SAFETY: `ring_addr` is the SAS shared ring mmap returned by SAS.
            let frames =
                unsafe { writable_sas_frames(self.ring_addr).min(total_data_frames - pos_frame) };
            if frames == 0 {
                thread::sleep(Duration::from_millis(2));
                continue;
            }

            let data_offset = pos_frame * self.frame_bytes;
            // SAFETY: `frames` is bounded by SAS writable space and source data length.
            unsafe {
                write_sas_ring_chunk(
                    self.ring_addr,
                    &data[data_offset..data_offset + frames * self.frame_bytes],
                    self.frame_bytes,
                    frames,
                );
            }
            pos_frame += frames;
        }
        Ok(())
    }

    fn drain_close(mut self, clock: &AudioClock) -> Result<(), String> {
        write_sas_frame(&mut self.socket, protocol::MSG_DRAIN, &[])?;
        // SAFETY: `ring_addr` remains mapped until playback cleanup completes.
        while !unsafe { sas_ring_is_empty(self.ring_addr) } {
            // SAFETY: `ring_addr` remains mapped until playback cleanup completes.
            clock.update_read_frames(unsafe { sas_ring_read_frames(self.ring_addr) });
            thread::sleep(Duration::from_millis(10));
        }
        read_sas_ok(&mut self.socket)?;
        let _ = write_sas_frame(&mut self.socket, protocol::MSG_CLOSE, &[]);
        Ok(())
    }
}

#[cfg(feature = "mp4-aac")]
fn play_aac_source_sas(
    source: &Mp4AacAudioSource,
    clock: &AudioClock,
    controls: &ControlsOverlay,
) -> Result<(), String> {
    let frame_bytes = source.config.channels as usize * 2;
    if frame_bytes == 0 {
        return Err(String::from("AAC channel count is zero"));
    }
    let mut writer = SasPcmWriter::new(
        source.config.sample_rate,
        source.config.channels,
        frame_bytes,
        clock,
    )?;
    let mut decoder = create_aac_decoder(source)?;
    let mut samples = Vec::<i16>::new();
    let mut bytes = Vec::<u8>::new();
    let mut pts = 0i64;
    for range in &source.samples {
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
        writer.write_bytes(&bytes, controls, clock)?;
    }
    writer.drain_close(clock)
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

fn publish_frame_synced(
    frame_store: &VideoFrameStore,
    paint_signal: &PaintSignal,
    controls: &ControlsOverlay,
    frame: DecodedVideoFrame,
    display_index: usize,
    total_frames: u32,
    presentation_time_us: u64,
    clock: Option<&AudioClock>,
) -> Result<(), String> {
    wait_while_paused(controls);
    if let Some(clock) = clock {
        loop {
            wait_while_paused(controls);
            let Some(audio_time_us) = clock.elapsed_us() else {
                if clock.is_unavailable() {
                    thread::sleep(Duration::from_millis(FRAME_INTERVAL_MS));
                    break;
                }
                thread::sleep(Duration::from_millis(1));
                continue;
            };
            if audio_time_us >= presentation_time_us {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
    } else {
        thread::sleep(Duration::from_millis(FRAME_INTERVAL_MS));
    }

    publish_frame(
        frame_store,
        paint_signal,
        controls,
        frame,
        display_index,
        total_frames,
    )
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

fn write_sas_frame(socket: &mut Socket, msg_type: u32, payload: &[u8]) -> Result<(), String> {
    let frame = protocol::frame(msg_type, payload);
    write_all(socket, &frame)
}

fn read_sas_ok(socket: &mut Socket) -> Result<(), String> {
    let mut header_bytes = [0u8; protocol::HEADER_SIZE];
    read_exact(socket, &mut header_bytes)?;
    let header = protocol::Header::from_le_bytes(header_bytes);
    if header.payload_size as usize > protocol::MAX_PAYLOAD_SIZE {
        return Err(String::from("SAS response too large"));
    }

    let mut payload = Vec::new();
    payload.resize(header.payload_size as usize, 0);
    if !payload.is_empty() {
        read_exact(socket, &mut payload)?;
    }

    match header.msg_type {
        protocol::MSG_OK => Ok(()),
        protocol::MSG_ERROR => Err(String::from("SAS returned an error")),
        _ => Err(String::from("unexpected SAS response")),
    }
}

fn read_exact(socket: &mut Socket, out: &mut [u8]) -> Result<(), String> {
    let mut read = 0usize;
    while read < out.len() {
        match socket.read(&mut out[read..]) {
            Ok(0) => return Err(String::from("SAS socket closed")),
            Ok(n) => read += n,
            Err(_) => core::hint::spin_loop(),
        }
    }
    Ok(())
}

fn write_all(socket: &mut Socket, bytes: &[u8]) -> Result<(), String> {
    let mut written = 0usize;
    while written < bytes.len() {
        match socket.write(&bytes[written..]) {
            Ok(0) => return Err(String::from("SAS socket closed")),
            Ok(n) => written += n,
            Err(_) => core::hint::spin_loop(),
        }
    }
    socket
        .flush()
        .map_err(|_| format!("failed to flush SAS socket"))
}

unsafe fn sas_ring_is_empty(ring_addr: usize) -> bool {
    let header = ring_addr as *mut protocol::RingHeader;
    // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
    let read_frames = unsafe { core::ptr::addr_of!((*header).read_frames).read_volatile() };
    // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
    let write_frames = unsafe { core::ptr::addr_of!((*header).write_frames).read_volatile() };
    read_frames >= write_frames
}

unsafe fn sas_ring_read_frames(ring_addr: usize) -> u64 {
    let header = ring_addr as *mut protocol::RingHeader;
    // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
    unsafe { core::ptr::addr_of!((*header).read_frames).read_volatile() }
}

unsafe fn writable_sas_frames(ring_addr: usize) -> usize {
    let header = ring_addr as *mut protocol::RingHeader;
    // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
    let buffer_frames =
        unsafe { core::ptr::addr_of!((*header).buffer_frames).read_volatile() as usize };
    // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
    let read_frames = unsafe { core::ptr::addr_of!((*header).read_frames).read_volatile() };
    // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
    let write_frames = unsafe { core::ptr::addr_of!((*header).write_frames).read_volatile() };
    let queued_frames = write_frames.saturating_sub(read_frames) as usize;
    buffer_frames.saturating_sub(queued_frames)
}

unsafe fn write_sas_ring_chunk(ring_addr: usize, data: &[u8], frame_bytes: usize, frames: usize) {
    let header = ring_addr as *mut protocol::RingHeader;
    let data_ptr = (ring_addr + protocol::RING_HEADER_SIZE) as *mut u8;
    // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
    let buffer_frames =
        unsafe { core::ptr::addr_of!((*header).buffer_frames).read_volatile() as usize };
    // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
    let write_frames = unsafe { core::ptr::addr_of!((*header).write_frames).read_volatile() };
    let ring_frame = write_frames as usize % buffer_frames;
    let contiguous_frames = frames.min(buffer_frames - ring_frame);
    let ring_offset = ring_frame * frame_bytes;
    let first_bytes = contiguous_frames * frame_bytes;

    // SAFETY: caller bounded `frames` by ring writable space and input length.
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr.add(ring_offset), first_bytes);
    }

    let remaining_frames = frames - contiguous_frames;
    if remaining_frames != 0 {
        let second_bytes = remaining_frames * frame_bytes;
        // SAFETY: wrap copy writes from the start of the same mapped ring.
        unsafe {
            core::ptr::copy_nonoverlapping(data[first_bytes..].as_ptr(), data_ptr, second_bytes);
        }
    }

    compiler_fence(Ordering::Release);
    // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
    unsafe {
        core::ptr::addr_of_mut!((*header).write_frames)
            .write_volatile(write_frames + frames as u64);
    }
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
    controls.update_canvas_size(canvas_width, canvas_height);
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
        draw_seek_bar(buffer, canvas_width, canvas_height, &frame, controls);
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

    draw_seek_bar(buffer, canvas_width, canvas_height, &frame, controls);
}

fn handle_canvas_event(event: &Event, controls: &ControlsOverlay) -> bool {
    match event {
        Event::Mouse(MouseEvent::Entered { .. }) | Event::Mouse(MouseEvent::Moved { .. }) => {
            controls.show_for_mouse_activity()
        }
        Event::Mouse(MouseEvent::Exited { .. }) => controls.hide(),
        Event::Mouse(MouseEvent::ButtonReleased {
            button: MouseButton::Left,
            x,
            y,
        }) => {
            controls.show_for_mouse_activity();
            if controls.play_pause_button_contains(*x, *y) {
                controls.toggle_paused();
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
    if let KeyEvent::Pressed {
        keycode: KeyCode::Space,
    } = event
    {
        controls.toggle_paused();
        paint_signal.notify();
        true
    } else {
        false
    }
}

fn draw_seek_bar(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    frame: &VideoFrameData,
    controls: &ControlsOverlay,
) {
    if !controls.is_visible() {
        return;
    }

    let Some((button_x, button_y)) = play_pause_button_origin(canvas_width, canvas_height) else {
        return;
    };
    let total_frames = frame.total_frames.max(frame.current_frame).max(1);
    let current_frame = frame.current_frame.min(total_frames);
    let panel_y = canvas_height.saturating_sub(CONTROLS_PANEL_HEIGHT);

    blend_rect_bgra(
        buffer,
        canvas_width,
        canvas_height,
        0,
        panel_y,
        canvas_width,
        canvas_height - panel_y,
        [0, 0, 0, 112],
    );

    if canvas_width < 180 {
        draw_play_pause_button(
            buffer,
            canvas_width,
            canvas_height,
            button_x,
            button_y,
            controls.is_paused(),
        );
        return;
    }

    let track_x = 80u32;
    let right_inset = 32u32.min(canvas_width / 8);
    let track_width = canvas_width.saturating_sub(track_x + right_inset).max(1);
    let track_height = 4;
    let track_y = canvas_height.saturating_sub(28);
    let progress_width =
        (u64::from(track_width) * u64::from(current_frame) / u64::from(total_frames)) as u32;
    let knob_x = track_x + progress_width.saturating_sub(1).min(track_width - 1);

    blend_rect_bgra(
        buffer,
        canvas_width,
        canvas_height,
        track_x,
        track_y,
        track_width,
        track_height,
        [88, 88, 88, 192],
    );
    draw_rect_bgra(
        buffer,
        canvas_width,
        canvas_height,
        track_x,
        track_y,
        progress_width,
        track_height,
        [238, 238, 238, 255],
    );
    draw_rect_bgra(
        buffer,
        canvas_width,
        canvas_height,
        knob_x.saturating_sub(3),
        track_y.saturating_sub(5),
        7,
        14,
        [255, 255, 255, 255],
    );
    draw_play_pause_button(
        buffer,
        canvas_width,
        canvas_height,
        button_x,
        button_y,
        controls.is_paused(),
    );
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

fn draw_play_pause_button(
    buffer: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    x: u32,
    y: u32,
    paused: bool,
) {
    blend_rect_bgra(
        buffer,
        canvas_width,
        canvas_height,
        x,
        y,
        32,
        32,
        [0, 0, 0, 96],
    );

    if paused {
        draw_play_triangle(
            buffer,
            canvas_width,
            canvas_height,
            x + 12,
            y + 8,
            [255, 255, 255, 255],
        );
    } else {
        draw_rect_bgra(
            buffer,
            canvas_width,
            canvas_height,
            x + 10,
            y + 8,
            4,
            16,
            [255, 255, 255, 255],
        );
        draw_rect_bgra(
            buffer,
            canvas_width,
            canvas_height,
            x + 18,
            y + 8,
            4,
            16,
            [255, 255, 255, 255],
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
) {
    const HEIGHT: u32 = 16;
    const WIDTH: u32 = 14;
    const MID: u32 = HEIGHT / 2;

    for row in 0..HEIGHT {
        let distance = row.abs_diff(MID);
        let width = 1 + (WIDTH - 1) * (MID.saturating_sub(distance)) / MID;
        let row_y = y + row;
        draw_rect_bgra(
            buffer,
            canvas_width,
            canvas_height,
            x,
            row_y,
            width,
            1,
            color,
        );
    }
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
    println!("[{}] playing {}", APP_NAME, video_path);
    if args.hardware_decode {
        println!("[{}] hardware decoder {}", APP_NAME, VVIDEO_DEVICE_PATH);
    }
    let mp4_data = if is_mp4_path(&video_path) {
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
    let audio_source = if let Some(path) = args.audio_path {
        println!("[{}] audio {}", APP_NAME, path);
        Some(PlayerAudioSource::Wav(path))
    } else if let Some(data) = &mp4_data {
        println!("[{}] audio MP4/AAC", APP_NAME);
        Some(PlayerAudioSource::Mp4Aac(data.clone()))
    } else {
        None
    };

    let mut app = VideoPlayerApp::new(video_path, mp4_data, audio_source, args.hardware_decode);
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
    audio_path: Option<String>,
    hardware_decode: bool,
}

fn parse_args(args: Vec<String>) -> PlayerArgs {
    let mut positional = Vec::new();
    let mut audio_path = None;
    let mut hardware_decode = false;

    let mut args = args.into_iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--hwdc" || arg == "--hwdec" {
            hardware_decode = true;
        } else if arg == "--software" || arg == "--swdec" {
            hardware_decode = false;
        } else if arg == "--audio" {
            audio_path = args.next();
        } else if let Some(path) = arg.strip_prefix("--audio=") {
            audio_path = Some(String::from(path));
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
        audio_path,
        hardware_decode,
    }
}
