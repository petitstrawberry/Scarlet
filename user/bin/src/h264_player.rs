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
    Application, CanvasView, ComponentElement, Element, InvalidationKind, Listenable, Size,
    SubscriptionId, View, Window,
};
use std::audio::AUDIO_PCM_FORMAT_S16LE;
use std::fs::File;
use std::handle::capability::memory_mapping::{flags as mmap_flags, prot};
use std::io::{Read, Write};
use std::ipc::SharedMemory;
use std::socket::Socket;
use std::sync::Mutex;
use std::{format, println, thread};
use userprogram::sas_protocol as protocol;

const DEFAULT_VIDEO_PATH: &str = "/root/media/bad_apple.h264";
const DEFAULT_AUDIO_PATH: &str = "/root/media/bad_apple.wav";
const VIDEO_WIDTH: u32 = 480;
const VIDEO_HEIGHT: u32 = 360;
const DISPLAY_WIDTH: u32 = 640;
const DISPLAY_HEIGHT: u32 = 360;
const FRAME_INTERVAL_MS: u64 = 33;

struct VideoFrameStore {
    data: Mutex<VideoFrameData>,
}

struct VideoFrameData {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

impl VideoFrameStore {
    fn new() -> Self {
        Self {
            data: Mutex::new(VideoFrameData {
                pixels: vec![0; (VIDEO_WIDTH * VIDEO_HEIGHT * 4) as usize],
                width: VIDEO_WIDTH,
                height: VIDEO_HEIGHT,
            }),
        }
    }

    fn update_from_frame(&self, frame: &Frame) {
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
struct H264PlayerApp {
    path: String,
    audio_path: Option<String>,
    frame_store: Arc<VideoFrameStore>,
    paint_signal: Arc<PaintSignal>,
    clock: Arc<AudioClock>,
}

impl H264PlayerApp {
    fn new(path: String, audio_path: Option<String>) -> Self {
        Self {
            path,
            audio_path,
            frame_store: Arc::new(VideoFrameStore::new()),
            paint_signal: Arc::new(PaintSignal::new()),
            clock: Arc::new(AudioClock::new()),
        }
    }
}

struct AudioClock {
    started: AtomicBool,
    sample_rate: AtomicU64,
    read_frames: AtomicU64,
}

impl AudioClock {
    fn new() -> Self {
        Self {
            started: AtomicBool::new(false),
            sample_rate: AtomicU64::new(48_000),
            read_frames: AtomicU64::new(0),
        }
    }

    fn mark_started(&self, sample_rate: u32) {
        self.sample_rate
            .store(u64::from(sample_rate), Ordering::Release);
        self.started.store(true, Ordering::Release);
    }

    fn update_read_frames(&self, read_frames: u64) {
        self.read_frames.store(read_frames, Ordering::Release);
    }

    fn video_frame_index(&self) -> Option<usize> {
        if !self.started.load(Ordering::Acquire) {
            return None;
        }
        let rate = self.sample_rate.load(Ordering::Acquire).max(1);
        let audio_frames = self.read_frames.load(Ordering::Acquire);
        Some((audio_frames.saturating_mul(30) / rate) as usize)
    }
}

impl View for H264PlayerApp {
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

impl Application for H264PlayerApp {
    fn body(&self) -> impl View {
        let frame_store = self.frame_store.clone();
        Window::new(
            "H.264 Player",
            CanvasView::new(
                DISPLAY_WIDTH as f32,
                DISPLAY_HEIGHT as f32,
                Rc::new(move |buffer, width, height| {
                    draw_video_frame(buffer, width, height, &frame_store);
                }),
            ),
        )
        .app_id("org.scarlet-os.h264-player")
        .size(Size::new(DISPLAY_WIDTH as f32, DISPLAY_HEIGHT as f32))
    }

    fn init(&mut self) {
        if let Some(audio_path) = self.audio_path.clone() {
            start_audio_thread(audio_path, self.clock.clone());
        }
        start_decoder_thread(
            self.path.clone(),
            self.frame_store.clone(),
            self.paint_signal.clone(),
            self.audio_path.is_some().then(|| self.clock.clone()),
        );
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn start_decoder_thread(
    path: String,
    frame_store: Arc<VideoFrameStore>,
    paint_signal: Arc<PaintSignal>,
    clock: Option<Arc<AudioClock>>,
) {
    thread::spawn(move || {
        if let Err(err) = decode_loop(&path, &frame_store, &paint_signal, clock.as_deref()) {
            println!("[h264_player] {}", err);
        }
    });
}

fn decode_loop(
    path: &str,
    frame_store: &VideoFrameStore,
    paint_signal: &PaintSignal,
    clock: Option<&AudioClock>,
) -> Result<(), String> {
    let data = read_file(path)?;
    let nals = parse_annex_b(&data);
    let mut decoder = Decoder::new();
    let mut display_index = 0usize;
    println!("[h264_player] {} NAL units", nals.len());

    for nal in &nals {
        match decoder.decode_nal(nal) {
            Ok(Some(frame)) => {
                publish_frame_synced(frame_store, paint_signal, frame, display_index, clock);
                display_index += 1;
            }
            Ok(None) => {}
            Err(err) => return Err(format!("decode failed: {err}")),
        }
    }

    if let Some(frame) = decoder.flush() {
        publish_frame_synced(frame_store, paint_signal, frame, display_index, clock);
        display_index += 1;
    }

    println!("[h264_player] finished: {} frames", display_index);

    Ok(())
}

fn start_audio_thread(path: String, clock: Arc<AudioClock>) {
    thread::spawn(move || {
        if let Err(err) = play_wav_sas(&path, &clock) {
            println!("[h264_player] audio: {}", err);
        }
    });
}

fn play_wav_sas(path: &str, clock: &AudioClock) -> Result<(), String> {
    let bytes = read_file(path)?;
    let wav = parse_wav(&bytes)?;
    if wav.audio_format != 1 || wav.bits_per_sample != 16 {
        return Err(String::from("SAS accepts only PCM S16LE WAV files"));
    }
    if wav.sample_rate != 48_000 || wav.channels != 2 {
        return Err(String::from("SAS accepts only 48000 Hz stereo WAV files"));
    }

    let mut socket = Socket::new().map_err(|_| format!("failed to create SAS socket"))?;
    socket
        .connect(protocol::SOCKET_PATH)
        .map_err(|_| format!("failed to connect to SAS"))?;

    let config = protocol::Config {
        format: AUDIO_PCM_FORMAT_S16LE,
        rate: wav.sample_rate,
        channels: wav.channels,
        reserved: 0,
        period_frames: 480,
        buffer_frames: 1_920,
    };
    write_sas_frame(&mut socket, protocol::MSG_CONFIGURE, &config.to_le_bytes())?;
    read_sas_ok(&mut socket)?;
    let shm_handle = socket
        .recv_handle()
        .map_err(|_| format!("failed to receive SAS shared ring"))?;
    let shm = SharedMemory::from_handle(shm_handle).map_err(|_| format!("invalid SAS ring"))?;
    let ring_size = protocol::RING_HEADER_SIZE + config.buffer_frames as usize * 4;
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

    let data = &bytes[wav.data_offset..wav.data_offset + wav.data_len];
    let frame_bytes = wav.channels as usize * 2;
    let total_data_frames = data.len() / frame_bytes;
    let mut pos_frame = 0usize;
    clock.mark_started(wav.sample_rate);

    while pos_frame < total_data_frames {
        // SAFETY: `ring_addr` is the SAS shared ring mmap returned by SAS.
        clock.update_read_frames(unsafe { sas_ring_read_frames(ring_addr) });
        // SAFETY: `ring_addr` is the SAS shared ring mmap returned by SAS.
        let frames = unsafe { writable_sas_frames(ring_addr).min(total_data_frames - pos_frame) };
        if frames == 0 {
            thread::sleep(Duration::from_millis(2));
            continue;
        }

        let data_offset = pos_frame * frame_bytes;
        // SAFETY: `frames` is bounded by SAS writable space and source data length.
        unsafe {
            write_sas_ring_chunk(
                ring_addr,
                &data[data_offset..data_offset + frames * frame_bytes],
                frame_bytes,
                frames,
            );
        }
        pos_frame += frames;
    }

    write_sas_frame(&mut socket, protocol::MSG_DRAIN, &[])?;
    // SAFETY: `ring_addr` remains mapped until playback cleanup completes.
    while !unsafe { sas_ring_is_empty(ring_addr) } {
        // SAFETY: `ring_addr` remains mapped until playback cleanup completes.
        clock.update_read_frames(unsafe { sas_ring_read_frames(ring_addr) });
        thread::sleep(Duration::from_millis(10));
    }
    read_sas_ok(&mut socket)?;
    let _ = write_sas_frame(&mut socket, protocol::MSG_CLOSE, &[]);
    Ok(())
}

fn publish_frame_synced(
    frame_store: &VideoFrameStore,
    paint_signal: &PaintSignal,
    frame: Frame,
    display_index: usize,
    clock: Option<&AudioClock>,
) {
    if let Some(clock) = clock {
        loop {
            let Some(target_index) = clock.video_frame_index() else {
                thread::sleep(Duration::from_millis(1));
                continue;
            };
            if target_index >= display_index {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
    } else {
        thread::sleep(Duration::from_millis(FRAME_INTERVAL_MS));
    }

    publish_frame(frame_store, paint_signal, frame);
}

fn publish_frame(frame_store: &VideoFrameStore, paint_signal: &PaintSignal, frame: Frame) {
    frame_store.update_from_frame(&frame);
    paint_signal.notify();
}

fn read_file(path: &str) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|_| format!("open failed"))?;
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

fn yuv420_to_bgra(frame: &Frame, pixels: &mut [u8]) {
    yuv420_to_bgra_simd(frame, pixels);
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
) {
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
    let args: Vec<String> = std::env::args().collect();
    let video_path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| String::from(DEFAULT_VIDEO_PATH));
    let audio_path = args
        .get(2)
        .cloned()
        .or_else(|| default_audio_path_for(&video_path));
    println!("[h264_player] playing {}", video_path);
    if let Some(path) = audio_path.as_ref() {
        println!("[h264_player] audio {}", path);
    }

    let mut app = H264PlayerApp::new(video_path, audio_path);
    match app.run() {
        Ok(()) => 0,
        Err(error) => {
            println!("[h264_player] Application error: {}", error);
            1
        }
    }
}

fn default_audio_path_for(video_path: &str) -> Option<String> {
    if video_path == DEFAULT_VIDEO_PATH {
        return Some(String::from(DEFAULT_AUDIO_PATH));
    }

    let stem = video_path.strip_suffix(".h264")?;
    let mut audio_path = String::from(stem);
    audio_path.push_str(".wav");
    Some(audio_path)
}
