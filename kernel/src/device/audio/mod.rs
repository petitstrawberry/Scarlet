//! PCM audio device support.
//!
//! This module exposes Scarlet's native audio device model: a playback-only PCM
//! ring buffer with mmap support. Hardware drivers implement
//! [`AudioPlaybackDevice`], while [`AudioCharDevice`] provides the `/dev/audioN`
//! character-device surface.

extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicUsize, Ordering};

use spin::Mutex;

use crate::device::char::CharDevice;
use crate::device::{Device, DeviceCapability, DeviceType};
use crate::environment::PAGE_SIZE;
use crate::library::std::usercopy::{copy_from_user, copy_to_user};
use crate::mem::page::ContiguousPages;
use crate::object::capability::selectable::{
    ReadyInterest, ReadySet, SelectWaitOutcome, Selectable,
};
use crate::object::capability::{ControlOps, MemoryMappingOps};
use crate::println;
use crate::task::mytask;

static AUDIO_DEVICE_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Native audio control command namespace.
pub mod commands {
    /// Query device playback capabilities.
    pub const AUDIO_GET_CAPS: u32 = 0x4100;
    /// Configure the PCM stream and allocate the ring buffer.
    pub const AUDIO_SET_PARAMS: u32 = 0x4101;
    /// Query mmap layout for the configured ring buffer.
    pub const AUDIO_GET_BUFFER: u32 = 0x4102;
    /// Commit frames written directly into the mmaped ring buffer.
    pub const AUDIO_COMMIT_FRAMES: u32 = 0x4103;
    /// Start playback.
    pub const AUDIO_START: u32 = 0x4104;
    /// Stop playback.
    pub const AUDIO_STOP: u32 = 0x4105;
    /// Query stream status and pointer positions.
    pub const AUDIO_GET_STATUS: u32 = 0x4106;
    /// Ask the backend to release stream resources.
    pub const AUDIO_RELEASE: u32 = 0x4107;
    /// Query stable device identity and routing metadata.
    pub const AUDIO_GET_INFO: u32 = 0x4108;
}

/// Unknown or unspecified audio output kind.
pub const AUDIO_DEVICE_KIND_UNKNOWN: u32 = 0;
/// Built-in speaker output.
pub const AUDIO_DEVICE_KIND_SPEAKERS: u32 = 1;
/// Headphone or headset output.
pub const AUDIO_DEVICE_KIND_HEADPHONES: u32 = 2;

/// Maximum bytes in a stable audio device name, including trailing zero padding.
pub const AUDIO_DEVICE_NAME_LEN: usize = 32;
/// Maximum bytes in an audio device description, including trailing zero padding.
pub const AUDIO_DEVICE_DESCRIPTION_LEN: usize = 64;

/// Signed 16-bit little-endian interleaved PCM.
pub const AUDIO_PCM_FORMAT_S16LE: u32 = 1;
/// Signed 24-bit little-endian interleaved PCM packed in 3 bytes.
pub const AUDIO_PCM_FORMAT_S24LE3: u32 = 2;
/// Signed 32-bit little-endian interleaved PCM.
pub const AUDIO_PCM_FORMAT_S32LE: u32 = 3;
/// 32-bit little-endian floating point interleaved PCM.
pub const AUDIO_PCM_FORMAT_F32LE: u32 = 4;
/// Signed 8-bit interleaved PCM.
pub const AUDIO_PCM_FORMAT_S8: u32 = 5;
/// Maximum sample rates returned in one capability query.
pub const AUDIO_PCM_MAX_RATES: usize = 16;

/// Stream is configured but not running.
pub const AUDIO_STATE_PREPARED: u32 = 1;
/// Stream is running.
pub const AUDIO_STATE_RUNNING: u32 = 2;
/// Stream has been stopped.
pub const AUDIO_STATE_STOPPED: u32 = 3;
/// Stream hit an underrun or backend error.
pub const AUDIO_STATE_XRUN: u32 = 4;

/// PCM playback parameters.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AudioPcmParams {
    /// Sample format, currently [`AUDIO_PCM_FORMAT_S16LE`].
    pub format: u32,
    /// Frame rate in Hz.
    pub rate: u32,
    /// Interleaved channel count.
    pub channels: u16,
    /// Reserved for ABI alignment; must be zero.
    pub _reserved: u16,
    /// Period size in frames.
    pub period_frames: u32,
    /// Ring buffer size in frames.
    pub buffer_frames: u32,
}

impl Default for AudioPcmParams {
    fn default() -> Self {
        Self {
            format: AUDIO_PCM_FORMAT_S16LE,
            rate: 48_000,
            channels: 2,
            _reserved: 0,
            period_frames: 480,
            buffer_frames: 1_920,
        }
    }
}

impl AudioPcmParams {
    /// Return the number of bytes in one interleaved frame.
    pub fn frame_bytes(&self) -> Option<usize> {
        match self.format {
            AUDIO_PCM_FORMAT_S16LE => Some(self.channels as usize * 2),
            AUDIO_PCM_FORMAT_S24LE3 => Some(self.channels as usize * 3),
            AUDIO_PCM_FORMAT_S32LE | AUDIO_PCM_FORMAT_F32LE => Some(self.channels as usize * 4),
            AUDIO_PCM_FORMAT_S8 => Some(self.channels as usize),
            _ => None,
        }
    }

    /// Return the number of bytes in one period.
    pub fn period_bytes(&self) -> Option<usize> {
        self.frame_bytes()?.checked_mul(self.period_frames as usize)
    }

    /// Return the number of bytes in the full ring buffer.
    pub fn buffer_bytes(&self) -> Option<usize> {
        self.frame_bytes()?.checked_mul(self.buffer_frames as usize)
    }

    fn validate(&self) -> Result<(), &'static str> {
        if self._reserved != 0 {
            return Err("Audio PCM reserved field must be zero");
        }
        if self.rate == 0 || self.channels == 0 {
            return Err("Invalid PCM rate or channel count");
        }
        if self.period_frames == 0 || self.buffer_frames == 0 {
            return Err("Invalid PCM ring size");
        }
        if self.buffer_frames % self.period_frames != 0 {
            return Err("PCM buffer_frames must be divisible by period_frames");
        }
        let _ = self.frame_bytes().ok_or("Unsupported PCM frame format")?;
        let _ = self.period_bytes().ok_or("PCM period size overflow")?;
        let _ = self.buffer_bytes().ok_or("PCM buffer size overflow")?;
        Ok(())
    }
}

fn pcm_format_bit(format: u32) -> Option<u32> {
    if format < u32::BITS {
        Some(1u32 << format)
    } else {
        None
    }
}

/// PCM device capabilities exposed to user space.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioPcmCapabilities {
    /// Supported sample formats as native Scarlet format bits.
    pub formats: u32,
    /// Number of valid entries in [`Self::rates`].
    pub rate_count: u32,
    /// Supported sample rates in Hz.
    pub rates: [u32; AUDIO_PCM_MAX_RATES],
    /// Minimum supported channel count.
    pub min_channels: u16,
    /// Maximum supported channel count.
    pub max_channels: u16,
    /// Minimum period size in frames.
    pub min_period_frames: u32,
    /// Maximum period size in frames.
    pub max_period_frames: u32,
    /// Minimum ring size in frames.
    pub min_buffer_frames: u32,
    /// Maximum ring size in frames.
    pub max_buffer_frames: u32,
}

impl AudioPcmCapabilities {
    fn supports_format(&self, format: u32) -> bool {
        pcm_format_bit(format)
            .map(|bit| self.formats & bit != 0)
            .unwrap_or(false)
    }

    fn supports_rate(&self, rate: u32) -> bool {
        self.rates
            .iter()
            .take(self.rate_count as usize)
            .any(|supported| *supported == rate)
    }

    fn supports_params(&self, params: &AudioPcmParams) -> bool {
        self.supports_format(params.format)
            && self.supports_rate(params.rate)
            && params.channels >= self.min_channels
            && params.channels <= self.max_channels
            && params.period_frames >= self.min_period_frames
            && params.period_frames <= self.max_period_frames
            && params.buffer_frames >= self.min_buffer_frames
            && params.buffer_frames <= self.max_buffer_frames
    }
}

/// mmap layout for a configured PCM ring.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioPcmBufferInfo {
    /// mmap offset for the PCM data area.
    pub mmap_offset: u64,
    /// mmap length in bytes.
    pub buffer_bytes: u64,
    /// Bytes in one interleaved PCM frame.
    pub frame_bytes: u32,
    /// Bytes in one backend period.
    pub period_bytes: u32,
    /// Ring size in frames.
    pub buffer_frames: u32,
    /// Period size in frames.
    pub period_frames: u32,
}

/// Current PCM stream state.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioPcmStatus {
    /// One of `AUDIO_STATE_*`.
    pub state: u32,
    /// Frames completed by the hardware backend.
    pub hw_ptr_frames: u64,
    /// Frames committed by user space.
    pub app_ptr_frames: u64,
    /// Frames submitted to the hardware backend.
    pub submitted_ptr_frames: u64,
    /// Frames that can be written without overwriting pending audio.
    pub writable_frames: u32,
    /// Frames submitted or buffered but not completed.
    pub delay_frames: u32,
    /// Number of detected underruns/backend submission failures.
    pub xruns: u32,
}

/// Stable identity for a native Scarlet audio output device.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AudioDeviceInfo {
    /// One of `AUDIO_DEVICE_KIND_*`.
    pub kind: u32,
    /// Reserved device flags; currently zero.
    pub flags: u32,
    /// Stable ASCII device name, zero padded.
    pub name: [u8; AUDIO_DEVICE_NAME_LEN],
    /// Human-readable ASCII description, zero padded.
    pub description: [u8; AUDIO_DEVICE_DESCRIPTION_LEN],
}

impl Default for AudioDeviceInfo {
    fn default() -> Self {
        Self::new(AUDIO_DEVICE_KIND_UNKNOWN, "unknown", "Unknown Audio Output")
    }
}

impl AudioDeviceInfo {
    /// Create fixed-size audio device identity metadata.
    ///
    /// # Arguments
    ///
    /// * `kind` - Device kind exposed through `AUDIO_DEVICE_KIND_*`.
    /// * `name` - Stable short ASCII name.
    /// * `description` - Human-readable ASCII description.
    ///
    /// # Returns
    ///
    /// A zero-padded device info structure safe to copy to user space.
    pub fn new(kind: u32, name: &str, description: &str) -> Self {
        let mut info = Self {
            kind,
            flags: 0,
            name: [0; AUDIO_DEVICE_NAME_LEN],
            description: [0; AUDIO_DEVICE_DESCRIPTION_LEN],
        };
        copy_cstr_bytes(&mut info.name, name.as_bytes());
        copy_cstr_bytes(&mut info.description, description.as_bytes());
        info
    }
}

/// Kernel PCM ring memory exposed to a playback backend.
#[derive(Clone, Copy, Debug)]
pub struct AudioPcmBuffer {
    /// Physical address of the first byte of the PCM ring.
    pub paddr: usize,
    /// Kernel virtual address of the first byte of the PCM ring.
    pub vaddr: usize,
    /// Usable PCM ring length in bytes.
    pub buffer_bytes: usize,
    /// Page-aligned mapping length in bytes.
    pub mapped_bytes: usize,
}

/// One committed period within a PCM ring.
#[derive(Clone, Copy, Debug)]
pub struct AudioPcmPeriod {
    /// Absolute frame position of this period.
    pub start_frames: u64,
    /// Byte offset within [`AudioPcmBuffer::buffer_bytes`].
    pub byte_offset: usize,
    /// Period length in bytes.
    pub byte_len: usize,
}

/// Backend implemented by hardware audio drivers.
pub trait AudioPlaybackDevice: Send + Sync {
    /// Return playback capabilities.
    fn capabilities(&self) -> AudioPcmCapabilities;

    /// Configure and prepare the playback stream.
    ///
    /// # Arguments
    ///
    /// * `params` - PCM parameters selected by user space.
    /// * `buffer` - DMA-capable PCM ring mapped by `/dev/audio`.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the stream is ready for period submissions.
    fn configure(
        &self,
        params: &AudioPcmParams,
        buffer: AudioPcmBuffer,
    ) -> Result<(), &'static str>;

    /// Start playback on the backend.
    fn start(&self) -> Result<(), &'static str>;

    /// Stop playback on the backend.
    fn stop(&self) -> Result<(), &'static str>;

    /// Release backend stream resources.
    fn release(&self) -> Result<(), &'static str>;

    /// Submit one committed period from the configured PCM ring.
    ///
    /// # Arguments
    ///
    /// * `period` - Period position inside the configured PCM ring.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the period was queued for playback.
    fn submit_period(&self, period: AudioPcmPeriod) -> Result<(), &'static str>;

    /// Reclaim completed periods.
    ///
    /// # Returns
    ///
    /// Number of completed periods.
    fn process_completions(&self) -> usize;

    /// Maximum backend periods that may be queued at once.
    fn max_in_flight_periods(&self) -> usize {
        4
    }
}

/// Audio codec endpoint controlled by a machine or controller driver.
pub trait AudioCodec: Send + Sync {
    /// Configure codec playback parameters for one DAI link.
    ///
    /// # Arguments
    ///
    /// * `params` - PCM parameters selected for the stream.
    /// * `tx_mask` - TDM transmit slot mask used by the CPU DAI.
    /// * `slots` - Total number of TDM slots in the frame.
    /// * `slot_width` - Width of each TDM slot in bits.
    ///
    /// # Returns
    ///
    /// `Ok(())` when codec playback registers were configured.
    fn configure_playback(
        &self,
        params: &AudioPcmParams,
        tx_mask: u32,
        slots: usize,
        slot_width: usize,
    ) -> Result<(), &'static str>;

    /// Change codec playback mute state.
    ///
    /// # Arguments
    ///
    /// * `muted` - `true` to mute playback output, `false` to unmute it.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the mute state was applied.
    fn set_playback_muted(&self, muted: bool) -> Result<(), &'static str>;

    /// Change codec playback power state.
    ///
    /// # Arguments
    ///
    /// * `powered` - `true` to power playback circuitry, `false` to shut it down.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the power state was applied.
    fn set_playback_powered(&self, powered: bool) -> Result<(), &'static str>;
}

/// CPU-side audio DAI provider that can be routed to codecs.
pub trait AudioDaiProvider: Send + Sync {
    /// Number of firmware cells consumed after the provider phandle.
    ///
    /// # Returns
    ///
    /// Number of `sound-dai` specifier cells expected by this provider.
    fn sound_dai_cells(&self) -> usize;

    /// Attach a playback codec to a CPU DAI endpoint.
    ///
    /// # Arguments
    ///
    /// * `spec` - Firmware specifier cells following the provider phandle.
    /// * `codec` - Codec endpoint to control for playback on this DAI.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the route was accepted.
    fn attach_playback_codec(
        &self,
        spec: &[u32],
        codec: Arc<dyn AudioCodec>,
    ) -> Result<(), &'static str>;

    /// Attach a playback codec with an explicit CPU-side TDM slot mask.
    ///
    /// # Arguments
    ///
    /// * `spec` - Firmware specifier cells following the provider phandle.
    /// * `codec` - Codec endpoint to control for playback on this DAI.
    /// * `tx_mask` - TDM transmit slot mask assigned to this codec.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the route was accepted.
    fn attach_playback_codec_tdm(
        &self,
        spec: &[u32],
        codec: Arc<dyn AudioCodec>,
        tx_mask: u32,
    ) -> Result<(), &'static str> {
        let _ = tx_mask;
        self.attach_playback_codec(spec, codec)
    }
}

struct AudioPcmRing {
    params: AudioPcmParams,
    pages: ContiguousPages,
    mapped_bytes: usize,
    app_ptr_frames: u64,
    submitted_ptr_frames: u64,
    hw_ptr_frames: u64,
    xruns: u32,
    state: u32,
}

impl AudioPcmRing {
    fn new(params: AudioPcmParams) -> Result<Self, &'static str> {
        params.validate()?;
        let buffer_bytes = params.buffer_bytes().ok_or("PCM buffer size overflow")?;
        let mapped_bytes = align_up(buffer_bytes, PAGE_SIZE);
        let page_count = mapped_bytes / PAGE_SIZE;
        let pages = ContiguousPages::new(page_count).ok_or("Failed to allocate PCM ring")?;
        Ok(Self {
            params,
            pages,
            mapped_bytes,
            app_ptr_frames: 0,
            submitted_ptr_frames: 0,
            hw_ptr_frames: 0,
            xruns: 0,
            state: AUDIO_STATE_PREPARED,
        })
    }

    fn frame_bytes(&self) -> usize {
        self.params.frame_bytes().unwrap_or(0)
    }

    fn period_bytes(&self) -> usize {
        self.params.period_bytes().unwrap_or(0)
    }

    fn buffer_bytes(&self) -> usize {
        self.params.buffer_bytes().unwrap_or(0)
    }

    fn queued_frames(&self) -> u64 {
        self.app_ptr_frames.saturating_sub(self.hw_ptr_frames)
    }

    fn writable_frames(&self) -> u32 {
        let queued = self.queued_frames().min(u64::from(u32::MAX));
        self.params.buffer_frames.saturating_sub(queued as u32)
    }

    fn clear_data(&mut self) {
        // SAFETY: `pages` owns `mapped_bytes` bytes backing the mmap PCM ring.
        unsafe {
            core::ptr::write_bytes(self.vaddr() as *mut u8, 0, self.mapped_bytes);
        }
        crate::arch::clean_dcache_to_poc_range(self.vaddr(), self.mapped_bytes);
    }

    fn discard_pending(&mut self) {
        let period_frames = u64::from(self.params.period_frames);
        let next_period = self
            .app_ptr_frames
            .saturating_add(period_frames.saturating_sub(1))
            / period_frames
            * period_frames;
        self.app_ptr_frames = next_period;
        self.hw_ptr_frames = next_period;
        self.submitted_ptr_frames = next_period;
    }

    fn buffer_info(&self) -> AudioPcmBufferInfo {
        AudioPcmBufferInfo {
            mmap_offset: 0,
            buffer_bytes: self.mapped_bytes as u64,
            frame_bytes: self.frame_bytes() as u32,
            period_bytes: self.period_bytes() as u32,
            buffer_frames: self.params.buffer_frames,
            period_frames: self.params.period_frames,
        }
    }

    fn dma_buffer(&self) -> AudioPcmBuffer {
        AudioPcmBuffer {
            paddr: self.paddr(),
            vaddr: self.vaddr(),
            buffer_bytes: self.buffer_bytes(),
            mapped_bytes: self.mapped_bytes,
        }
    }

    fn status(&self) -> AudioPcmStatus {
        let delay = self
            .submitted_ptr_frames
            .saturating_sub(self.hw_ptr_frames)
            .min(u64::from(u32::MAX)) as u32;
        AudioPcmStatus {
            state: self.state,
            hw_ptr_frames: self.hw_ptr_frames,
            app_ptr_frames: self.app_ptr_frames,
            submitted_ptr_frames: self.submitted_ptr_frames,
            writable_frames: self.writable_frames(),
            delay_frames: delay,
            xruns: self.xruns,
        }
    }

    fn paddr(&self) -> usize {
        self.pages.as_paddr()
    }

    fn vaddr(&self) -> usize {
        self.pages.as_vaddr()
    }

    fn commit_frames(&mut self, frames: u32) -> Result<(), &'static str> {
        if frames > self.writable_frames() {
            return Err("PCM ring commit exceeds writable space");
        }
        self.app_ptr_frames = self
            .app_ptr_frames
            .checked_add(u64::from(frames))
            .ok_or("PCM app pointer overflow")?;
        Ok(())
    }

    fn copy_from(&mut self, mut src: &[u8]) -> Result<usize, &'static str> {
        let frame_bytes = self.frame_bytes();
        if frame_bytes == 0 {
            return Err("PCM ring is not configured");
        }
        let writable_bytes = self.writable_frames() as usize * frame_bytes;
        let to_copy = src.len().min(writable_bytes);
        let aligned = to_copy - (to_copy % frame_bytes);
        if aligned == 0 {
            return Ok(0);
        }

        let mut copied = 0usize;
        let buffer_bytes = self.buffer_bytes();
        while copied < aligned {
            let frame_offset = self.app_ptr_frames as usize % self.params.buffer_frames as usize;
            let byte_offset = frame_offset * frame_bytes;
            let chunk = (aligned - copied).min(buffer_bytes - byte_offset);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    src.as_ptr(),
                    (self.vaddr() + byte_offset) as *mut u8,
                    chunk,
                );
            }
            src = &src[chunk..];
            copied += chunk;
            self.app_ptr_frames += (chunk / frame_bytes) as u64;
        }

        Ok(copied)
    }

    fn period_at(&self, start_frames: u64) -> Result<AudioPcmPeriod, &'static str> {
        let frame_bytes = self.frame_bytes();
        let buffer_frames = self.params.buffer_frames as usize;
        let byte_offset = (start_frames as usize % buffer_frames) * frame_bytes;
        let byte_len = self.period_bytes();
        if byte_offset + byte_len > self.buffer_bytes() {
            return Err("PCM period crosses ring boundary");
        }
        Ok(AudioPcmPeriod {
            start_frames,
            byte_offset,
            byte_len,
        })
    }

    fn clean_period(&self, period: AudioPcmPeriod) {
        crate::arch::clean_dcache_to_poc_range(self.vaddr() + period.byte_offset, period.byte_len);
    }
}

/// Character device exposing a playback PCM ring as `/dev/audioN`.
pub struct AudioCharDevice {
    backend: Arc<dyn AudioPlaybackDevice>,
    info: AudioDeviceInfo,
    ring: Mutex<Option<AudioPcmRing>>,
    opened: Mutex<bool>,
}

impl AudioCharDevice {
    /// Create a new audio character device.
    ///
    /// # Arguments
    ///
    /// * `backend` - Hardware playback backend.
    ///
    /// # Returns
    ///
    /// A new audio character device.
    pub fn new(backend: Arc<dyn AudioPlaybackDevice>) -> Self {
        Self::new_with_info(backend, AudioDeviceInfo::default())
    }

    /// Create a new audio character device with stable identity metadata.
    ///
    /// # Arguments
    ///
    /// * `backend` - Hardware playback backend.
    /// * `info` - Device identity returned by `AUDIO_GET_INFO`.
    ///
    /// # Returns
    ///
    /// A new audio character device.
    pub fn new_with_info(backend: Arc<dyn AudioPlaybackDevice>, info: AudioDeviceInfo) -> Self {
        Self {
            backend,
            info,
            ring: Mutex::new(None),
            opened: Mutex::new(false),
        }
    }

    fn pump_locked(&self, ring: &mut AudioPcmRing) {
        let completed = self.backend.process_completions();
        if completed != 0 {
            ring.hw_ptr_frames = ring
                .hw_ptr_frames
                .saturating_add(completed as u64 * u64::from(ring.params.period_frames));
        }

        if ring.state != AUDIO_STATE_RUNNING {
            return;
        }

        let max_in_flight = self.backend.max_in_flight_periods() as u64;
        let period_frames = u64::from(ring.params.period_frames);
        while ring.submitted_ptr_frames + period_frames <= ring.app_ptr_frames {
            let in_flight =
                (ring.submitted_ptr_frames.saturating_sub(ring.hw_ptr_frames)) / period_frames;
            if in_flight >= max_in_flight {
                break;
            }

            let period = match ring.period_at(ring.submitted_ptr_frames) {
                Ok(period) => period,
                Err(error) => {
                    ring.xruns = ring.xruns.saturating_add(1);
                    ring.state = AUDIO_STATE_XRUN;
                    println!("[audio] pump: XRUN on submit: {}", error);
                    break;
                }
            };
            ring.clean_period(period);
            match self.backend.submit_period(period) {
                Ok(()) => {
                    ring.submitted_ptr_frames += period_frames;
                }
                Err(error) => {
                    ring.xruns = ring.xruns.saturating_add(1);
                    ring.state = AUDIO_STATE_XRUN;
                    println!("[audio] pump: XRUN on submit: {}", error);
                    break;
                }
            }
        }
    }

    fn with_ring_mut<T>(
        &self,
        f: impl FnOnce(&mut AudioPcmRing) -> Result<T, &'static str>,
    ) -> Result<T, &'static str> {
        let mut guard = self.ring.lock();
        let ring = guard.as_mut().ok_or("PCM ring is not configured")?;
        let result = f(ring)?;
        Ok(result)
    }

    fn handle_get_caps(&self, arg: usize) -> Result<i32, &'static str> {
        let caps = self.backend.capabilities();
        write_user_value(arg, &caps)?;
        Ok(0)
    }

    fn handle_get_info(&self, arg: usize) -> Result<i32, &'static str> {
        write_user_value(arg, &self.info)?;
        Ok(0)
    }

    fn handle_set_params(&self, arg: usize) -> Result<i32, &'static str> {
        let params: AudioPcmParams = read_user_value(arg)?;
        params.validate()?;
        if !self.backend.capabilities().supports_params(&params) {
            return Err("Unsupported PCM parameters");
        }
        let should_release = {
            let guard = self.ring.lock();
            if let Some(ring) = guard.as_ref() {
                match ring.state {
                    AUDIO_STATE_STOPPED | AUDIO_STATE_XRUN => true,
                    _ => return Err("PCM stream is busy"),
                }
            } else {
                false
            }
        };
        if should_release {
            self.backend.release()?;
        }
        let mut ring = AudioPcmRing::new(params)?;
        ring.clear_data();
        self.backend.configure(&params, ring.dma_buffer())?;
        *self.ring.lock() = Some(ring);
        Ok(0)
    }

    fn handle_get_buffer(&self, arg: usize) -> Result<i32, &'static str> {
        self.with_ring_mut(|ring| {
            let info = ring.buffer_info();
            write_user_value(arg, &info)?;
            Ok(0)
        })
    }

    fn handle_commit_frames(&self, arg: usize) -> Result<i32, &'static str> {
        self.with_ring_mut(|ring| {
            let frames = u32::try_from(arg).map_err(|_| "Frame commit count is too large")?;
            ring.commit_frames(frames)?;
            self.pump_locked(ring);
            Ok(0)
        })
    }

    fn handle_start(&self) -> Result<i32, &'static str> {
        {
            let mut guard = self.ring.lock();
            let ring = guard.as_mut().ok_or("PCM ring is not configured")?;
            if ring.state == AUDIO_STATE_XRUN {
                return Err("PCM stream is in XRUN");
            }
            ring.state = AUDIO_STATE_RUNNING;
            self.pump_locked(ring);
            if ring.state != AUDIO_STATE_RUNNING {
                return Err("PCM submit failed during start");
            }
        }
        if let Err(error) = self.backend.start() {
            let mut guard = self.ring.lock();
            if let Some(ring) = guard.as_mut() {
                ring.state = AUDIO_STATE_XRUN;
            }
            let _ = self.backend.stop();
            return Err(error);
        }
        Ok(0)
    }

    fn handle_stop(&self) -> Result<i32, &'static str> {
        let (was_running, was_xrun) = {
            let mut guard = self.ring.lock();
            let ring = guard.as_mut().ok_or("PCM ring is not configured")?;
            let was_running = ring.state == AUDIO_STATE_RUNNING;
            let was_xrun = ring.state == AUDIO_STATE_XRUN;
            ring.state = AUDIO_STATE_STOPPED;
            (was_running, was_xrun)
        };
        let mut result = Ok(());
        if was_running || was_xrun {
            if let Err(error) = self.backend.stop() {
                result = Err(error);
            }
        }
        if let Some(ring) = self.ring.lock().as_mut() {
            ring.discard_pending();
        }
        result.map(|_| 0)
    }

    fn handle_get_status(&self, arg: usize) -> Result<i32, &'static str> {
        self.with_ring_mut(|ring| {
            self.pump_locked(ring);
            let status = ring.status();
            write_user_value(arg, &status)?;
            Ok(0)
        })
    }

    fn handle_release(&self) -> Result<i32, &'static str> {
        let Some(was_running) = ({
            let mut guard = self.ring.lock();
            let Some(ring) = guard.as_mut() else {
                *self.opened.lock() = false;
                return Ok(0);
            };
            let was_running = ring.state == AUDIO_STATE_RUNNING;
            ring.state = AUDIO_STATE_STOPPED;
            Some(was_running)
        }) else {
            return Ok(0);
        };
        let mut result = Ok(());
        if was_running {
            if let Err(error) = self.backend.stop() {
                result = Err(error);
            }
        }
        if let Err(error) = self.backend.release()
            && result.is_ok()
        {
            result = Err(error);
        }
        *self.ring.lock() = None;
        *self.opened.lock() = false;
        result.map(|_| 0)
    }
}

/// Register a playback backend as a native Scarlet audio device.
///
/// # Arguments
///
/// * `backend` - Hardware playback backend to expose.
///
/// # Returns
///
/// The registered character device name.
pub fn register_playback_device(backend: Arc<dyn AudioPlaybackDevice>) -> String {
    register_playback_device_with_info(backend, AudioDeviceInfo::default())
}

/// Register a playback backend as a native Scarlet audio device with identity metadata.
///
/// # Arguments
///
/// * `backend` - Hardware playback backend to expose.
/// * `info` - Device identity returned by `AUDIO_GET_INFO`.
///
/// # Returns
///
/// The registered character device name.
pub fn register_playback_device_with_info(
    backend: Arc<dyn AudioPlaybackDevice>,
    info: AudioDeviceInfo,
) -> String {
    let id = AUDIO_DEVICE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let name = alloc::format!("audio{}", id);
    let audio_char: Arc<dyn Device> = Arc::new(AudioCharDevice::new_with_info(backend, info));
    crate::device::manager::DeviceManager::get_manager()
        .register_device_with_name(name.clone(), audio_char);
    name
}

impl Device for AudioCharDevice {
    fn open(&self) -> Result<(), &'static str> {
        let mut opened = self.opened.lock();
        if *opened {
            return Err("PCM device is busy");
        }
        *opened = true;
        Ok(())
    }

    fn close(&self) {
        let stream = {
            let mut ring = self.ring.lock();
            ring.as_mut().map(|ring| {
                let was_running = ring.state == AUDIO_STATE_RUNNING;
                ring.state = AUDIO_STATE_STOPPED;
                was_running
            })
        };
        if let Some(was_running) = stream {
            if was_running {
                let _ = self.backend.stop();
            }
            let _ = self.backend.release();
            *self.ring.lock() = None;
        }
        *self.opened.lock() = false;
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Char
    }

    fn name(&self) -> &'static str {
        "audio"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn capabilities(&self) -> &'static [DeviceCapability] {
        static CAPS: [DeviceCapability; 1] = [DeviceCapability::Audio];
        &CAPS
    }

    fn as_char_device(&self) -> Option<&dyn CharDevice> {
        Some(self)
    }
}

impl CharDevice for AudioCharDevice {
    fn read_byte(&self) -> Option<u8> {
        None
    }

    fn write_byte(&self, byte: u8) -> Result<(), &'static str> {
        self.write(&[byte]).map(|_| ())
    }

    fn write(&self, buffer: &[u8]) -> Result<usize, &'static str> {
        self.write_at(0, buffer)
    }

    fn can_read(&self) -> bool {
        false
    }

    fn can_write(&self) -> bool {
        self.ring
            .lock()
            .as_ref()
            .map(|ring| ring.writable_frames() != 0)
            .unwrap_or(false)
    }

    fn write_at(&self, _position: u64, buffer: &[u8]) -> Result<usize, &'static str> {
        self.with_ring_mut(|ring| {
            let written = ring.copy_from(buffer)?;
            self.pump_locked(ring);
            Ok(written)
        })
    }
}

impl ControlOps for AudioCharDevice {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        use commands::*;

        match command {
            AUDIO_GET_CAPS => self.handle_get_caps(arg),
            AUDIO_GET_INFO => self.handle_get_info(arg),
            AUDIO_SET_PARAMS => self.handle_set_params(arg),
            AUDIO_GET_BUFFER => self.handle_get_buffer(arg),
            AUDIO_COMMIT_FRAMES => self.handle_commit_frames(arg),
            AUDIO_START => self.handle_start(),
            AUDIO_STOP => self.handle_stop(),
            AUDIO_GET_STATUS => self.handle_get_status(arg),
            AUDIO_RELEASE => self.handle_release(),
            _ => Err("Unsupported audio control command"),
        }
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        use commands::*;
        alloc::vec![
            (AUDIO_GET_CAPS, "Get audio playback capabilities"),
            (AUDIO_GET_INFO, "Get audio device identity"),
            (AUDIO_SET_PARAMS, "Set PCM playback parameters"),
            (AUDIO_GET_BUFFER, "Get mmap PCM ring buffer layout"),
            (AUDIO_COMMIT_FRAMES, "Commit mmap-written PCM frames"),
            (AUDIO_START, "Start PCM playback"),
            (AUDIO_STOP, "Stop PCM playback"),
            (AUDIO_GET_STATUS, "Get PCM stream status"),
            (AUDIO_RELEASE, "Release PCM playback stream"),
        ]
    }
}

impl MemoryMappingOps for AudioCharDevice {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
        let guard = self.ring.lock();
        let ring = guard.as_ref().ok_or("PCM ring is not configured")?;
        if offset % PAGE_SIZE != 0 || length % PAGE_SIZE != 0 {
            return Err("PCM mmap offset and length must be page-aligned");
        }
        if offset >= ring.mapped_bytes {
            return Err("PCM mmap offset exceeds ring size");
        }
        if length > ring.mapped_bytes - offset {
            return Err("PCM mmap length exceeds ring size");
        }
        Ok((ring.paddr() + offset, 0x3, true))
    }

    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {}

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {}

    fn supports_mmap(&self) -> bool {
        self.ring.lock().as_ref().is_some()
    }
}

impl Selectable for AudioCharDevice {
    fn current_ready(&self, interest: ReadyInterest) -> ReadySet {
        let mut set = ReadySet::none();
        if interest.write {
            let mut guard = self.ring.lock();
            if let Some(ring) = guard.as_mut() {
                self.pump_locked(ring);
                set.write = ring.writable_frames() >= ring.params.period_frames;
            }
        }
        set
    }

    fn wait_until_ready(
        &self,
        interest: ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> SelectWaitOutcome {
        let ready = self.current_ready(interest);
        if (interest.write && ready.write) || (interest.read && ready.read) {
            SelectWaitOutcome::Ready
        } else {
            SelectWaitOutcome::TimedOut
        }
    }
}

fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}

fn copy_cstr_bytes(dst: &mut [u8], src: &[u8]) {
    if dst.is_empty() {
        return;
    }
    let len = src.len().min(dst.len() - 1);
    dst[..len].copy_from_slice(&src[..len]);
}

fn read_user_value<T: Copy>(ptr: usize) -> Result<T, &'static str> {
    if ptr == 0 {
        return Err("Audio ioctl pointer is null");
    }
    let task = mytask().ok_or("No current task for audio ioctl")?;
    let mut value = core::mem::MaybeUninit::<T>::uninit();
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(value.as_mut_ptr() as *mut u8, core::mem::size_of::<T>())
    };
    copy_from_user(task, ptr, bytes).map_err(|_| "Failed to copy audio ioctl from user")?;
    Ok(unsafe { value.assume_init() })
}

fn write_user_value<T: Copy>(ptr: usize, value: &T) -> Result<(), &'static str> {
    if ptr == 0 {
        return Err("Audio ioctl pointer is null");
    }
    let task = mytask().ok_or("No current task for audio ioctl")?;
    let bytes = unsafe {
        core::slice::from_raw_parts(value as *const T as *const u8, core::mem::size_of::<T>())
    };
    copy_to_user(task, ptr, bytes).map_err(|_| "Failed to copy audio ioctl to user")
}
