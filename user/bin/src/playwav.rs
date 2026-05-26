#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::sync::atomic::{Ordering, compiler_fence};
use core::time::Duration;
use std::audio::{AUDIO_PCM_FORMAT_S16LE, AudioDevice, AudioPcmParams};
use std::fs::File;
use std::handle::capability::memory_mapping::{flags as mmap_flags, prot};
use std::io::{Read, Write};
use std::ipc::SharedMemory;
use std::println;
use std::socket::Socket;
use std::string::String;
use std::thread::sleep;
use std::vec::Vec;
use userprogram::sas_protocol as protocol;

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 && !(args.len() == 3 && args[1] == "--sas") {
        println!("usage: playwav [--sas] FILE.wav");
        return 1;
    }

    let result = if args.len() == 3 {
        run_sas(&args[2])
    } else {
        run(&args[1])
    };

    match result {
        Ok(()) => 0,
        Err(message) => {
            println!("playwav: {}", message);
            1
        }
    }
}

fn run(path: &str) -> Result<(), &'static str> {
    let bytes = read_file(path)?;
    let wav = parse_wav(&bytes)?;
    if wav.audio_format != 1 || wav.bits_per_sample != 16 {
        return Err("only PCM S16LE WAV files are supported");
    }

    let audio = AudioDevice::open("/dev/audio0").map_err(|_| "failed to open /dev/audio0")?;
    let caps = audio
        .capabilities()
        .map_err(|_| "failed to query audio capabilities")?;
    if !caps.supports_format(AUDIO_PCM_FORMAT_S16LE) {
        return Err("audio device does not support PCM S16LE");
    }
    if !caps.supports_rate(wav.sample_rate) {
        return Err("audio device does not support WAV sample rate");
    }
    if wav.channels < caps.min_channels || wav.channels > caps.max_channels {
        return Err("audio device does not support WAV channel count");
    }
    let frame_bytes = wav.channels as usize * 2;
    let period_frames = (wav.sample_rate / 100)
        .max(caps.min_period_frames)
        .min(caps.max_period_frames);
    let min_periods = caps.min_buffer_frames.div_ceil(period_frames).max(1);
    let max_periods = (caps.max_buffer_frames / period_frames).max(min_periods);
    let buffer_frames = period_frames * 4u32.clamp(min_periods, max_periods);
    let params = AudioPcmParams {
        format: AUDIO_PCM_FORMAT_S16LE,
        rate: wav.sample_rate,
        channels: wav.channels,
        _reserved: 0,
        period_frames,
        buffer_frames,
    };
    audio
        .set_params(&params)
        .map_err(|_| "failed to configure audio device")?;
    let info = audio
        .buffer_info()
        .map_err(|_| "failed to get audio ring info")?;
    let ring = audio
        .mmap_buffer(&info)
        .map_err(|_| "failed to mmap audio ring")?;

    let data = &bytes[wav.data_offset..wav.data_offset + wav.data_len];
    let mut pos = 0usize;
    let mut started = false;

    while pos + frame_bytes <= data.len() {
        let status = audio.status().map_err(|_| "failed to get audio status")?;
        let frames_left = (data.len() - pos) / frame_bytes;
        let period_frames = params.period_frames as usize;
        let writable_frames = status.writable_frames as usize;
        let (frames, data_frames) = if frames_left <= period_frames {
            if writable_frames < period_frames {
                core::hint::spin_loop();
                continue;
            }
            (period_frames, frames_left)
        } else {
            let frames = core::cmp::min(writable_frames, frames_left);
            let frames = frames - frames % period_frames;
            if frames == 0 {
                core::hint::spin_loop();
                continue;
            }
            (frames, frames)
        };

        let byte_count = data_frames * frame_bytes;
        unsafe {
            write_ring_frames(
                ring,
                info.buffer_frames as usize,
                frame_bytes,
                status.app_ptr_frames,
                &data[pos..pos + byte_count],
                frames,
            );
        }
        audio
            .commit_frames(frames as u32)
            .map_err(|_| "failed to commit audio frames")?;
        pos += byte_count;

        if !started && status.app_ptr_frames + frames as u64 >= u64::from(params.period_frames) {
            audio.start().map_err(|_| "failed to start audio")?;
            started = true;
        }
    }

    if !started {
        audio.start().map_err(|_| "failed to start audio")?;
    }

    loop {
        let status = audio.status().map_err(|_| "failed to get audio status")?;
        if status.hw_ptr_frames >= status.app_ptr_frames {
            break;
        }
        core::hint::spin_loop();
    }

    let _ = audio.stop();
    let _ = audio.release();
    Ok(())
}

fn run_sas(path: &str) -> Result<(), &'static str> {
    let bytes = read_file(path)?;
    let wav = parse_wav(&bytes)?;
    if wav.audio_format != 1 || wav.bits_per_sample != 16 {
        return Err("SAS MVP accepts only PCM S16LE WAV files");
    }
    if wav.sample_rate != 48_000 || wav.channels != 2 {
        return Err("SAS MVP accepts only 48000 Hz stereo WAV files");
    }

    let mut socket = Socket::new().map_err(|_| "failed to create SAS socket")?;
    socket
        .connect(protocol::SOCKET_PATH)
        .map_err(|_| "failed to connect to SAS")?;

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
        .map_err(|_| "failed to receive SAS shared ring")?;
    let shm = SharedMemory::from_handle(shm_handle).map_err(|_| "invalid SAS shared ring")?;
    let ring_size = protocol::RING_HEADER_SIZE + config.buffer_frames as usize * 4;
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

    let data = &bytes[wav.data_offset..wav.data_offset + wav.data_len];
    let frame_bytes = wav.channels as usize * 2;
    write_sas_ring(ring_addr, data, frame_bytes)?;

    write_sas_frame(&mut socket, protocol::MSG_DRAIN, &[])?;
    read_sas_ok(&mut socket)?;
    let _ = write_sas_frame(&mut socket, protocol::MSG_CLOSE, &[]);
    Ok(())
}

fn write_sas_ring(ring_addr: usize, data: &[u8], frame_bytes: usize) -> Result<(), &'static str> {
    let header = ring_addr as *mut protocol::RingHeader;
    let data_ptr = (ring_addr + protocol::RING_HEADER_SIZE) as *mut u8;
    let buffer_frames = unsafe {
        // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
        core::ptr::addr_of!((*header).buffer_frames).read_volatile() as usize
    };
    if buffer_frames == 0 || data.len() % frame_bytes != 0 {
        return Err("invalid SAS shared ring");
    }

    let mut written_data_frames = 0usize;
    let total_data_frames = data.len() / frame_bytes;
    while written_data_frames < total_data_frames {
        let read_frames = unsafe {
            // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
            core::ptr::addr_of!((*header).read_frames).read_volatile()
        };
        let write_frames = unsafe {
            // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
            core::ptr::addr_of!((*header).write_frames).read_volatile()
        };
        let queued_frames = write_frames.saturating_sub(read_frames) as usize;
        let writable_frames = buffer_frames.saturating_sub(queued_frames);
        if writable_frames == 0 {
            sleep(Duration::from_millis(2));
            continue;
        }

        let frames = writable_frames.min(total_data_frames - written_data_frames);
        let ring_frame = write_frames as usize % buffer_frames;
        let contiguous_frames = frames.min(buffer_frames - ring_frame);
        let data_offset = written_data_frames * frame_bytes;
        let ring_offset = ring_frame * frame_bytes;
        let bytes = contiguous_frames * frame_bytes;
        unsafe {
            // SAFETY: both source and destination ranges are bounded by the WAV
            // data slice and SAS shared ring size.
            core::ptr::copy_nonoverlapping(
                data[data_offset..].as_ptr(),
                data_ptr.add(ring_offset),
                bytes,
            );
        }

        let remaining_frames = frames - contiguous_frames;
        if remaining_frames != 0 {
            let second_offset = data_offset + bytes;
            let second_bytes = remaining_frames * frame_bytes;
            unsafe {
                // SAFETY: wrap copy writes from the start of the same ring.
                core::ptr::copy_nonoverlapping(
                    data[second_offset..].as_ptr(),
                    data_ptr,
                    second_bytes,
                );
            }
        }

        compiler_fence(Ordering::Release);
        unsafe {
            // SAFETY: `ring_addr` is a mapped SAS ring header received from SAS.
            core::ptr::addr_of_mut!((*header).write_frames)
                .write_volatile(write_frames + frames as u64);
        }
        written_data_frames += frames;
    }
    Ok(())
}

unsafe fn write_ring_frames(
    ring: *mut u8,
    buffer_frames: usize,
    frame_bytes: usize,
    mut start_frame: u64,
    data: &[u8],
    frames: usize,
) {
    let mut data_offset = 0usize;
    let mut frames_left = frames;
    while frames_left > 0 {
        let ring_frame = start_frame as usize % buffer_frames;
        let chunk_frames = core::cmp::min(frames_left, buffer_frames - ring_frame);
        let chunk_bytes = chunk_frames * frame_bytes;
        let ring_offset = ring_frame * frame_bytes;
        let copy_bytes = core::cmp::min(data.len() - data_offset, chunk_bytes);
        if copy_bytes > 0 {
            // SAFETY: caller provides a valid mmap pointer and `copy_bytes` is
            // limited to the ring span and input data.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    data[data_offset..].as_ptr(),
                    ring.add(ring_offset),
                    copy_bytes,
                );
            }
        }
        if copy_bytes < chunk_bytes {
            // SAFETY: the remaining chunk is inside the mapped ring and is used
            // as tail padding so the backend receives a complete period.
            unsafe {
                core::ptr::write_bytes(
                    ring.add(ring_offset + copy_bytes),
                    0,
                    chunk_bytes - copy_bytes,
                );
            }
        }
        data_offset += copy_bytes;
        frames_left -= chunk_frames;
        start_frame += chunk_frames as u64;
    }
}

fn read_file(path: &str) -> Result<Vec<u8>, &'static str> {
    let mut file = File::open(path).map_err(|_| "failed to open input file")?;
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|_| "failed to read input file")?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
    }
    Ok(out)
}

fn write_sas_frame(socket: &mut Socket, msg_type: u32, payload: &[u8]) -> Result<(), &'static str> {
    let frame = protocol::frame(msg_type, payload);
    write_all(socket, &frame)
}

fn read_sas_ok(socket: &mut Socket) -> Result<(), &'static str> {
    let mut header_bytes = [0u8; protocol::HEADER_SIZE];
    read_exact(socket, &mut header_bytes)?;
    let header = protocol::Header::from_le_bytes(header_bytes);
    if header.payload_size as usize > protocol::MAX_PAYLOAD_SIZE {
        return Err("SAS response too large");
    }

    let mut payload = Vec::new();
    payload.resize(header.payload_size as usize, 0);
    if !payload.is_empty() {
        read_exact(socket, &mut payload)?;
    }

    match header.msg_type {
        protocol::MSG_OK => Ok(()),
        protocol::MSG_ERROR => Err("SAS returned an error"),
        _ => Err("unexpected SAS response"),
    }
}

fn read_exact(socket: &mut Socket, out: &mut [u8]) -> Result<(), &'static str> {
    let mut read = 0usize;
    while read < out.len() {
        match socket.read(&mut out[read..]) {
            Ok(0) => return Err("SAS socket closed"),
            Ok(n) => read += n,
            Err(_) => core::hint::spin_loop(),
        }
    }
    Ok(())
}

fn write_all(socket: &mut Socket, bytes: &[u8]) -> Result<(), &'static str> {
    let mut written = 0usize;
    while written < bytes.len() {
        match socket.write(&bytes[written..]) {
            Ok(0) => return Err("SAS socket closed"),
            Ok(n) => written += n,
            Err(_) => core::hint::spin_loop(),
        }
    }
    socket.flush().map_err(|_| "failed to flush SAS socket")
}

struct WavInfo {
    audio_format: u16,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    data_offset: usize,
    data_len: usize,
}

fn parse_wav(bytes: &[u8]) -> Result<WavInfo, &'static str> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file");
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
            return Err("truncated WAV chunk");
        }

        if id == b"fmt " {
            if len < 16 {
                return Err("invalid WAV fmt chunk");
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
        data_offset: data_offset.ok_or("WAV data chunk not found")?,
        data_len,
    })
}

fn read_u16_le(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn read_u32_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}
