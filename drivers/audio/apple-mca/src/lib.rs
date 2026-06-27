#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::{
    arch::mmio,
    device::{
        DeviceInfo,
        audio::{
            AUDIO_PCM_FORMAT_S16LE, AUDIO_PCM_FORMAT_S32LE, AUDIO_PCM_MAX_RATES, AudioCodec,
            AudioDaiProvider, AudioPcmCapabilities, AudioPcmParams, AudioPlaybackDevice,
            register_playback_device,
        },
        clk::ClkHandle,
        dma::{
            DmaBusWidth, DmaChannel, DmaCyclicConfig, DmaDirection, DmaError, DmaPeripheralConfig,
        },
        fdt::FdtManager,
        manager::{DeviceManager, DriverPriority, probe_defer},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_println,
    environment::PAGE_SIZE,
    mem::page::ContiguousPages,
    time::udelay,
};

const APPLE_MCA_T8103_CLUSTERS: usize = 6;
const APPLE_MCA_T6000_CLUSTERS: usize = 4;
const APPLE_MCA_PLAYBACK_CLUSTER: usize = 0;
const APPLE_MCA_PLAYBACK_DMA: &str = "tx0a";
const APPLE_MCA_BCLK_RATIO: u64 = 64;
const APPLE_MCA_SLOT_WIDTH: usize = 32;
const APPLE_MCA_SOUND_DAI_CELLS: usize = 1;
const APPLE_MCA_MAX_BCLK: u32 = 24_576_000;
const APPLE_MCA_MAX_PERIOD_FRAMES: u32 = 4_096;
const APPLE_MCA_MAX_BUFFER_FRAMES: u32 = 65_536;

const CLUSTER_STRIDE: usize = 0x4000;

const REG_STATUS: usize = 0x000;
const STATUS_MCLK_EN: u32 = 1 << 0;
const REG_MCLK_CONF: usize = 0x004;
const MCLK_CONF_DIV_SHIFT: u32 = 8;

const REG_SYNCGEN_STATUS: usize = 0x100;
const SYNCGEN_STATUS_EN: u32 = 1 << 0;
const REG_SYNCGEN_MCLK_SEL: usize = 0x104;
const REG_SYNCGEN_HI_PERIOD: usize = 0x108;
const REG_SYNCGEN_LO_PERIOD: usize = 0x10c;

const CLUSTER_TXA_OFF: usize = 0x300;
const REG_SERDES_STATUS: usize = 0x00;
const SERDES_STATUS_EN: u32 = 1 << 0;
const SERDES_STATUS_RST: u32 = 1 << 1;
const REG_TX_SERDES_CONF: usize = 0x04;
const REG_TX_SERDES_BITSTART: usize = 0x08;
const REG_TX_SERDES_SLOTMASK: usize = 0x0c;
const SERDES_CONF_NCHANS_MASK: u32 = 0x0f;
const SERDES_CONF_WIDTH_MASK: u32 = 0x1f0;
const SERDES_CONF_WIDTH_16BIT: u32 = 0x040;
const SERDES_CONF_WIDTH_32BIT: u32 = 0x100;
const SERDES_CONF_UNK1: u32 = 1 << 12;
const SERDES_CONF_UNK2: u32 = 1 << 13;
const SERDES_CONF_UNK3: u32 = 1 << 14;
const SERDES_CONF_SYNC_SEL_MASK: u32 = 0x7 << 16;
const SERDES_CONF_SYNC_SEL_SHIFT: u32 = 16;

const REG_PORT_ENABLES: usize = 0x600;
const PORT_ENABLES_CLOCKS: u32 = 0x6;
const PORT_ENABLES_TX_DATA: u32 = 1 << 3;
const REG_PORT_CLOCK_SEL: usize = 0x604;
const REG_PORT_DATA_SEL: usize = 0x608;
const PORT_CLOCK_SEL_SHIFT: u32 = 8;

const DMA_ADAPTER_TX_NCHANS_SHIFT: u32 = 5;
const DMA_ADAPTER_RX_MSB_PAD_SHIFT: u32 = 8;
const DMA_ADAPTER_RX_NCHANS_SHIFT: u32 = 13;
const DMA_ADAPTER_NCHANS_SHIFT: u32 = 20;
const DMA_ADAPTER_FIXED_NCHANS: u32 = 0x2;

static APPLE_MCA_DEVICES: Mutex<Vec<Arc<AppleMca>>> = Mutex::new(Vec::new());

struct AppleMcaDma {
    name: String,
    channel: Arc<dyn DmaChannel>,
}

struct AppleMcaStream {
    channel: Arc<dyn DmaChannel>,
    pages: ContiguousPages,
    params: AudioPcmParams,
    mapped_bytes: usize,
    buffer_bytes: usize,
    period_bytes: usize,
    period_count: usize,
    submit_period: usize,
    in_flight_periods: usize,
    running: bool,
}

impl AppleMcaStream {
    fn new(
        channel: Arc<dyn DmaChannel>,
        pages: ContiguousPages,
        params: AudioPcmParams,
        mapped_bytes: usize,
    ) -> Result<Self, &'static str> {
        let buffer_bytes = params
            .buffer_bytes()
            .ok_or("apple-mca: PCM buffer overflow")?;
        let period_bytes = params
            .period_bytes()
            .ok_or("apple-mca: PCM period overflow")?;
        let period_count = (params.buffer_frames / params.period_frames) as usize;
        if period_count == 0 {
            return Err("apple-mca: invalid period count");
        }

        Ok(Self {
            channel,
            pages,
            params,
            mapped_bytes,
            buffer_bytes,
            period_bytes,
            period_count,
            submit_period: 0,
            in_flight_periods: 0,
            running: false,
        })
    }

    fn clear(&self) {
        // SAFETY: `pages` owns `mapped_bytes` bytes of contiguous kernel
        // memory for the DMA ring and the pointer is valid for writes.
        unsafe {
            core::ptr::write_bytes(self.pages.as_vaddr() as *mut u8, 0, self.mapped_bytes);
        }
    }

    fn copy_period(&mut self, pcm: &[u8]) -> Result<(), &'static str> {
        if pcm.len() != self.period_bytes {
            return Err("apple-mca: period size mismatch");
        }
        if self.in_flight_periods >= self.period_count {
            return Err("apple-mca: DMA ring is full");
        }

        let offset = self.submit_period * self.period_bytes;
        if offset + pcm.len() > self.buffer_bytes {
            return Err("apple-mca: DMA ring write out of range");
        }

        // SAFETY: The destination range is within the owned DMA ring and the
        // source slice is valid for `pcm.len()` bytes. The regions do not
        // overlap because user PCM data is copied from a temporary period
        // buffer owned by the audio core.
        unsafe {
            core::ptr::copy_nonoverlapping(
                pcm.as_ptr(),
                (self.pages.as_vaddr() + offset) as *mut u8,
                pcm.len(),
            );
        }

        self.submit_period = (self.submit_period + 1) % self.period_count;
        self.in_flight_periods += 1;
        Ok(())
    }

    fn take_completions(&mut self) -> usize {
        let completed = self.channel.take_completed_periods();
        let completed = completed.min(self.in_flight_periods);
        self.in_flight_periods -= completed;
        completed
    }
}

struct AppleMca {
    base: usize,
    switch_base: usize,
    size: usize,
    switch_size: usize,
    clocks: Vec<ClkHandle>,
    dmas: Vec<AppleMcaDma>,
    stream: Mutex<Option<AppleMcaStream>>,
    playback_codec: Mutex<Option<Arc<dyn AudioCodec>>>,
}

impl AppleMca {
    fn new(
        base: usize,
        size: usize,
        switch_base: usize,
        switch_size: usize,
        clocks: Vec<ClkHandle>,
        dmas: Vec<AppleMcaDma>,
    ) -> Self {
        Self {
            base,
            switch_base,
            size,
            switch_size,
            clocks,
            dmas,
            stream: Mutex::new(None),
            playback_codec: Mutex::new(None),
        }
    }

    fn read_reg(&self, base: usize, offset: usize) -> u32 {
        // SAFETY: `base` is an ioremap'd MCA MMIO window and offsets are fixed
        // MCA register offsets.
        unsafe { mmio::read32(base + offset) }
    }

    fn write_reg(&self, base: usize, offset: usize, value: u32) {
        // SAFETY: `base` is an ioremap'd MCA MMIO window and offsets are fixed
        // MCA register offsets.
        unsafe { mmio::write32(base + offset, value) }
    }

    fn modify_reg(&self, base: usize, offset: usize, mask: u32, value: u32) {
        let current = self.read_reg(base, offset);
        self.write_reg(base, offset, (current & !mask) | (value & mask));
    }

    fn cluster_base(&self, cluster: usize) -> Result<usize, &'static str> {
        if cluster >= self.clocks.len() || cluster >= cluster_count_from_size(self.size) {
            return Err("apple-mca: cluster out of range");
        }
        Ok(self.base + CLUSTER_STRIDE * cluster)
    }

    fn dma_adapter_a_reg(cluster: usize) -> usize {
        0x8000 * cluster
    }

    fn txa_port_data_sel(cluster: usize) -> u32 {
        1u32 << (cluster * 2)
    }

    fn playback_dma(&self) -> Result<Arc<dyn DmaChannel>, &'static str> {
        self.dmas
            .iter()
            .find(|dma| dma.name == APPLE_MCA_PLAYBACK_DMA)
            .map(|dma| dma.channel.clone())
            .ok_or("apple-mca: missing playback DMA channel")
    }

    fn playback_codec(&self) -> Option<Arc<dyn AudioCodec>> {
        self.playback_codec.lock().clone()
    }

    fn playback_slots(params: &AudioPcmParams) -> usize {
        (APPLE_MCA_BCLK_RATIO as usize / APPLE_MCA_SLOT_WIDTH).max(params.channels as usize)
    }

    fn playback_tx_mask(params: &AudioPcmParams) -> u32 {
        let channels = params.channels as usize;
        if channels >= u32::BITS as usize {
            u32::MAX
        } else {
            (1u32 << channels) - 1
        }
    }

    fn sample_width_bits(params: &AudioPcmParams) -> Result<usize, &'static str> {
        match params.format {
            AUDIO_PCM_FORMAT_S16LE => Ok(16),
            AUDIO_PCM_FORMAT_S32LE => Ok(32),
            _ => Err("apple-mca: unsupported PCM format"),
        }
    }

    fn dma_width(params: &AudioPcmParams) -> Result<DmaBusWidth, &'static str> {
        match params.format {
            AUDIO_PCM_FORMAT_S16LE => Ok(DmaBusWidth::Width2),
            AUDIO_PCM_FORMAT_S32LE => Ok(DmaBusWidth::Width4),
            _ => Err("apple-mca: unsupported DMA width"),
        }
    }

    fn configure_serdes(
        &self,
        cluster_base: usize,
        cluster: usize,
        params: &AudioPcmParams,
    ) -> Result<(), &'static str> {
        let slots = Self::playback_slots(params);
        let slot_mask = Self::playback_tx_mask(params);
        let width = match Self::sample_width_bits(params)? {
            16 => SERDES_CONF_WIDTH_16BIT,
            32 => SERDES_CONF_WIDTH_32BIT,
            _ => return Err("apple-mca: unsupported SERDES width"),
        };

        let conf_mask = SERDES_CONF_NCHANS_MASK
            | SERDES_CONF_WIDTH_MASK
            | SERDES_CONF_SYNC_SEL_MASK
            | SERDES_CONF_UNK1
            | SERDES_CONF_UNK2
            | SERDES_CONF_UNK3;
        let conf = ((slots.saturating_sub(1) as u32) & SERDES_CONF_NCHANS_MASK)
            | width
            | (((cluster + 1) as u32) << SERDES_CONF_SYNC_SEL_SHIFT)
            | SERDES_CONF_UNK1
            | SERDES_CONF_UNK2
            | SERDES_CONF_UNK3;
        let serdes = CLUSTER_TXA_OFF;
        self.modify_reg(cluster_base, serdes + REG_TX_SERDES_CONF, conf_mask, conf);
        self.write_reg(cluster_base, serdes + REG_TX_SERDES_BITSTART, 1);
        self.write_reg(cluster_base, serdes + REG_TX_SERDES_SLOTMASK, u32::MAX);
        self.write_reg(
            cluster_base,
            serdes + REG_TX_SERDES_SLOTMASK + 0x4,
            !slot_mask,
        );
        self.write_reg(
            cluster_base,
            serdes + REG_TX_SERDES_SLOTMASK + 0x8,
            u32::MAX,
        );
        self.write_reg(
            cluster_base,
            serdes + REG_TX_SERDES_SLOTMASK + 0xc,
            !slot_mask,
        );
        Ok(())
    }

    fn configure_clocks_and_port(
        &self,
        cluster_base: usize,
        cluster: usize,
        params: &AudioPcmParams,
    ) -> Result<(), &'static str> {
        let bclk = u64::from(params.rate)
            .checked_mul(APPLE_MCA_BCLK_RATIO)
            .ok_or("apple-mca: BCLK overflow")?;
        if bclk > u64::from(APPLE_MCA_MAX_BCLK) {
            return Err("apple-mca: requested BCLK is too high");
        }

        let clock = self
            .clocks
            .get(cluster)
            .ok_or("apple-mca: missing cluster clock")?;
        clock
            .set_rate(bclk)
            .map_err(|_| "apple-mca: failed to set NCO rate")?;

        self.write_reg(
            cluster_base,
            REG_SYNCGEN_HI_PERIOD,
            (APPLE_MCA_BCLK_RATIO / 2 - 1) as u32,
        );
        self.write_reg(
            cluster_base,
            REG_SYNCGEN_LO_PERIOD,
            ((APPLE_MCA_BCLK_RATIO + 1) / 2 - 1) as u32,
        );
        self.write_reg(cluster_base, REG_MCLK_CONF, 1 << MCLK_CONF_DIV_SHIFT);
        self.write_reg(cluster_base, REG_SYNCGEN_MCLK_SEL, (cluster + 1) as u32);
        self.modify_reg(
            cluster_base,
            REG_SYNCGEN_STATUS,
            SYNCGEN_STATUS_EN,
            SYNCGEN_STATUS_EN,
        );
        self.modify_reg(cluster_base, REG_STATUS, STATUS_MCLK_EN, STATUS_MCLK_EN);

        self.write_reg(
            cluster_base,
            REG_PORT_CLOCK_SEL,
            ((cluster + 1) as u32) << PORT_CLOCK_SEL_SHIFT,
        );
        self.modify_reg(
            cluster_base,
            REG_PORT_ENABLES,
            PORT_ENABLES_CLOCKS,
            PORT_ENABLES_CLOCKS,
        );
        self.write_reg(
            cluster_base,
            REG_PORT_DATA_SEL,
            Self::txa_port_data_sel(cluster),
        );
        self.modify_reg(
            cluster_base,
            REG_PORT_ENABLES,
            PORT_ENABLES_TX_DATA,
            PORT_ENABLES_TX_DATA,
        );
        Ok(())
    }

    fn configure_dma_adapter(
        &self,
        cluster: usize,
        params: &AudioPcmParams,
    ) -> Result<(), &'static str> {
        let sample_width = Self::sample_width_bits(params)?;
        let pad = (32usize)
            .checked_sub(sample_width)
            .ok_or("apple-mca: invalid sample width")? as u32;
        let channels = (params.channels as u32).min(4).max(1);
        let value = (channels << DMA_ADAPTER_NCHANS_SHIFT)
            | (DMA_ADAPTER_FIXED_NCHANS << DMA_ADAPTER_TX_NCHANS_SHIFT)
            | (DMA_ADAPTER_FIXED_NCHANS << DMA_ADAPTER_RX_NCHANS_SHIFT)
            | pad
            | (pad << DMA_ADAPTER_RX_MSB_PAD_SHIFT);
        self.write_reg(self.switch_base, Self::dma_adapter_a_reg(cluster), value);
        Ok(())
    }

    fn early_start_serdes(&self, cluster_base: usize, cluster: usize) {
        let serdes = CLUSTER_TXA_OFF;
        self.modify_reg(
            cluster_base,
            serdes + REG_TX_SERDES_CONF,
            SERDES_CONF_SYNC_SEL_MASK,
            0,
        );
        self.modify_reg(
            cluster_base,
            serdes + REG_TX_SERDES_CONF,
            SERDES_CONF_SYNC_SEL_MASK,
            7 << SERDES_CONF_SYNC_SEL_SHIFT,
        );
        self.modify_reg(
            cluster_base,
            serdes + REG_SERDES_STATUS,
            SERDES_STATUS_EN | SERDES_STATUS_RST,
            SERDES_STATUS_RST,
        );
        udelay(50);
        self.modify_reg(
            cluster_base,
            serdes + REG_TX_SERDES_CONF,
            SERDES_CONF_SYNC_SEL_MASK,
            0,
        );
        self.modify_reg(
            cluster_base,
            serdes + REG_TX_SERDES_CONF,
            SERDES_CONF_SYNC_SEL_MASK,
            ((cluster + 1) as u32) << SERDES_CONF_SYNC_SEL_SHIFT,
        );
        udelay(100);
    }

    fn enable_serdes(&self, cluster_base: usize) {
        self.modify_reg(
            cluster_base,
            CLUSTER_TXA_OFF + REG_SERDES_STATUS,
            SERDES_STATUS_EN | SERDES_STATUS_RST,
            SERDES_STATUS_EN,
        );
    }

    fn disable_serdes(&self, cluster_base: usize) {
        self.modify_reg(
            cluster_base,
            CLUSTER_TXA_OFF + REG_SERDES_STATUS,
            SERDES_STATUS_EN,
            0,
        );
    }

    fn disable_port(&self, cluster_base: usize) {
        self.modify_reg(cluster_base, REG_PORT_ENABLES, PORT_ENABLES_TX_DATA, 0);
        self.write_reg(cluster_base, REG_PORT_DATA_SEL, 0);
    }
}

fn cluster_count_from_size(size: usize) -> usize {
    if size < CLUSTER_STRIDE {
        0
    } else {
        (size - CLUSTER_STRIDE) / CLUSTER_STRIDE + 1
    }
}

fn align_up(value: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (value + align - 1) & !(align - 1)
}

fn dma_error_to_str(error: DmaError) -> &'static str {
    match error {
        DmaError::InvalidSpec => "apple-mca: invalid DMA spec",
        DmaError::ChannelNotFound => "apple-mca: DMA channel not found",
        DmaError::ChannelBusy => "apple-mca: DMA channel busy",
        DmaError::InvalidConfig => "apple-mca: invalid DMA config",
        DmaError::Unsupported => "apple-mca: unsupported DMA operation",
        DmaError::HardwareError => "apple-mca: DMA hardware error",
        DmaError::NotPrepared => "apple-mca: DMA channel not prepared",
    }
}

fn read_phandle(device: &PlatformDeviceInfo) -> Result<u32, &'static str> {
    device
        .property("phandle")
        .or_else(|| device.property("linux,phandle"))
        .and_then(|property| property.as_usize())
        .map(|value| value as u32)
        .ok_or("apple-mca: missing phandle")
}

fn read_be_u32_cells(value: &[u8]) -> Vec<u32> {
    value
        .chunks_exact(4)
        .map(|chunk| u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn parse_sound_dai_property(value: &[u8]) -> Result<(u32, Vec<u32>), &'static str> {
    if value.len() % 4 != 0 {
        return Err("apple-macaudio: malformed sound-dai");
    }
    let cells = read_be_u32_cells(value);
    let phandle = *cells.first().ok_or("apple-macaudio: empty sound-dai")?;
    Ok((phandle, cells[1..].to_vec()))
}

impl AudioDaiProvider for AppleMca {
    fn sound_dai_cells(&self) -> usize {
        APPLE_MCA_SOUND_DAI_CELLS
    }

    fn attach_playback_codec(
        &self,
        spec: &[u32],
        codec: Arc<dyn AudioCodec>,
    ) -> Result<(), &'static str> {
        if spec.len() != APPLE_MCA_SOUND_DAI_CELLS {
            return Err("apple-mca: invalid sound-dai specifier");
        }
        if spec[0] as usize != APPLE_MCA_PLAYBACK_CLUSTER {
            return Err("apple-mca: only cluster 0 playback is supported");
        }

        *self.playback_codec.lock() = Some(codec);
        Ok(())
    }
}

impl AudioPlaybackDevice for AppleMca {
    fn capabilities(&self) -> AudioPcmCapabilities {
        let max_rate = APPLE_MCA_MAX_BCLK / APPLE_MCA_BCLK_RATIO as u32;
        let mut rates = [0u32; AUDIO_PCM_MAX_RATES];
        let mut rate_count = 0usize;
        for rate in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
            if rate <= max_rate && rate_count < rates.len() {
                rates[rate_count] = rate;
                rate_count += 1;
            }
        }

        AudioPcmCapabilities {
            formats: (1 << AUDIO_PCM_FORMAT_S16LE) | (1 << AUDIO_PCM_FORMAT_S32LE),
            rate_count: rate_count as u32,
            rates,
            min_channels: 1,
            max_channels: 2,
            min_period_frames: 64,
            max_period_frames: APPLE_MCA_MAX_PERIOD_FRAMES,
            min_buffer_frames: 128,
            max_buffer_frames: APPLE_MCA_MAX_BUFFER_FRAMES,
        }
    }

    fn configure(&self, params: &AudioPcmParams) -> Result<(), &'static str> {
        self.release()?;

        let cluster = APPLE_MCA_PLAYBACK_CLUSTER;
        let cluster_base = self.cluster_base(cluster)?;
        let channel = self.playback_dma()?;
        let buffer_bytes = params
            .buffer_bytes()
            .ok_or("apple-mca: PCM buffer overflow")?;
        let period_bytes = params
            .period_bytes()
            .ok_or("apple-mca: PCM period overflow")?;
        let mapped_bytes = align_up(buffer_bytes, PAGE_SIZE);
        let pages =
            ContiguousPages::new(mapped_bytes / PAGE_SIZE).ok_or("apple-mca: DMA alloc failed")?;

        self.configure_clocks_and_port(cluster_base, cluster, params)?;
        self.configure_serdes(cluster_base, cluster, params)?;
        self.configure_dma_adapter(cluster, params)?;
        if let Some(codec) = self.playback_codec() {
            codec.configure_playback(
                params,
                Self::playback_tx_mask(params),
                Self::playback_slots(params),
                APPLE_MCA_SLOT_WIDTH,
            )?;
            codec.set_playback_powered(true)?;
            codec.set_playback_muted(true)?;
        }

        let stream = AppleMcaStream::new(channel.clone(), pages, *params, mapped_bytes)?;
        stream.clear();

        let burst_len = (params.channels as usize).min(4).max(1);
        channel
            .prepare_cyclic(DmaCyclicConfig {
                buffer_addr: stream.pages.as_paddr(),
                buffer_len: buffer_bytes,
                period_len: period_bytes,
                direction: DmaDirection::MemToDev,
                peripheral: Some(DmaPeripheralConfig {
                    addr: 1,
                    width: Self::dma_width(params)?,
                    burst_len,
                }),
            })
            .map_err(dma_error_to_str)?;

        *self.stream.lock() = Some(stream);
        Ok(())
    }

    fn start(&self) -> Result<(), &'static str> {
        let cluster = APPLE_MCA_PLAYBACK_CLUSTER;
        let cluster_base = self.cluster_base(cluster)?;
        let channel = {
            let mut guard = self.stream.lock();
            let stream = guard.as_mut().ok_or("apple-mca: stream not configured")?;
            if stream.running {
                return Ok(());
            }
            stream.running = true;
            stream.channel.clone()
        };
        let codec = self.playback_codec();

        self.early_start_serdes(cluster_base, cluster);
        self.enable_serdes(cluster_base);
        if let Err(error) = channel.start() {
            let mut guard = self.stream.lock();
            if let Some(stream) = guard.as_mut() {
                stream.running = false;
            }
            self.disable_serdes(cluster_base);
            return Err(dma_error_to_str(error));
        }
        if let Some(codec) = codec
            && let Err(error) = codec
                .set_playback_powered(true)
                .and_then(|_| codec.set_playback_muted(false))
        {
            let _ = channel.stop();
            let mut guard = self.stream.lock();
            if let Some(stream) = guard.as_mut() {
                stream.running = false;
            }
            self.disable_serdes(cluster_base);
            return Err(error);
        }
        Ok(())
    }

    fn stop(&self) -> Result<(), &'static str> {
        let cluster_base = self.cluster_base(APPLE_MCA_PLAYBACK_CLUSTER)?;
        let codec_result = self
            .playback_codec()
            .map(|codec| codec.set_playback_muted(true))
            .unwrap_or(Ok(()));
        let channel = {
            let mut guard = self.stream.lock();
            let Some(stream) = guard.as_mut() else {
                self.disable_serdes(cluster_base);
                codec_result?;
                return Ok(());
            };
            stream.running = false;
            Some(stream.channel.clone())
        };

        if let Some(channel) = channel {
            channel.stop().map_err(dma_error_to_str)?;
        }
        self.disable_serdes(cluster_base);
        codec_result?;
        Ok(())
    }

    fn release(&self) -> Result<(), &'static str> {
        let codec = self.playback_codec();
        self.stop()?;
        if self.stream.lock().is_some() {
            let cluster_base = self.cluster_base(APPLE_MCA_PLAYBACK_CLUSTER)?;
            self.disable_port(cluster_base);
        }
        *self.stream.lock() = None;
        if let Some(codec) = codec {
            codec.set_playback_powered(false)?;
        }
        Ok(())
    }

    fn submit_period(&self, pcm: &[u8]) -> Result<(), &'static str> {
        let mut guard = self.stream.lock();
        let stream = guard.as_mut().ok_or("apple-mca: stream not configured")?;
        stream.copy_period(pcm)
    }

    fn process_completions(&self) -> usize {
        let mut guard = self.stream.lock();
        let Some(stream) = guard.as_mut() else {
            return 0;
        };
        stream.take_completions()
    }

    fn max_in_flight_periods(&self) -> usize {
        self.stream
            .lock()
            .as_ref()
            .map(|stream| stream.period_count)
            .unwrap_or(4)
    }
}

fn cluster_count(device: &PlatformDeviceInfo) -> usize {
    let compatible = device.compatible();
    if compatible
        .iter()
        .any(|entry| *entry == "apple,t6000-mca" || *entry == "apple,t6020-mca")
    {
        APPLE_MCA_T6000_CLUSTERS
    } else {
        APPLE_MCA_T8103_CLUSTERS
    }
}

fn resolve_clocks(
    device: &PlatformDeviceInfo,
    count: usize,
) -> Result<Vec<ClkHandle>, &'static str> {
    let manager = DeviceManager::get_manager();
    let mut clocks: Vec<ClkHandle> = Vec::new();

    for index in 0..count {
        let clk = manager.resolve_clk_by_index(device, index)?;
        if clk.prepare_enable().is_err() {
            for clock in &clocks {
                clock.disable_unprepare();
            }
            return Err("apple-mca: failed to enable clock");
        }
        clocks.push(clk);
    }

    Ok(clocks)
}

fn resolve_dmas(device: &PlatformDeviceInfo) -> Result<Vec<AppleMcaDma>, &'static str> {
    let names = device
        .property("dma-names")
        .ok_or("apple-mca: missing dma-names")?
        .as_string_list()
        .ok_or("apple-mca: malformed dma-names")?;
    let manager = DeviceManager::get_manager();
    let mut dmas = Vec::new();

    for name in names {
        let channel = manager.resolve_dma_channel(device, name)?;
        dmas.push(AppleMcaDma {
            name: name.to_string(),
            channel,
        });
    }

    Ok(dmas)
}

fn map_resource(device: &PlatformDeviceInfo, index: usize) -> Result<(usize, usize), &'static str> {
    let resource = device
        .get_resources()
        .iter()
        .filter(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .nth(index)
        .ok_or("apple-mca: missing memory resource")?;
    let paddr = resource.start;
    let size = resource.end - resource.start + 1;
    let base = scarlet::vm::ioremap(paddr, size).map_err(|_| "apple-mca: ioremap failed")?;

    Ok((base, size))
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let phandle = read_phandle(device)?;
    let (base, size) = map_resource(device, 0)?;
    let (switch_base, switch_size) = map_resource(device, 1)?;
    let clusters = cluster_count(device);
    let clocks = resolve_clocks(device, clusters)?;
    let dmas = resolve_dmas(device)?;
    let dma_count = dmas.len();
    let mca = Arc::new(AppleMca::new(
        base,
        size,
        switch_base,
        switch_size,
        clocks,
        dmas,
    ));
    let dai_provider: Arc<dyn AudioDaiProvider> = mca.clone();
    DeviceManager::get_manager().register_audio_dai_provider(phandle, dai_provider);
    let audio_backend: Arc<dyn AudioPlaybackDevice> = mca.clone();
    let audio_name = register_playback_device(audio_backend);
    APPLE_MCA_DEVICES.lock().push(mca);

    early_println!(
        "[apple-mca] probed {} at phandle={:#x}, base={:#x}, switch={:#x}, clusters={}, dmas={}, audio={}",
        device.name(),
        phandle,
        base,
        switch_base,
        clusters,
        dma_count,
        audio_name
    );

    Ok(())
}

fn probe_macaudio_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let fdt = FdtManager::get_manager()
        .get_fdt()
        .ok_or("apple-macaudio: FDT is not available")?;
    let sound = fdt
        .find_node("/sound")
        .ok_or("apple-macaudio: missing /sound node")?;
    let manager = DeviceManager::get_manager();
    let mut speaker_seen = false;
    let mut attached = 0usize;

    for link in sound.children() {
        if !link.name.starts_with("dai-link") {
            continue;
        }
        let link_name = link
            .property("link-name")
            .and_then(|property| property.as_str())
            .unwrap_or(link.name);
        if link_name != "Speaker" {
            continue;
        }
        speaker_seen = true;

        let cpu_node = link
            .children()
            .find(|child| child.name == "cpu")
            .ok_or("apple-macaudio: missing Speaker CPU endpoint")?;
        let codec_node = link
            .children()
            .find(|child| child.name == "codec")
            .ok_or("apple-macaudio: missing Speaker codec endpoint")?;
        let (cpu_phandle, cpu_spec) = parse_sound_dai_property(
            cpu_node
                .property("sound-dai")
                .ok_or("apple-macaudio: missing Speaker CPU sound-dai")?
                .value,
        )?;
        let (codec_phandle, codec_spec) = parse_sound_dai_property(
            codec_node
                .property("sound-dai")
                .ok_or("apple-macaudio: missing Speaker codec sound-dai")?
                .value,
        )?;
        if !codec_spec.is_empty() {
            return Err("apple-macaudio: unsupported Speaker codec specifier");
        }

        let Some(provider) = manager.get_audio_dai_provider_by_phandle(cpu_phandle) else {
            early_println!(
                "[apple-macaudio] CPU DAI phandle {:#x} is not ready, deferring",
                cpu_phandle
            );
            return probe_defer();
        };
        if provider.sound_dai_cells() != cpu_spec.len() {
            return Err("apple-macaudio: invalid Speaker CPU sound-dai cells");
        }

        let Some(codec) = manager.get_audio_codec_by_phandle(codec_phandle) else {
            early_println!(
                "[apple-macaudio] codec phandle {:#x} is not ready, deferring",
                codec_phandle
            );
            return probe_defer();
        };

        provider.attach_playback_codec(&cpu_spec, codec)?;
        attached += 1;
        early_println!(
            "[apple-macaudio] attached Speaker link cpu={:#x} spec={:?} codec={:#x}",
            cpu_phandle,
            cpu_spec,
            codec_phandle
        );
    }

    if !speaker_seen {
        return Err("apple-macaudio: no Speaker link found");
    }
    if attached == 0 {
        return Err("apple-macaudio: no Speaker link attached");
    }

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-mca",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-mca",
            "apple,t8112-mca",
            "apple,t6000-mca",
            "apple,t6020-mca",
            "apple,mca",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Standard);

    let machine_driver = PlatformDeviceDriver::new(
        "apple-macaudio",
        probe_macaudio_fn,
        remove_fn,
        alloc::vec!["apple,macaudio"],
    );

    DeviceManager::get_manager()
        .register_driver(Box::new(machine_driver), DriverPriority::Standard);
}

scarlet::driver_initcall!(register_driver);

#[used]
static SCARLET_DRIVER_APPLE_MCA_ANCHOR: fn() = force_link;

/// Keep the external driver object linked into Scarlet module builds.
#[inline(never)]
pub fn force_link() {}
