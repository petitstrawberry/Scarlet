//! Scarlet Audio Server (SAS).

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{Ordering, compiler_fence};
use core::time::Duration;

use sbus_client as sbus;
use std::audio::{AUDIO_PCM_FORMAT_S16LE, AudioDevice, AudioPcmParams};
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
const OUTPUT_BUFFER_FRAMES: u32 = 1_920;
const MAX_CLIENT_RING_FRAMES: usize = OUTPUT_RATE as usize * 2;

struct ClientStream {
    shm: Option<SharedMemory>,
    ring_addr: Option<usize>,
    ring_size: usize,
    buffer_frames: usize,
    frame_bytes: usize,
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
}

impl OutputDevice {
    fn open() -> Result<Self, &'static str> {
        let audio = AudioDevice::open("/dev/audio0").map_err(|_| "failed to open /dev/audio0")?;
        let caps = audio
            .capabilities()
            .map_err(|_| "failed to query audio capabilities")?;
        if !caps.supports_format(AUDIO_PCM_FORMAT_S16LE) || !caps.supports_rate(OUTPUT_RATE) {
            return Err("audio0 does not support SAS output format");
        }
        if OUTPUT_CHANNELS < caps.min_channels || OUTPUT_CHANNELS > caps.max_channels {
            return Err("audio0 does not support SAS output channels");
        }

        let params = AudioPcmParams {
            format: AUDIO_PCM_FORMAT_S16LE,
            rate: OUTPUT_RATE,
            channels: OUTPUT_CHANNELS,
            _reserved: 0,
            period_frames: OUTPUT_PERIOD_FRAMES
                .max(caps.min_period_frames)
                .min(caps.max_period_frames),
            buffer_frames: OUTPUT_BUFFER_FRAMES
                .max(caps.min_buffer_frames)
                .min(caps.max_buffer_frames),
        };
        audio
            .set_params(&params)
            .map_err(|_| "failed to configure audio0")?;
        let info = audio
            .buffer_info()
            .map_err(|_| "failed to get audio ring info")?;
        let ring = audio
            .mmap_buffer(&info)
            .map_err(|_| "failed to mmap audio ring")?;

        Ok(Self {
            audio,
            params,
            ring,
            buffer_frames: info.buffer_frames as usize,
            frame_bytes: info.frame_bytes as usize,
            started: false,
        })
    }

    fn write_period(&mut self, samples: &[i16]) -> Result<(), &'static str> {
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
                if !self.started {
                    self.audio.start().map_err(|_| "failed to start audio")?;
                    self.started = true;
                }
                return Ok(());
            }
            sleep(Duration::from_millis(1));
        }
    }

    fn stop_if_drained(&mut self) {
        if !self.started {
            return;
        }
        if let Ok(status) = self.audio.status()
            && status.hw_ptr_frames >= status.app_ptr_frames
        {
            let _ = self.audio.stop();
            self.started = false;
        }
    }
}

impl Drop for OutputDevice {
    fn drop(&mut self) {
        let _ = self.audio.stop();
        let _ = self.audio.release();
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    println!("=== Scarlet Audio Server (SAS) ===");

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
    thread::spawn(move || audio_thread(audio_state));

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

fn audio_thread(state: Arc<Mutex<ServerState>>) {
    let mut output = match OutputDevice::open() {
        Ok(output) => output,
        Err(e) => {
            println!("sas: audio output unavailable: {}", e);
            return;
        }
    };
    println!(
        "sas: output configured S16LE {} Hz {}ch period={} buffer={}",
        output.params.rate,
        output.params.channels,
        output.params.period_frames,
        output.params.buffer_frames
    );

    let samples_per_period = output.params.period_frames as usize * output.params.channels as usize;
    let mut idle_ticks = 0usize;
    loop {
        let mut mixed = alloc::vec![0i16; samples_per_period];
        let active = mix_period(&state, &mut mixed);
        if active {
            idle_ticks = 0;
            if let Err(e) = output.write_period(&mixed) {
                println!("sas: output error: {}", e);
                sleep(Duration::from_millis(20));
            }
        } else {
            idle_ticks = idle_ticks.saturating_add(1);
            if idle_ticks >= 50 {
                output.stop_if_drained();
                idle_ticks = 0;
            }
            sleep(Duration::from_millis(5));
        }
    }
}

fn mix_period(state: &Arc<Mutex<ServerState>>, out: &mut [i16]) -> bool {
    let mut acc = alloc::vec![0i32; out.len()];
    let mut active = false;
    let mut to_remove = Vec::new();

    {
        let mut guard = state.lock();
        for (client_id, stream) in guard.clients.iter_mut() {
            let Some(ring_addr) = stream.ring_addr else {
                if stream.closed {
                    to_remove.push(*client_id);
                }
                continue;
            };

            let frames = unsafe { mix_client_ring(stream, ring_addr, &mut acc) };
            if frames != 0 {
                active = true;
            }
            if stream.closed && frames == 0 && unsafe { ring_is_empty(ring_addr) } {
                to_remove.push(*client_id);
            }
        }

        for client_id in to_remove {
            guard.clients.remove(&client_id);
        }
    }

    for (dst, sample) in out.iter_mut().zip(acc.iter()) {
        *dst = (*sample).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    }
    active
}

unsafe fn mix_client_ring(stream: &ClientStream, ring_addr: usize, acc: &mut [i32]) -> usize {
    let header = ring_addr as *mut protocol::RingHeader;
    // SAFETY: `ring_addr` is a server-side mapping of a SAS shared-memory ring.
    let read_frames = unsafe { core::ptr::addr_of!((*header).read_frames).read_volatile() };
    // SAFETY: `ring_addr` is a server-side mapping of a SAS shared-memory ring.
    let write_frames = unsafe { core::ptr::addr_of!((*header).write_frames).read_volatile() };
    compiler_fence(Ordering::Acquire);

    let available = write_frames.saturating_sub(read_frames) as usize;
    let frames = available.min(OUTPUT_PERIOD_FRAMES as usize);
    if frames == 0 {
        return 0;
    }

    let channels = OUTPUT_CHANNELS as usize;
    let data = (ring_addr + protocol::RING_HEADER_SIZE) as *const u8;
    for frame in 0..frames {
        let ring_frame = (read_frames as usize + frame) % stream.buffer_frames;
        let frame_offset = ring_frame * stream.frame_bytes;
        for channel in 0..channels {
            let sample_offset = frame_offset + channel * 2;
            // SAFETY: sample offset is bounded by `buffer_frames * frame_bytes`.
            let lo = unsafe { data.add(sample_offset).read_volatile() };
            // SAFETY: sample offset is bounded by `buffer_frames * frame_bytes`.
            let hi = unsafe { data.add(sample_offset + 1).read_volatile() };
            let sample = i16::from_le_bytes([lo, hi]);
            let acc_index = frame * channels + channel;
            acc[acc_index] = acc[acc_index].saturating_add(sample as i32);
        }
    }

    compiler_fence(Ordering::Release);
    // SAFETY: `ring_addr` is a server-side mapping of a SAS shared-memory ring.
    unsafe {
        core::ptr::addr_of_mut!((*header).read_frames).write_volatile(read_frames + frames as u64);
    }
    frames
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
    if config.format != AUDIO_PCM_FORMAT_S16LE
        || config.rate != OUTPUT_RATE
        || config.channels != OUTPUT_CHANNELS
    {
        return Err("SAS MVP accepts only S16LE 48000 Hz stereo streams");
    }

    let frame_bytes = config.channels as usize * 2;
    let buffer_frames = (config.buffer_frames as usize)
        .max(OUTPUT_BUFFER_FRAMES as usize)
        .min(MAX_CLIENT_RING_FRAMES);
    let ring_size = protocol::RING_HEADER_SIZE + buffer_frames * frame_bytes;
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
