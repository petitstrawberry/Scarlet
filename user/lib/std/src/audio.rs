//! Native Scarlet PCM audio API.

use core::cell::Cell;

use crate::fs::{File, OpenOptions};
use crate::handle::capability::memory_mapping::{flags as mmap_flags, prot};
use crate::io::{Error, ErrorKind, Result};

/// Native audio control command namespace.
pub mod commands {
    pub const AUDIO_GET_CAPS: u32 = 0x4100;
    pub const AUDIO_SET_PARAMS: u32 = 0x4101;
    pub const AUDIO_GET_BUFFER: u32 = 0x4102;
    pub const AUDIO_COMMIT_FRAMES: u32 = 0x4103;
    pub const AUDIO_START: u32 = 0x4104;
    pub const AUDIO_STOP: u32 = 0x4105;
    pub const AUDIO_GET_STATUS: u32 = 0x4106;
    pub const AUDIO_RELEASE: u32 = 0x4107;
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

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AudioPcmParams {
    pub format: u32,
    pub rate: u32,
    pub channels: u16,
    pub _reserved: u16,
    pub period_frames: u32,
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
    pub fn frame_bytes(&self) -> usize {
        match self.format {
            AUDIO_PCM_FORMAT_S16LE => self.channels as usize * 2,
            AUDIO_PCM_FORMAT_S24LE3 => self.channels as usize * 3,
            AUDIO_PCM_FORMAT_S32LE | AUDIO_PCM_FORMAT_F32LE => self.channels as usize * 4,
            AUDIO_PCM_FORMAT_S8 => self.channels as usize,
            _ => 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioPcmCapabilities {
    pub formats: u32,
    pub rate_count: u32,
    pub rates: [u32; AUDIO_PCM_MAX_RATES],
    pub min_channels: u16,
    pub max_channels: u16,
    pub min_period_frames: u32,
    pub max_period_frames: u32,
    pub min_buffer_frames: u32,
    pub max_buffer_frames: u32,
}

impl AudioPcmCapabilities {
    pub fn supports_format(&self, format: u32) -> bool {
        if format >= u32::BITS {
            return false;
        }
        self.formats & (1u32 << format) != 0
    }

    pub fn supports_rate(&self, rate: u32) -> bool {
        self.rates
            .iter()
            .take(self.rate_count as usize)
            .any(|supported| *supported == rate)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioPcmBufferInfo {
    pub mmap_offset: u64,
    pub buffer_bytes: u64,
    pub frame_bytes: u32,
    pub period_bytes: u32,
    pub buffer_frames: u32,
    pub period_frames: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioPcmStatus {
    pub state: u32,
    pub hw_ptr_frames: u64,
    pub app_ptr_frames: u64,
    pub submitted_ptr_frames: u64,
    pub writable_frames: u32,
    pub delay_frames: u32,
    pub xruns: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AudioDeviceInfo {
    pub kind: u32,
    pub flags: u32,
    pub name: [u8; AUDIO_DEVICE_NAME_LEN],
    pub description: [u8; AUDIO_DEVICE_DESCRIPTION_LEN],
}

impl Default for AudioDeviceInfo {
    fn default() -> Self {
        Self {
            kind: AUDIO_DEVICE_KIND_UNKNOWN,
            flags: 0,
            name: [0; AUDIO_DEVICE_NAME_LEN],
            description: [0; AUDIO_DEVICE_DESCRIPTION_LEN],
        }
    }
}

/// Open PCM playback on `/dev/audio0`.
pub fn open_default() -> Result<AudioDevice> {
    AudioDevice::open("/dev/audio0")
}

/// Native PCM audio device.
pub struct AudioDevice {
    file: File,
    configured: Cell<bool>,
}

impl AudioDevice {
    /// Open an audio device by path.
    pub fn open(path: &str) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| Error::new(ErrorKind::NotFound, "audio device not found"))?;
        Ok(Self {
            file,
            configured: Cell::new(false),
        })
    }

    /// Query playback capabilities.
    pub fn capabilities(&self) -> Result<AudioPcmCapabilities> {
        let mut caps = AudioPcmCapabilities::default();
        self.file
            .as_handle()
            .control(commands::AUDIO_GET_CAPS, &mut caps as *mut _ as usize)
            .map_err(|_| Error::new(ErrorKind::Other, "AUDIO_GET_CAPS failed"))?;
        Ok(caps)
    }

    /// Query stable device identity and routing metadata.
    pub fn info(&self) -> Result<AudioDeviceInfo> {
        let mut info = AudioDeviceInfo::default();
        self.file
            .as_handle()
            .control(commands::AUDIO_GET_INFO, &mut info as *mut _ as usize)
            .map_err(|_| Error::new(ErrorKind::Other, "AUDIO_GET_INFO failed"))?;
        Ok(info)
    }

    /// Configure the playback stream.
    pub fn set_params(&self, params: &AudioPcmParams) -> Result<()> {
        self.file
            .as_handle()
            .control(commands::AUDIO_SET_PARAMS, params as *const _ as usize)
            .map_err(|_| Error::new(ErrorKind::InvalidInput, "AUDIO_SET_PARAMS failed"))?;
        self.configured.set(true);
        Ok(())
    }

    /// Query the mmap ring buffer layout.
    pub fn buffer_info(&self) -> Result<AudioPcmBufferInfo> {
        let mut info = AudioPcmBufferInfo::default();
        self.file
            .as_handle()
            .control(commands::AUDIO_GET_BUFFER, &mut info as *mut _ as usize)
            .map_err(|_| Error::new(ErrorKind::Other, "AUDIO_GET_BUFFER failed"))?;
        Ok(info)
    }

    /// Map the configured PCM ring into this process.
    pub fn mmap_buffer(&self, info: &AudioPcmBufferInfo) -> Result<*mut u8> {
        let mapper = self
            .file
            .as_handle()
            .as_memory_mapping()
            .map_err(|_| Error::new(ErrorKind::Unsupported, "audio mmap unsupported"))?;
        let addr = mapper
            .mmap(
                0,
                info.buffer_bytes as usize,
                prot::READ | prot::WRITE,
                mmap_flags::SHARED,
                info.mmap_offset as usize,
            )
            .map_err(|_| Error::new(ErrorKind::Other, "audio mmap failed"))?;
        Ok(addr as *mut u8)
    }

    /// Commit frames written into the mmap ring.
    pub fn commit_frames(&self, frames: u32) -> Result<()> {
        self.file
            .as_handle()
            .control(commands::AUDIO_COMMIT_FRAMES, frames as usize)
            .map_err(|_| Error::new(ErrorKind::InvalidInput, "AUDIO_COMMIT_FRAMES failed"))?;
        Ok(())
    }

    /// Start playback.
    pub fn start(&self) -> Result<()> {
        self.file
            .as_handle()
            .control(commands::AUDIO_START, 0)
            .map_err(|_| Error::new(ErrorKind::Other, "AUDIO_START failed"))?;
        Ok(())
    }

    /// Stop playback.
    pub fn stop(&self) -> Result<()> {
        self.file
            .as_handle()
            .control(commands::AUDIO_STOP, 0)
            .map_err(|_| Error::new(ErrorKind::Other, "AUDIO_STOP failed"))?;
        Ok(())
    }

    /// Release the configured playback stream.
    pub fn release(&self) -> Result<()> {
        if !self.configured.get() {
            return Ok(());
        }
        self.file
            .as_handle()
            .control(commands::AUDIO_RELEASE, 0)
            .map_err(|_| Error::new(ErrorKind::Other, "AUDIO_RELEASE failed"))?;
        self.configured.set(false);
        Ok(())
    }

    /// Query playback status.
    pub fn status(&self) -> Result<AudioPcmStatus> {
        let mut status = AudioPcmStatus::default();
        self.file
            .as_handle()
            .control(commands::AUDIO_GET_STATUS, &mut status as *mut _ as usize)
            .map_err(|_| Error::new(ErrorKind::Other, "AUDIO_GET_STATUS failed"))?;
        Ok(status)
    }

    /// Write bytes through the fallback stream path.
    pub fn write(&mut self, data: &[u8]) -> Result<usize> {
        self.file.write(data)
    }
}

impl Drop for AudioDevice {
    fn drop(&mut self) {
        if self.configured.get() {
            let _ = self.file.as_handle().control(commands::AUDIO_STOP, 0);
            let _ = self.file.as_handle().control(commands::AUDIO_RELEASE, 0);
        }
    }
}
