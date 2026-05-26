#![no_std]
#![no_main]

extern crate scarlet_std as std;

use std::audio::{AUDIO_PCM_FORMAT_S16LE, AudioDevice, AudioPcmParams};
use std::fs::File;
use std::println;
use std::string::String;
use std::vec::Vec;

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        println!("usage: playwav FILE.wav");
        return 1;
    }

    match run(&args[1]) {
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
    let frame_bytes = wav.channels as usize * 2;
    let period_frames = (wav.sample_rate / 100).max(64);
    let params = AudioPcmParams {
        format: AUDIO_PCM_FORMAT_S16LE,
        rate: wav.sample_rate,
        channels: wav.channels,
        _reserved: 0,
        period_frames,
        buffer_frames: period_frames * 4,
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
