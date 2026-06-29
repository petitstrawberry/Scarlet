//! Scarlet Audio Server (SAS).

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{Ordering, compiler_fence};
use core::time::Duration;

use sbus_client as sbus;
use std::audio::{
    AUDIO_DEVICE_KIND_SPEAKERS, AUDIO_PCM_FORMAT_S16LE, AudioDevice, AudioDeviceInfo,
    AudioPcmCapabilities, AudioPcmParams,
};
use std::handle::capability::memory_mapping::{flags as mmap_flags, prot};
use std::io::{Read, Write};
use std::ipc::{SharedMemory, permissions};
use std::println;
use std::socket::Socket;
use std::sync::Mutex;
use std::thread::{self, sleep};
use userprogram::sas_protocol as protocol;

const OUTPUT_RATE: u32 = 48_000;
const OUTPUT_CHANNELS: u16 = 2;
const OUTPUT_PERIOD_FRAMES: u32 = 480;
const OUTPUT_BUFFER_FRAMES: u32 = 9_600;
const OUTPUT_START_PREFILL_PERIODS: u32 = 4;
const MAX_OUTPUT_DEVICES: usize = 8;
const MIN_CLIENT_RATE: u32 = 8_000;
const MAX_CLIENT_RATE: u32 = 192_000;
const MAX_CLIENT_RING_FRAMES: usize = MAX_CLIENT_RATE as usize * 2;

struct ClientStream {
    shm: Option<SharedMemory>,
    ring_addr: Option<usize>,
    ring_size: usize,
    buffer_frames: usize,
    frame_bytes: usize,
    rate: u32,
    channels: u16,
    resample_pos_num: u128,
    gain: f32,
    configured: bool,
    closed: bool,
}

impl ClientStream {
    fn new() -> Self {
        Self {
            shm: None,
            ring_addr: None,
            ring_size: 0,
            buffer_frames: 0,
            frame_bytes: 0,
            rate: OUTPUT_RATE,
            channels: OUTPUT_CHANNELS,
            resample_pos_num: 0,
            gain: 1.0,
            configured: false,
            closed: false,
        }
    }
}

struct ServerState {
    clients: BTreeMap<usize, ClientStream>,
}

impl ServerState {
    fn new() -> Self {
        Self {
            clients: BTreeMap::new(),
        }
    }
}

struct OutputDevice {
    audio: AudioDevice,
    params: AudioPcmParams,
    ring: *mut u8,
    buffer_frames: usize,
    frame_bytes: usize,
    started: bool,
    dirty: bool,
}

// SAFETY: `OutputDevice` is moved into the audio thread before use and is not
// shared with other threads afterwards. The mapped ring pointer is only accessed
// by that owning thread.
unsafe impl Send for OutputDevice {}

struct MixPeriodResult {
    active: bool,
    pending: bool,
    draining: bool,
    has_clients: bool,
}

struct ClientMixResult {
    frames: usize,
    pending: bool,
    draining: bool,
}

struct AudioOutputCandidate {
    audio: AudioDevice,
    info: AudioDeviceInfo,
    caps: AudioPcmCapabilities,
    path: String,
}

impl OutputDevice {
    fn open() -> Result<Self, &'static str> {
        let output = select_audio_output()?;
        println!(
            "sas: selected {} kind={} name={} description={}",
            output.path,
            output.info.kind,
            fixed_str(&output.info.name),
            fixed_str(&output.info.description)
        );

        let params = AudioPcmParams {
            format: AUDIO_PCM_FORMAT_S16LE,
            rate: OUTPUT_RATE,
            channels: OUTPUT_CHANNELS,
            _reserved: 0,
            period_frames: OUTPUT_PERIOD_FRAMES
                .max(output.caps.min_period_frames)
                .min(output.caps.max_period_frames),
            buffer_frames: OUTPUT_BUFFER_FRAMES
                .max(output.caps.min_buffer_frames)
                .min(output.caps.max_buffer_frames),
        };
        println!(
            "sas: configuring {} S16LE {} Hz {}ch period={} buffer={}",
            output.path, params.rate, params.channels, params.period_frames, params.buffer_frames
        );
        output
            .audio
            .set_params(&params)
            .map_err(|_| "failed to configure audio output")?;
        println!("sas: mapping {} ring", output.path);
        let ring_info = output
            .audio
            .buffer_info()
            .map_err(|_| "failed to get audio ring info")?;
        let ring = output
            .audio
            .mmap_buffer(&ring_info)
            .map_err(|_| "failed to mmap audio ring")?;
        println!("sas: {} ready", output.path);

        Ok(Self {
            audio: output.audio,
            params,
            ring,
            buffer_frames: ring_info.buffer_frames as usize,
            frame_bytes: ring_info.frame_bytes as usize,
            started: false,
            dirty: false,
        })
    }

    fn start_prefill_frames(&self, force_start: bool) -> u64 {
        if force_start {
            return u64::from(self.params.period_frames);
        }

        u64::from(
            self.params
                .period_frames
                .saturating_mul(OUTPUT_START_PREFILL_PERIODS)
                .min(self.params.buffer_frames),
        )
    }

    fn write_period(&mut self, samples: &[i16], force_start: bool) -> Result<(), &'static str> {
        let period_frames = self.params.period_frames as usize;
        let period_bytes = period_frames * self.frame_bytes;
        let mut bytes = Vec::with_capacity(period_bytes);
        for sample in samples.iter() {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        loop {
            let status = self
                .audio
                .status()
                .map_err(|_| "failed to query audio status")?;
            if status.writable_frames as usize >= period_frames {
                unsafe {
                    write_ring_bytes(
                        self.ring,
                        self.buffer_frames,
                        self.frame_bytes,
                        status.app_ptr_frames,
                        &bytes,
                    );
                }
                self.audio
                    .commit_frames(self.params.period_frames)
                    .map_err(|_| "failed to commit audio frames")?;
                self.dirty = true;
                let queued_after_commit = status
                    .app_ptr_frames
                    .saturating_add(u64::from(self.params.period_frames))
                    .saturating_sub(status.hw_ptr_frames);
                if !self.started && queued_after_commit >= self.start_prefill_frames(force_start) {
                    self.audio.start().map_err(|_| "failed to start audio")?;
                    self.started = true;
                }
                return Ok(());
            }
            sleep(Duration::from_millis(1));
        }
    }

    fn is_started(&self) -> bool {
        self.started
    }

    fn stop_stream(&mut self) {
        if !self.started && !self.dirty {
            return;
        }
        let _ = self.audio.stop();
        self.started = false;
        self.dirty = false;
    }
}

fn select_audio_output() -> Result<AudioOutputCandidate, &'static str> {
    println!("sas: probing audio outputs");
    let mut fallback = None;

    for index in 0..MAX_OUTPUT_DEVICES {
        let path = format!("/dev/audio{}", index);
        let Ok(audio) = AudioDevice::open(&path) else {
            continue;
        };
        let info = audio.info().unwrap_or_default();
        let Ok(caps) = audio.capabilities() else {
            println!("sas: skipping {}: failed to query capabilities", path);
            continue;
        };
        if !supports_sas_output(&caps) {
            println!(
                "sas: skipping {}: unsupported S16LE {} Hz {}ch output",
                path, OUTPUT_RATE, OUTPUT_CHANNELS
            );
            continue;
        }

        let candidate = AudioOutputCandidate {
            audio,
            info,
            caps,
            path,
        };
        if candidate.info.kind == AUDIO_DEVICE_KIND_SPEAKERS {
            return Ok(candidate);
        }
        if fallback.is_none() {
            fallback = Some(candidate);
        }
    }

    fallback.ok_or("no compatible audio output found")
}

fn supports_sas_output(caps: &AudioPcmCapabilities) -> bool {
    caps.supports_format(AUDIO_PCM_FORMAT_S16LE)
        && caps.supports_rate(OUTPUT_RATE)
        && OUTPUT_CHANNELS >= caps.min_channels
        && OUTPUT_CHANNELS <= caps.max_channels
}

fn fixed_str(bytes: &[u8]) -> &str {
    let len = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..len]).unwrap_or("")
}

impl Drop for OutputDevice {
    fn drop(&mut self) {
        self.stop_stream();
        let _ = self.audio.release();
    }
}

trait MixerBackend {
    fn reset(&mut self, samples: usize);

    unsafe fn mix_s16le_ring(
        &mut self,
        stream: &ClientStream,
        ring_addr: usize,
        read_frames: u64,
        frames: usize,
        available_frames: usize,
    );

    fn finish_s16le(&self, out: &mut [i16]);
}

struct ScalarF32Mixer {
    acc: Vec<f32>,
}

impl ScalarF32Mixer {
    fn new() -> Self {
        Self { acc: Vec::new() }
    }
}

impl MixerBackend for ScalarF32Mixer {
    fn reset(&mut self, samples: usize) {
        if self.acc.len() != samples {
            self.acc.resize(samples, 0.0);
        } else {
            self.acc.fill(0.0);
        }
    }

    unsafe fn mix_s16le_ring(
        &mut self,
        stream: &ClientStream,
        ring_addr: usize,
        read_frames: u64,
        frames: usize,
        available_frames: usize,
    ) {
        let data = (ring_addr + protocol::RING_HEADER_SIZE) as *const u8;
        for frame in 0..frames {
            let src_pos_num = stream.resample_pos_num + frame as u128 * u128::from(stream.rate);
            let src_index = (src_pos_num / u128::from(OUTPUT_RATE)) as usize;
            let frac = (src_pos_num % u128::from(OUTPUT_RATE)) as u32;
            let next_index = (src_index + 1).min(available_frames - 1);
            let left = unsafe {
                interpolate_ring_s16(data, stream, read_frames, src_index, next_index, 0, frac)
            };
            let right_channel = if stream.channels > 1 { 1 } else { 0 };
            let right = unsafe {
                interpolate_ring_s16(
                    data,
                    stream,
                    read_frames,
                    src_index,
                    next_index,
                    right_channel,
                    frac,
                )
            };
            self.acc[frame * 2] += left * stream.gain;
            self.acc[frame * 2 + 1] += right * stream.gain;
        }
    }

    fn finish_s16le(&self, out: &mut [i16]) {
        for (dst, sample) in out.iter_mut().zip(self.acc.iter()) {
            *dst = f32_to_s16(*sample);
        }
    }
}

fn f32_to_s16(sample: f32) -> i16 {
    let sample = sample.clamp(-1.0, 1.0);
    if sample <= -1.0 {
        i16::MIN
    } else {
        (sample * i16::MAX as f32) as i16
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("=== Scarlet Audio Server (SAS) ===");

    let output = match OutputDevice::open() {
        Ok(output) => output,
        Err(e) => {
            println!("sas: audio output unavailable: {}", e);
            return 1;
        }
    };

    match sbus::Connection::connect() {
        Ok(mut conn) => {
            if let Err(e) = conn.register_service(protocol::SERVICE_NAME) {
                println!("sas: failed to register with sbus: {:?}", e);
            } else {
                println!("sas: registered with sbus as {}", protocol::SERVICE_NAME);
            }
        }
        Err(e) => println!("sas: continuing without sbus registration: {:?}", e),
    }

    let state = Arc::new(Mutex::new(ServerState::new()));
    let audio_state = state.clone();
    thread::spawn(move || audio_thread(audio_state, output));

    let server = match Socket::new() {
        Ok(socket) => socket,
        Err(e) => {
            println!("sas: failed to create socket: {:?}", e);
            return 1;
        }
    };
    if let Err(e) = server.bind(protocol::SOCKET_PATH) {
        println!("sas: failed to bind {}: {:?}", protocol::SOCKET_PATH, e);
        return 1;
    }
    if let Err(e) = server.listen(32) {
        println!("sas: failed to listen: {:?}", e);
        return 1;
    }

    println!("sas: listening on {}", protocol::SOCKET_PATH);
    let mut next_client_id = 1usize;
    loop {
        match server.accept() {
            Ok(socket) => {
                let client_id = next_client_id;
                next_client_id += 1;
                state.lock().clients.insert(client_id, ClientStream::new());
                let client_state = state.clone();
                thread::spawn(move || client_thread(client_id, socket, client_state));
            }
            Err(_) => {
                sleep(Duration::from_millis(10));
            }
        }
    }
}

fn audio_thread(state: Arc<Mutex<ServerState>>, mut output: OutputDevice) {
    println!(
        "sas: output configured S16LE {} Hz {}ch period={} buffer={}",
        output.params.rate,
        output.params.channels,
        output.params.period_frames,
        output.params.buffer_frames
    );

    let samples_per_period = output.params.period_frames as usize * output.params.channels as usize;
    let mut mixer = ScalarF32Mixer::new();
    let mut mixed = alloc::vec![0i16; samples_per_period];
    loop {
        let result = mix_period(&state, &mut mixer, &mut mixed);
        if result.active {
            if let Err(e) = output.write_period(&mixed, result.draining) {
                println!("sas: output error: {}", e);
                output.stop_stream();
                sleep(Duration::from_millis(20));
            }
        } else if result.pending {
            sleep(Duration::from_millis(1));
        } else if result.has_clients && output.is_started() {
            if let Err(e) = output.write_period(&mixed, true) {
                println!("sas: output error: {}", e);
                output.stop_stream();
                sleep(Duration::from_millis(20));
            }
        } else {
            if !result.has_clients {
                output.stop_stream();
            }
            sleep(Duration::from_millis(5));
        }
    }
}

fn mix_period(
    state: &Arc<Mutex<ServerState>>,
    mixer: &mut dyn MixerBackend,
    out: &mut [i16],
) -> MixPeriodResult {
    mixer.reset(out.len());
    let mut active = false;
    let mut pending = false;
    let mut draining = false;
    let mut has_clients = false;
    let mut to_remove = Vec::new();

    {
        let mut guard = state.lock();
        for (client_id, stream) in guard.clients.iter_mut() {
            if stream.closed {
                to_remove.push(*client_id);
                continue;
            }
            has_clients = true;

            let Some(ring_addr) = stream.ring_addr else {
                continue;
            };

            let result = unsafe { mix_client_ring(mixer, stream, ring_addr) };
            if result.frames != 0 {
                active = true;
            }
            pending |= result.pending;
            draining |= result.draining;
        }

        for client_id in to_remove {
            guard.clients.remove(&client_id);
        }
    }

    mixer.finish_s16le(out);
    MixPeriodResult {
        active,
        pending,
        draining,
        has_clients,
    }
}

unsafe fn mix_client_ring(
    mixer: &mut dyn MixerBackend,
    stream: &mut ClientStream,
    ring_addr: usize,
) -> ClientMixResult {
    let header = ring_addr as *mut protocol::RingHeader;
    // SAFETY: `ring_addr` is a server-side mapping of a SAS shared-memory ring.
    let read_frames = unsafe { core::ptr::addr_of!((*header).read_frames).read_volatile() };
    // SAFETY: `ring_addr` is a server-side mapping of a SAS shared-memory ring.
    let write_frames = unsafe { core::ptr::addr_of!((*header).write_frames).read_volatile() };
    compiler_fence(Ordering::Acquire);

    let available = write_frames.saturating_sub(read_frames) as usize;
    let draining = is_ring_draining(header);
    let frames = resampled_output_frames(stream, available);
    if frames == 0 {
        if draining && available != 0 {
            compiler_fence(Ordering::Release);
            // SAFETY: `ring_addr` is a server-side mapping of a SAS shared-memory ring.
            unsafe {
                core::ptr::addr_of_mut!((*header).read_frames).write_volatile(write_frames);
            }
            stream.resample_pos_num = 0;
        }
        return ClientMixResult {
            frames: 0,
            pending: available != 0 && !draining,
            draining,
        };
    }

    if frames < OUTPUT_PERIOD_FRAMES as usize && !draining {
        return ClientMixResult {
            frames: 0,
            pending: true,
            draining,
        };
    }

    // SAFETY: the ring was created by SAS and `frames` is bounded by readable
    // frames and one output period.
    unsafe {
        mixer.mix_s16le_ring(stream, ring_addr, read_frames, frames, available);
    }

    stream.resample_pos_num += frames as u128 * u128::from(stream.rate);
    let consumed = stream.resample_pos_num / u128::from(OUTPUT_RATE);
    stream.resample_pos_num %= u128::from(OUTPUT_RATE);
    compiler_fence(Ordering::Release);
    // SAFETY: `ring_addr` is a server-side mapping of a SAS shared-memory ring.
    unsafe {
        core::ptr::addr_of_mut!((*header).read_frames)
            .write_volatile(read_frames + consumed as u64);
    }
    ClientMixResult {
        frames,
        pending: false,
        draining,
    }
}

fn resampled_output_frames(stream: &ClientStream, available: usize) -> usize {
    if available == 0 {
        return 0;
    }
    let mut frames = 0usize;
    while frames < OUTPUT_PERIOD_FRAMES as usize {
        let src_pos_num = stream.resample_pos_num + frames as u128 * u128::from(stream.rate);
        let src_index = (src_pos_num / u128::from(OUTPUT_RATE)) as usize;
        if src_index >= available {
            break;
        }
        let next_frames = frames + 1;
        let consumed = (stream.resample_pos_num + next_frames as u128 * u128::from(stream.rate))
            / u128::from(OUTPUT_RATE);
        if consumed > available as u128 {
            break;
        }
        frames = next_frames;
    }
    frames
}

unsafe fn interpolate_ring_s16(
    data: *const u8,
    stream: &ClientStream,
    read_frames: u64,
    src_index: usize,
    next_index: usize,
    channel: u16,
    frac: u32,
) -> f32 {
    let a = unsafe { read_ring_s16(data, stream, read_frames, src_index, channel) } as i32;
    let b = unsafe { read_ring_s16(data, stream, read_frames, next_index, channel) } as i32;
    let mixed = (a * (OUTPUT_RATE - frac) as i32 + b * frac as i32) / OUTPUT_RATE as i32;
    mixed as f32 / 32768.0
}

unsafe fn read_ring_s16(
    data: *const u8,
    stream: &ClientStream,
    read_frames: u64,
    src_index: usize,
    channel: u16,
) -> i16 {
    let ring_frame = (read_frames as usize + src_index) % stream.buffer_frames;
    let sample_offset = ring_frame * stream.frame_bytes + channel as usize * 2;
    // SAFETY: sample offset is bounded by `buffer_frames * frame_bytes`.
    let lo = unsafe { data.add(sample_offset).read_volatile() };
    // SAFETY: sample offset is bounded by `buffer_frames * frame_bytes`.
    let hi = unsafe { data.add(sample_offset + 1).read_volatile() };
    i16::from_le_bytes([lo, hi])
}

fn is_ring_draining(header: *mut protocol::RingHeader) -> bool {
    // SAFETY: `header` points to a mapped SAS ring header.
    let flags = unsafe { core::ptr::addr_of!((*header).flags).read_volatile() };
    flags & protocol::RING_FLAG_DRAINING != 0
}

unsafe fn ring_is_empty(ring_addr: usize) -> bool {
    let header = ring_addr as *mut protocol::RingHeader;
    // SAFETY: `ring_addr` is a server-side mapping of a SAS shared-memory ring.
    let read_frames = unsafe { core::ptr::addr_of!((*header).read_frames).read_volatile() };
    // SAFETY: `ring_addr` is a server-side mapping of a SAS shared-memory ring.
    let write_frames = unsafe { core::ptr::addr_of!((*header).write_frames).read_volatile() };
    read_frames >= write_frames
}

fn client_thread(client_id: usize, mut socket: Socket, state: Arc<Mutex<ServerState>>) {
    println!("sas: client {} connected", client_id);
    loop {
        let (msg_type, payload) = match read_frame(&mut socket) {
            Ok(frame) => frame,
            Err(e) => {
                println!("sas: client {} disconnected: {}", client_id, e);
                mark_client_closed(client_id, &state);
                return;
            }
        };

        let result = match msg_type {
            protocol::MSG_CONFIGURE => handle_configure(client_id, &payload, &state, &mut socket)
                .or_else(|e| write_error(&mut socket, e)),
            protocol::MSG_DRAIN => {
                handle_drain(client_id, &state).and_then(|_| write_ok(&mut socket))
            }
            protocol::MSG_CLOSE => {
                mark_client_closed(client_id, &state);
                println!("sas: client {} disconnected: close requested", client_id);
                let _ = write_ok(&mut socket);
                return;
            }
            _ => write_error(&mut socket, "unsupported SAS message"),
        };
        if result.is_err() {
            mark_client_closed(client_id, &state);
            return;
        }
    }
}

fn handle_configure(
    client_id: usize,
    payload: &[u8],
    state: &Arc<Mutex<ServerState>>,
    socket: &mut Socket,
) -> Result<(), &'static str> {
    let config = protocol::Config::from_payload(payload).ok_or("invalid SAS config")?;
    if config.format != AUDIO_PCM_FORMAT_S16LE {
        return Err("SAS accepts only S16LE streams");
    }
    if !(MIN_CLIENT_RATE..=MAX_CLIENT_RATE).contains(&config.rate) {
        return Err("SAS stream sample rate is unsupported");
    }
    if config.channels == 0 || config.channels > OUTPUT_CHANNELS {
        return Err("SAS accepts only mono or stereo streams");
    }
    if config.period_frames == 0 || config.buffer_frames == 0 {
        return Err("SAS stream period/buffer size is invalid");
    }

    let frame_bytes = config.channels as usize * 2;
    let buffer_frames = config.buffer_frames as usize;
    let min_buffer_frames = (config.period_frames as usize)
        .checked_mul(4)
        .ok_or("SAS stream buffer size overflow")?;
    if buffer_frames < min_buffer_frames || buffer_frames > MAX_CLIENT_RING_FRAMES {
        return Err("SAS stream buffer size is unsupported");
    }
    let ring_data_bytes = buffer_frames
        .checked_mul(frame_bytes)
        .ok_or("SAS stream ring size overflow")?;
    let ring_size = protocol::RING_HEADER_SIZE
        .checked_add(ring_data_bytes)
        .ok_or("SAS stream ring size overflow")?;
    let shm = SharedMemory::create(ring_size, permissions::READ_WRITE)
        .map_err(|_| "failed to create SAS shared ring")?;
    let mapper = shm
        .as_handle()
        .as_memory_mapping()
        .map_err(|_| "SAS shared ring is not mappable")?;
    let ring_addr = mapper
        .mmap(
            0,
            ring_size,
            prot::READ | prot::WRITE,
            mmap_flags::SHARED,
            0,
        )
        .map_err(|_| "failed to map SAS shared ring")?;

    unsafe {
        init_ring_header(ring_addr, &config, buffer_frames as u32, frame_bytes as u32);
    }

    let mut guard = state.lock();
    let stream = guard
        .clients
        .get_mut(&client_id)
        .ok_or("unknown SAS client")?;
    stream.shm = Some(shm);
    stream.ring_addr = Some(ring_addr);
    stream.ring_size = ring_size;
    stream.buffer_frames = buffer_frames;
    stream.frame_bytes = frame_bytes;
    stream.rate = config.rate;
    stream.channels = config.channels;
    stream.resample_pos_num = 0;
    stream.configured = true;
    stream.closed = false;

    let shm = stream.shm.as_ref().ok_or("SAS shared ring missing")?;
    write_ok(socket)?;
    socket
        .send_handle(shm.as_handle())
        .map_err(|_| "failed to send SAS shared ring handle")?;
    Ok(())
}

fn handle_drain(client_id: usize, state: &Arc<Mutex<ServerState>>) -> Result<(), &'static str> {
    let ring_addr = {
        let guard = state.lock();
        let stream = guard.clients.get(&client_id).ok_or("unknown SAS client")?;
        if !stream.configured {
            return Err("SAS stream is not configured");
        }
        stream.ring_addr.ok_or("SAS stream has no shared ring")?
    };

    unsafe {
        set_ring_flag(ring_addr, protocol::RING_FLAG_DRAINING);
    }
    loop {
        if unsafe { ring_is_empty(ring_addr) } {
            return Ok(());
        }
        sleep(Duration::from_millis(5));
    }
}

fn mark_client_closed(client_id: usize, state: &Arc<Mutex<ServerState>>) {
    let mut guard = state.lock();
    if let Some(stream) = guard.clients.get_mut(&client_id) {
        stream.closed = true;
        if let Some(ring_addr) = stream.ring_addr {
            unsafe {
                set_ring_flag(ring_addr, protocol::RING_FLAG_CLOSED);
            }
        }
    }
}

unsafe fn init_ring_header(
    ring_addr: usize,
    config: &protocol::Config,
    buffer_frames: u32,
    frame_bytes: u32,
) {
    let header = ring_addr as *mut protocol::RingHeader;
    let value = protocol::RingHeader {
        magic: protocol::RING_MAGIC,
        version: protocol::RING_VERSION,
        format: config.format,
        rate: config.rate,
        channels: config.channels as u32,
        frame_bytes,
        period_frames: config.period_frames,
        buffer_frames,
        write_frames: 0,
        read_frames: 0,
        flags: 0,
        xrun_count: 0,
        reserved: [0; 8],
    };
    // SAFETY: `ring_addr` is a writable mapping of at least `RING_HEADER_SIZE`.
    unsafe {
        header.write_volatile(value);
        core::ptr::write_bytes(
            (ring_addr + protocol::RING_HEADER_SIZE) as *mut u8,
            0,
            buffer_frames as usize * frame_bytes as usize,
        );
    }
    compiler_fence(Ordering::Release);
}

unsafe fn set_ring_flag(ring_addr: usize, flag: u32) {
    let header = ring_addr as *mut protocol::RingHeader;
    // SAFETY: `ring_addr` is a mapped SAS ring header.
    let flags = unsafe { core::ptr::addr_of!((*header).flags).read_volatile() };
    // SAFETY: `ring_addr` is a mapped SAS ring header.
    unsafe {
        core::ptr::addr_of_mut!((*header).flags).write_volatile(flags | flag);
    }
}

fn read_frame(socket: &mut Socket) -> Result<(u32, Vec<u8>), &'static str> {
    let mut header_bytes = [0u8; protocol::HEADER_SIZE];
    read_exact(socket, &mut header_bytes)?;
    let header = protocol::Header::from_le_bytes(header_bytes);
    if header.payload_size as usize > protocol::MAX_PAYLOAD_SIZE {
        return Err("SAS payload too large");
    }

    let mut payload = alloc::vec![0u8; header.payload_size as usize];
    if !payload.is_empty() {
        read_exact(socket, &mut payload)?;
    }
    Ok((header.msg_type, payload))
}

fn read_exact(socket: &mut Socket, out: &mut [u8]) -> Result<(), &'static str> {
    let mut read = 0usize;
    while read < out.len() {
        match socket.read(&mut out[read..]) {
            Ok(0) => return Err("socket closed"),
            Ok(n) => read += n,
            Err(_) => {
                sleep(Duration::from_millis(1));
            }
        }
    }
    Ok(())
}

fn write_ok(socket: &mut Socket) -> Result<(), &'static str> {
    write_frame(socket, protocol::MSG_OK, &[])
}

fn write_error(socket: &mut Socket, message: &str) -> Result<(), &'static str> {
    write_frame(socket, protocol::MSG_ERROR, message.as_bytes())
}

fn write_frame(socket: &mut Socket, msg_type: u32, payload: &[u8]) -> Result<(), &'static str> {
    let frame = protocol::frame(msg_type, payload);
    write_all(socket, &frame)
}

fn write_all(socket: &mut Socket, bytes: &[u8]) -> Result<(), &'static str> {
    let mut written = 0usize;
    while written < bytes.len() {
        match socket.write(&bytes[written..]) {
            Ok(0) => return Err("socket closed"),
            Ok(n) => written += n,
            Err(_) => {
                sleep(Duration::from_millis(1));
            }
        }
    }
    socket.flush().map_err(|_| "socket flush failed")
}

unsafe fn write_ring_bytes(
    ring: *mut u8,
    buffer_frames: usize,
    frame_bytes: usize,
    start_frame: u64,
    data: &[u8],
) {
    let mut data_offset = 0usize;
    let mut frames_left = data.len() / frame_bytes;
    let mut current_frame = start_frame;
    while frames_left > 0 {
        let ring_frame = current_frame as usize % buffer_frames;
        let chunk_frames = frames_left.min(buffer_frames - ring_frame);
        let chunk_bytes = chunk_frames * frame_bytes;
        let ring_offset = ring_frame * frame_bytes;
        unsafe {
            core::ptr::copy_nonoverlapping(
                data[data_offset..data_offset + chunk_bytes].as_ptr(),
                ring.add(ring_offset),
                chunk_bytes,
            );
        }
        data_offset += chunk_bytes;
        frames_left -= chunk_frames;
        current_frame += chunk_frames as u64;
    }
}
