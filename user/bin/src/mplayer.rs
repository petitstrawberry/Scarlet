#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use sas_client::{SasClient, StreamConfig};
use std::audio::{AUDIO_PCM_FORMAT_S16LE, AudioDevice, AudioPcmParams};
use std::fs::{File, OpenOptions};
use std::string::String;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, sleep};
use std::tty::{ReadPolicy, Terminal, TerminalSettings};
use std::vec::Vec;
use std::{print, println};

struct Options {
    use_sas: bool,
    repeat: bool,
    info_only: bool,
    seek_ms: u64,
    files: Vec<String>,
}

#[derive(Clone, Copy)]
enum PlayerCommand {
    Next,
    Previous,
    Quit,
    SeekRelative(i64),
    TogglePause,
    Help,
}

enum PlaybackAction {
    Finished,
    Next,
    Previous,
    Quit,
    SeekTo(u64),
    TogglePause,
}

struct Controls {
    commands: Arc<Mutex<Vec<PlayerCommand>>>,
    running: Arc<AtomicBool>,
    restore_tty: Option<File>,
    saved_settings: Option<TerminalSettings>,
    input_thread: Option<JoinHandle>,
}

impl Controls {
    fn open() -> Self {
        let mut tty = match OpenOptions::new().read(true).open("/dev/tty0") {
            Ok(file) => file,
            Err(_) => {
                return Self {
                    commands: Arc::new(Mutex::new(Vec::new())),
                    running: Arc::new(AtomicBool::new(false)),
                    restore_tty: None,
                    saved_settings: None,
                    input_thread: None,
                };
            }
        };

        let terminal = Terminal::from_file(&tty);
        let saved_settings = terminal.settings().ok();
        let _ = terminal.set_canonical(false);
        let _ = terminal.set_echo(false);
        let _ = terminal.set_read_policy(ReadPolicy::new(1, 100));
        let _ = terminal.flush_input();
        let restore_tty = tty
            .clone_handle()
            .ok()
            .and_then(|handle| File::from_handle(handle).ok());
        let commands = Arc::new(Mutex::new(Vec::new()));
        let thread_commands = commands.clone();
        let running = Arc::new(AtomicBool::new(true));
        let thread_running = running.clone();

        let input_thread = thread::spawn(move || {
            let mut buf = [0u8; 8];
            while thread_running.load(Ordering::Acquire) {
                match tty.read(&mut buf) {
                    Ok(0) => {}
                    Err(_) => {
                        sleep(Duration::from_millis(10));
                    }
                    Ok(n) => {
                        let mut pending = thread_commands.lock();
                        for byte in &buf[..n] {
                            if let Some(command) = command_from_byte(*byte) {
                                pending.push(command);
                            }
                        }
                    }
                }
            }
        });

        Self {
            commands,
            running,
            restore_tty,
            saved_settings,
            input_thread: Some(input_thread),
        }
    }

    fn next_command(&self) -> Option<PlayerCommand> {
        let mut commands = self.commands.lock();
        if commands.is_empty() {
            None
        } else {
            Some(commands.remove(0))
        }
    }

    fn print_help() {
        println!("controls: Space=play/pause n=next p=previous j=-10s l=+10s q=quit ?=help");
    }
}

impl Drop for Controls {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(tty) = self.restore_tty.as_mut() {
            let terminal = Terminal::from_file(tty);
            let _ = terminal.set_read_policy(ReadPolicy::new(1, 1));
        }
        if let Some(input_thread) = self.input_thread.take() {
            let _ = input_thread.join();
        }
        if let Some(tty) = self.restore_tty.as_mut() {
            if let Some(settings) = self.saved_settings {
                let terminal = Terminal::from_file(tty);
                let _ = terminal.apply_settings(settings);
            }
        }
    }
}

fn command_from_byte(byte: u8) -> Option<PlayerCommand> {
    match byte as char {
        'n' | 'N' => Some(PlayerCommand::Next),
        'p' | 'P' => Some(PlayerCommand::Previous),
        'q' | 'Q' => Some(PlayerCommand::Quit),
        'j' | 'J' => Some(PlayerCommand::SeekRelative(-10_000)),
        'l' | 'L' => Some(PlayerCommand::SeekRelative(10_000)),
        ' ' => Some(PlayerCommand::TogglePause),
        '?' => Some(PlayerCommand::Help),
        _ => None,
    }
}

#[unsafe(no_mangle)]
fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let options = match parse_options(&args) {
        Ok(options) => options,
        Err(message) => {
            println!("mplayer: {}", message);
            print_usage();
            return 1;
        }
    };

    match run_player(&options) {
        Ok(()) => 0,
        Err(message) => {
            println!("mplayer: {}", message);
            1
        }
    }
}

fn parse_options(args: &[String]) -> Result<Options, &'static str> {
    let mut options = Options {
        use_sas: false,
        repeat: false,
        info_only: false,
        seek_ms: 0,
        files: Vec::new(),
    };

    let mut index = 1usize;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--sas" {
            options.use_sas = true;
        } else if arg == "--direct" {
            options.use_sas = false;
        } else if arg == "--repeat" || arg == "-r" {
            options.repeat = true;
        } else if arg == "--info" {
            options.info_only = true;
        } else if arg == "--seek" {
            index += 1;
            if index >= args.len() {
                return Err("--seek requires seconds");
            }
            options.seek_ms = parse_seconds_to_ms(&args[index])?;
        } else if arg == "--help" || arg == "-h" {
            return Err("help requested");
        } else if arg.starts_with("-") {
            return Err("unknown option");
        } else {
            options.files.push(arg.clone());
        }
        index += 1;
    }

    if options.files.is_empty() {
        return Err("no input files");
    }
    Ok(options)
}

fn print_usage() {
    println!("usage: mplayer [--sas|--direct] [--repeat] [--seek SEC] [--info] FILE.wav...");
    Controls::print_help();
}

fn run_player(options: &Options) -> Result<(), &'static str> {
    if options.info_only {
        for path in options.files.iter() {
            let bytes = read_file(path)?;
            let wav = parse_wav(&bytes)?;
            print_wav_info(path, &wav);
        }
        return Ok(());
    }

    let mut controls = Controls::open();
    Controls::print_help();
    let mut track_index = 0usize;
    let mut seek_ms = options.seek_ms;
    loop {
        if track_index >= options.files.len() {
            if options.repeat {
                track_index = 0;
                seek_ms = options.seek_ms;
            } else {
                return Ok(());
            }
        }

        let path = &options.files[track_index];
        println!(
            "mplayer: playing {}/{}: {}",
            track_index + 1,
            options.files.len(),
            path
        );

        let action = if options.use_sas {
            run_sas(path, seek_ms, &mut controls)?
        } else {
            run(path, seek_ms, &mut controls)?
        };

        match action {
            PlaybackAction::Finished | PlaybackAction::Next => {
                track_index += 1;
                seek_ms = 0;
            }
            PlaybackAction::Previous => {
                track_index = track_index.saturating_sub(1);
                seek_ms = 0;
            }
            PlaybackAction::Quit => return Ok(()),
            PlaybackAction::SeekTo(next_seek_ms) => {
                seek_ms = next_seek_ms;
            }
            PlaybackAction::TogglePause => {}
        }
    }
}

fn run(path: &str, seek_ms: u64, controls: &mut Controls) -> Result<PlaybackAction, &'static str> {
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
    let total_frames = wav.frame_count();
    let start_frame = ms_to_frames(seek_ms, wav.sample_rate).min(total_frames);
    let total_ms = frames_to_ms(total_frames, wav.sample_rate);
    let start_ms = frames_to_ms(start_frame, wav.sample_rate);
    print_wav_info(path, &wav);
    print_seek_start(start_ms, total_ms);

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
    let mut pos = start_frame as usize * frame_bytes;
    let mut started = false;
    let mut paused = false;
    let mut next_progress_ms = start_ms;

    while pos + frame_bytes <= data.len() {
        let status = audio.status().map_err(|_| "failed to get audio status")?;
        if let Some(action) = handle_command(
            controls,
            start_ms + frames_to_ms(status.hw_ptr_frames, wav.sample_rate),
            total_ms,
        ) {
            match action {
                PlaybackAction::TogglePause => {
                    paused = !paused;
                    if paused {
                        if started {
                            let _ = audio.stop();
                        }
                        print_status_message("mplayer: paused");
                    } else {
                        if started {
                            let _ = audio.start();
                        }
                        print_status_message("mplayer: playing");
                    }
                }
                _ => {
                    let _ = audio.stop();
                    let _ = audio.release();
                    return Ok(action);
                }
            }
        }
        maybe_print_progress(
            start_ms + frames_to_ms(status.hw_ptr_frames, wav.sample_rate),
            total_ms,
            &mut next_progress_ms,
        );
        if paused {
            sleep(Duration::from_millis(20));
            continue;
        }

        let frames_left = (data.len() - pos) / frame_bytes;
        let period_frames = params.period_frames as usize;
        let writable_frames = status.writable_frames as usize;
        let (frames, data_frames) = if frames_left <= period_frames {
            if writable_frames < period_frames {
                sleep(Duration::from_millis(1));
                continue;
            }
            (period_frames, frames_left)
        } else {
            let frames = core::cmp::min(writable_frames, frames_left);
            let frames = frames - frames % period_frames;
            if frames == 0 {
                sleep(Duration::from_millis(1));
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
        let current_ms = start_ms + frames_to_ms(status.hw_ptr_frames, wav.sample_rate);
        if let Some(action) = handle_command(controls, current_ms, total_ms) {
            match action {
                PlaybackAction::TogglePause => {
                    paused = !paused;
                    if paused {
                        let _ = audio.stop();
                        print_status_message("mplayer: paused");
                    } else {
                        let _ = audio.start();
                        print_status_message("mplayer: playing");
                    }
                }
                _ => {
                    let _ = audio.stop();
                    let _ = audio.release();
                    return Ok(action);
                }
            }
        }
        maybe_print_progress(current_ms, total_ms, &mut next_progress_ms);
        if paused {
            sleep(Duration::from_millis(20));
            continue;
        }
        if status.hw_ptr_frames >= status.app_ptr_frames {
            break;
        }
        sleep(Duration::from_millis(5));
    }

    let _ = audio.stop();
    let _ = audio.release();
    print_status_message("mplayer: finished");
    Ok(PlaybackAction::Finished)
}

fn run_sas(
    path: &str,
    seek_ms: u64,
    controls: &mut Controls,
) -> Result<PlaybackAction, &'static str> {
    let bytes = read_file(path)?;
    let wav = parse_wav(&bytes)?;
    if wav.audio_format != 1 || wav.bits_per_sample != 16 {
        return Err("SAS MVP accepts only PCM S16LE WAV files");
    }
    if !(8_000..=192_000).contains(&wav.sample_rate) {
        return Err("SAS accepts only 8000-192000 Hz WAV files");
    }
    if wav.channels == 0 || wav.channels > 2 {
        return Err("SAS accepts only mono or stereo WAV files");
    }
    let total_frames = wav.frame_count();
    let start_frame = ms_to_frames(seek_ms, wav.sample_rate).min(total_frames);
    let total_ms = frames_to_ms(total_frames, wav.sample_rate);
    let start_ms = frames_to_ms(start_frame, wav.sample_rate);
    print_wav_info(path, &wav);
    print_seek_start(start_ms, total_ms);

    let mut client = SasClient::connect().map_err(|_| "failed to connect to SAS")?;

    let config = StreamConfig {
        format: AUDIO_PCM_FORMAT_S16LE,
        rate: wav.sample_rate,
        channels: wav.channels,
        period_frames: 480,
        buffer_frames: 1_920,
    };
    let mut stream = client
        .configure(&config)
        .map_err(|_| "failed to configure SAS stream")?;

    let data = &bytes[wav.data_offset..wav.data_offset + wav.data_len];
    let frame_bytes = wav.channels as usize * 2;
    let mut pos_frame = start_frame as usize;
    let total_data_frames = data.len() / frame_bytes;
    let mut segment_start_ms = start_ms;
    let mut next_progress_ms = start_ms;
    let mut paused = false;

    'playback: loop {
        while pos_frame < total_data_frames {
            let read_frames = stream.read_frames();
            let current_ms = segment_start_ms + frames_to_ms(read_frames, wav.sample_rate);
            if let Some(action) = handle_command(controls, current_ms, total_ms) {
                match action {
                    PlaybackAction::TogglePause => {
                        paused = !paused;
                        print_status_message(if paused {
                            "mplayer: paused"
                        } else {
                            "mplayer: playing"
                        });
                    }
                    PlaybackAction::SeekTo(next_ms) => {
                        let next_frame =
                            ms_to_frames(next_ms, wav.sample_rate).min(total_frames) as usize;
                        stream.reset();
                        pos_frame = next_frame;
                        segment_start_ms = frames_to_ms(next_frame as u64, wav.sample_rate);
                        next_progress_ms = segment_start_ms;
                        print_seek_start(segment_start_ms, total_ms);
                    }
                    _ => {
                        let _ = client.close();
                        return Ok(action);
                    }
                }
                continue;
            }
            maybe_print_progress(current_ms, total_ms, &mut next_progress_ms);
            if paused {
                sleep(Duration::from_millis(20));
                continue;
            }

            let writable = stream.writable_frames();
            let frames = writable.min(total_data_frames - pos_frame);
            if frames == 0 {
                sleep(Duration::from_millis(2));
                continue;
            }

            let data_offset = pos_frame * frame_bytes;
            let written = stream.write(&data[data_offset..data_offset + frames * frame_bytes]);
            pos_frame += written;
        }

        client.drain().map_err(|_| "SAS drain failed")?;
        loop {
            let read_frames = stream.read_frames();
            let current_ms = segment_start_ms + frames_to_ms(read_frames, wav.sample_rate);
            if let Some(action) = handle_command(controls, current_ms, total_ms) {
                match action {
                    PlaybackAction::TogglePause => {
                        paused = !paused;
                        print_status_message(if paused {
                            "mplayer: paused"
                        } else {
                            "mplayer: playing"
                        });
                    }
                    PlaybackAction::SeekTo(next_ms) => {
                        let next_frame =
                            ms_to_frames(next_ms, wav.sample_rate).min(total_frames) as usize;
                        stream.reset();
                        pos_frame = next_frame;
                        segment_start_ms = frames_to_ms(next_frame as u64, wav.sample_rate);
                        next_progress_ms = segment_start_ms;
                        print_seek_start(segment_start_ms, total_ms);
                        continue 'playback;
                    }
                    _ => {
                        let _ = client.close();
                        return Ok(action);
                    }
                }
            }
            maybe_print_progress(current_ms, total_ms, &mut next_progress_ms);
            if stream.is_empty() {
                break;
            }
            sleep(Duration::from_millis(10));
        }
        break;
    }
    let _ = client.close();
    print_status_message("mplayer: finished");
    Ok(PlaybackAction::Finished)
}

fn handle_command(controls: &Controls, current_ms: u64, total_ms: u64) -> Option<PlaybackAction> {
    match controls.next_command()? {
        PlayerCommand::Next => Some(PlaybackAction::Next),
        PlayerCommand::Previous => Some(PlaybackAction::Previous),
        PlayerCommand::Quit => Some(PlaybackAction::Quit),
        PlayerCommand::SeekRelative(delta_ms) => {
            let next_ms = if delta_ms < 0 {
                current_ms.saturating_sub((-delta_ms) as u64)
            } else {
                current_ms.saturating_add(delta_ms as u64).min(total_ms)
            };
            print_status_message("mplayer: seek");
            Some(PlaybackAction::SeekTo(next_ms))
        }
        PlayerCommand::TogglePause => Some(PlaybackAction::TogglePause),
        PlayerCommand::Help => {
            print_status_message(
                "controls: Space=play/pause n=next p=previous j=-10s l=+10s q=quit ?=help",
            );
            None
        }
    }
}

fn maybe_print_progress(current_ms: u64, total_ms: u64, next_progress_ms: &mut u64) {
    if current_ms < *next_progress_ms {
        return;
    }
    print_progress(current_ms.min(total_ms), total_ms);
    *next_progress_ms = current_ms - (current_ms % 1_000) + 1_000;
}

fn print_status_message(message: &str) {
    println!("\r\x1b[K{}", message);
}

fn print_progress(current_ms: u64, total_ms: u64) {
    let (cur_min, cur_sec) = duration_min_sec(current_ms);
    let (total_min, total_sec) = duration_min_sec(total_ms);
    let percent = if total_ms == 0 {
        100
    } else {
        current_ms
            .saturating_mul(100)
            .min(total_ms.saturating_mul(100))
            / total_ms
    };
    print!(
        "\rmplayer: {:02}:{:02} / {:02}:{:02} ({}%)\x1b[K",
        cur_min, cur_sec, total_min, total_sec, percent
    );
}

fn print_seek_start(start_ms: u64, total_ms: u64) {
    if start_ms != 0 {
        let (start_min, start_sec) = duration_min_sec(start_ms);
        let (total_min, total_sec) = duration_min_sec(total_ms);
        println!(
            "mplayer: start at {:02}:{:02} / {:02}:{:02}",
            start_min, start_sec, total_min, total_sec
        );
    }
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

fn parse_seconds_to_ms(input: &str) -> Result<u64, &'static str> {
    let seconds = input.parse::<u64>().map_err(|_| "invalid --seek seconds")?;
    Ok(seconds.saturating_mul(1_000))
}

fn frames_to_ms(frames: u64, rate: u32) -> u64 {
    if rate == 0 {
        return 0;
    }
    frames.saturating_mul(1_000) / u64::from(rate)
}

fn ms_to_frames(ms: u64, rate: u32) -> u64 {
    ms.saturating_mul(u64::from(rate)) / 1_000
}

fn duration_min_sec(ms: u64) -> (u64, u64) {
    let seconds = ms / 1_000;
    (seconds / 60, seconds % 60)
}

fn print_wav_info(path: &str, wav: &WavInfo) {
    let duration_ms = frames_to_ms(wav.frame_count(), wav.sample_rate);
    let (minutes, seconds) = duration_min_sec(duration_ms);
    println!(
        "mplayer: info: {}: {} Hz, {} ch, {} bits, {:02}:{:02}",
        path, wav.sample_rate, wav.channels, wav.bits_per_sample, minutes, seconds
    );
}

struct WavInfo {
    audio_format: u16,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    data_offset: usize,
    data_len: usize,
}

impl WavInfo {
    fn frame_count(&self) -> u64 {
        let frame_bytes = self.channels as usize * (self.bits_per_sample as usize / 8);
        if frame_bytes == 0 {
            0
        } else {
            (self.data_len / frame_bytes) as u64
        }
    }
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
