#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use scarlet::sync::Mutex;

use scarlet::{
    arch::mmio,
    device::{
        DeviceInfo,
        dma::{
            DmaBusWidth, DmaChannel, DmaController, DmaCyclicConfig, DmaDirection, DmaError,
            DmaSpec,
        },
        events::InterruptCapableDevice,
        iommu::{
            DmaAddr as IommuDmaAddr, DmaContext, IommuDomainConfig, IommuDomainType, IommuMapFlags,
        },
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_println,
    interrupt::{InterruptId, InterruptResult},
};

const NCHANNELS_MAX: usize = 64;
const IRQ_NOUTPUTS: usize = 4;
const SRAM_BLOCK: usize = 2048;

const RING_WRITE_SLOT_MASK: u32 = 0x3;
const RING_READ_SLOT_MASK: u32 = 0x30;
const RING_READ_SLOT_SHIFT: u32 = 4;
const RING_FULL: u32 = 1 << 9;
const RING_EMPTY: u32 = 1 << 8;
const RING_ERR: u32 = 1 << 10;

const STATUS_DESC_DONE: u32 = 1 << 0;
const STATUS_ERR: u32 = 1 << 6;

const FLAG_DESC_NOTIFY: u32 = 1 << 16;

const REG_TX_START: usize = 0x0000;
const REG_TX_STOP: usize = 0x0004;
const REG_RX_START: usize = 0x0008;
const REG_RX_STOP: usize = 0x000c;
const REG_TX_SRAM_SIZE: usize = 0x0094;
const REG_RX_SRAM_SIZE: usize = 0x0098;

const REG_CHAN_CTL_RST_RINGS: u32 = 1 << 0;

const BUS_WIDTH_WORD_SIZE_MASK: u32 = 0x0f;
const BUS_WIDTH_FRAME_SIZE_MASK: u32 = 0xf0;
const BUS_WIDTH_8BIT: u32 = 0x00;
const BUS_WIDTH_16BIT: u32 = 0x01;
const BUS_WIDTH_32BIT: u32 = 0x02;
const BUS_WIDTH_FRAME_2_WORDS: u32 = 0x10;
const BUS_WIDTH_FRAME_4_WORDS: u32 = 0x20;

const APPLE_ADMAC_DMA_CELLS: usize = 1;

#[derive(Debug)]
struct AppleAdmacSram {
    size: u32,
    allocated: u32,
}

struct AppleAdmacTransfer {
    config: DmaCyclicConfig,
    dma_addr: IommuDmaAddr,
    dma_len: usize,
    submitted_pos: usize,
    reclaimed_pos: usize,
}

impl AppleAdmacTransfer {
    fn new(config: DmaCyclicConfig, dma_addr: IommuDmaAddr, dma_len: usize) -> Self {
        Self {
            config,
            dma_addr,
            dma_len,
            submitted_pos: 0,
            reclaimed_pos: 0,
        }
    }
}

struct AppleAdmacChannelState {
    in_use: AtomicBool,
    running: AtomicBool,
    transfer: Mutex<Option<AppleAdmacTransfer>>,
    carveout: Mutex<Option<u32>>,
    completed_periods: AtomicUsize,
    error: AtomicBool,
}

impl AppleAdmacChannelState {
    fn new() -> Self {
        Self {
            in_use: AtomicBool::new(false),
            running: AtomicBool::new(false),
            transfer: Mutex::new(None),
            carveout: Mutex::new(None),
            completed_periods: AtomicUsize::new(0),
            error: AtomicBool::new(false),
        }
    }
}

struct AppleAdmacInner {
    base: usize,
    size: usize,
    channel_count: usize,
    irq_index: usize,
    interrupt_id: Mutex<Option<InterruptId>>,
    tx_sram: Mutex<AppleAdmacSram>,
    rx_sram: Mutex<AppleAdmacSram>,
    channels: Vec<AppleAdmacChannelState>,
    dma_context: DmaContext,
}

/// Apple ADMAC DMA controller.
pub struct AppleAdmac {
    inner: Arc<AppleAdmacInner>,
}

impl AppleAdmac {
    /// Create an ADMAC controller wrapper.
    ///
    /// # Arguments
    ///
    /// * `base` - Kernel virtual address of the ADMAC MMIO region.
    /// * `size` - Size of the mapped MMIO region in bytes.
    /// * `channel_count` - Number of channels reported by firmware.
    /// * `tx_sram_size` - TX SRAM size reported by hardware.
    /// * `rx_sram_size` - RX SRAM size reported by hardware.
    /// * `irq_index` - ADMAC IRQ output index used by the platform resource.
    /// * `dma_context` - DMA mapping context for this ADMAC requester.
    ///
    /// # Returns
    ///
    /// A new ADMAC controller instance.
    pub fn new(
        base: usize,
        size: usize,
        channel_count: usize,
        tx_sram_size: u32,
        rx_sram_size: u32,
        irq_index: usize,
        dma_context: DmaContext,
    ) -> Self {
        let mut channels = Vec::new();
        for _ in 0..channel_count {
            channels.push(AppleAdmacChannelState::new());
        }

        Self {
            inner: Arc::new(AppleAdmacInner {
                base,
                size,
                channel_count,
                irq_index,
                interrupt_id: Mutex::new(None),
                tx_sram: Mutex::new(AppleAdmacSram {
                    size: tx_sram_size,
                    allocated: 0,
                }),
                rx_sram: Mutex::new(AppleAdmacSram {
                    size: rx_sram_size,
                    allocated: 0,
                }),
                channels,
                dma_context,
            }),
        }
    }

    fn set_interrupt_id(&self, interrupt_id: InterruptId) {
        *self.inner.interrupt_id.lock() = Some(interrupt_id);
    }

    fn channel_state(&self, index: usize) -> Result<&AppleAdmacChannelState, DmaError> {
        self.inner
            .channels
            .get(index)
            .ok_or(DmaError::ChannelNotFound)
    }

    fn read_reg(&self, offset: usize) -> u32 {
        // SAFETY: `self.inner.base` is an ioremap'd ADMAC MMIO region and
        // all offsets are fixed controller register offsets.
        unsafe { mmio::read32(self.inner.base + offset) }
    }

    fn write_reg(&self, offset: usize, value: u32) {
        // SAFETY: `self.inner.base` is an ioremap'd ADMAC MMIO region and
        // all offsets are fixed controller register offsets.
        unsafe { mmio::write32(self.inner.base + offset, value) }
    }

    fn modify_reg(&self, offset: usize, mask: u32, value: u32) {
        let current = self.read_reg(offset);
        self.write_reg(offset, (current & !mask) | (value & mask));
    }

    fn channel_direction(index: usize) -> DmaDirection {
        if index & 1 == 0 {
            DmaDirection::MemToDev
        } else {
            DmaDirection::DevToMem
        }
    }

    fn chan_ctl_reg(index: usize) -> usize {
        0x8000 + index * 0x200
    }

    fn chan_intstatus_reg(index: usize, irq_index: usize) -> usize {
        0x8010 + index * 0x200 + irq_index * 4
    }

    fn chan_intmask_reg(index: usize, irq_index: usize) -> usize {
        0x8020 + index * 0x200 + irq_index * 4
    }

    fn bus_width_reg(index: usize) -> usize {
        0x8040 + index * 0x200
    }

    fn chan_sram_carveout_reg(index: usize) -> usize {
        0x8050 + index * 0x200
    }

    fn chan_fifoctl_reg(index: usize) -> usize {
        0x8054 + index * 0x200
    }

    fn residue_reg(index: usize) -> usize {
        0x8064 + index * 0x200
    }

    fn desc_ring_reg(index: usize) -> usize {
        0x8070 + index * 0x200
    }

    fn report_ring_reg(index: usize) -> usize {
        0x8074 + index * 0x200
    }

    fn desc_write_reg(index: usize) -> usize {
        0x10000 + (index / 2) * 0x4 + (index & 1) * 0x4000
    }

    fn report_read_reg(index: usize) -> usize {
        0x10100 + (index / 2) * 0x4 + (index & 1) * 0x4000
    }

    fn tx_intstate_reg(index: usize) -> usize {
        0x0030 + index * 4
    }

    fn rx_intstate_reg(index: usize) -> usize {
        0x0040 + index * 4
    }

    fn global_intstate_reg(index: usize) -> usize {
        0x0050 + index * 4
    }

    fn start_bit(channel_index: usize) -> u32 {
        1u32 << (channel_index / 2)
    }

    fn alloc_sram_carveout(&self, direction: DmaDirection) -> Result<u32, DmaError> {
        let sram = match direction {
            DmaDirection::MemToDev => &self.inner.tx_sram,
            DmaDirection::DevToMem => &self.inner.rx_sram,
            DmaDirection::MemToMem => return Err(DmaError::InvalidConfig),
        };
        let mut sram = sram.lock();
        let block_count = ((sram.size as usize) / SRAM_BLOCK).min(u32::BITS as usize);
        if block_count == 0 {
            return Err(DmaError::HardwareError);
        }

        for index in 0..block_count {
            let bit = 1u32 << index;
            if sram.allocated & bit == 0 {
                sram.allocated |= bit;
                let base = (index * SRAM_BLOCK) as u32;
                return Ok(((SRAM_BLOCK as u32) << 16) | base);
            }
        }

        Err(DmaError::ChannelBusy)
    }

    fn free_sram_carveout(&self, direction: DmaDirection, carveout: u32) {
        let sram = match direction {
            DmaDirection::MemToDev => &self.inner.tx_sram,
            DmaDirection::DevToMem => &self.inner.rx_sram,
            DmaDirection::MemToMem => return,
        };
        let mut sram = sram.lock();
        let base = (carveout & 0xffff) as usize;
        if base >= sram.size as usize {
            return;
        }
        let index = base / SRAM_BLOCK;
        if index < u32::BITS as usize {
            sram.allocated &= !(1u32 << index);
        }
    }

    fn configure_channel(&self, index: usize, config: DmaCyclicConfig) -> Result<(), DmaError> {
        let peripheral = config.peripheral.ok_or(DmaError::InvalidConfig)?;
        let mut bus_width = self.read_reg(Self::bus_width_reg(index))
            & !(BUS_WIDTH_WORD_SIZE_MASK | BUS_WIDTH_FRAME_SIZE_MASK);
        let word_size = match peripheral.width {
            DmaBusWidth::Width1 => {
                bus_width |= BUS_WIDTH_8BIT;
                1usize
            }
            DmaBusWidth::Width2 => {
                bus_width |= BUS_WIDTH_16BIT;
                2usize
            }
            DmaBusWidth::Width4 => {
                bus_width |= BUS_WIDTH_32BIT;
                4usize
            }
            DmaBusWidth::Width8 => return Err(DmaError::InvalidConfig),
        };

        match peripheral.burst_len {
            1 => {}
            2 => bus_width |= BUS_WIDTH_FRAME_2_WORDS,
            4 => bus_width |= BUS_WIDTH_FRAME_4_WORDS,
            _ => return Err(DmaError::InvalidConfig),
        }

        self.write_reg(Self::bus_width_reg(index), bus_width);
        self.write_reg(
            Self::chan_fifoctl_reg(index),
            ((0x30 * word_size) as u32) << 16 | ((0x18 * word_size) as u32),
        );
        Ok(())
    }

    fn reset_rings(&self, index: usize) {
        self.write_reg(Self::chan_ctl_reg(index), REG_CHAN_CTL_RST_RINGS);
        self.write_reg(Self::chan_ctl_reg(index), 0);
    }

    fn enable_channel_interrupts(&self, index: usize) {
        let irq_index = self.inner.irq_index;
        self.write_reg(
            Self::chan_intstatus_reg(index, irq_index),
            STATUS_DESC_DONE | STATUS_ERR,
        );
        self.write_reg(
            Self::chan_intmask_reg(index, irq_index),
            STATUS_DESC_DONE | STATUS_ERR,
        );
    }

    fn disable_channel_interrupts(&self, index: usize) {
        self.modify_reg(
            Self::chan_intmask_reg(index, self.inner.irq_index),
            STATUS_DESC_DONE | STATUS_ERR,
            0,
        );
    }

    fn start_channel(&self, index: usize) {
        self.enable_channel_interrupts(index);
        match Self::channel_direction(index) {
            DmaDirection::MemToDev => self.write_reg(REG_TX_START, Self::start_bit(index)),
            DmaDirection::DevToMem => self.write_reg(REG_RX_START, Self::start_bit(index)),
            DmaDirection::MemToMem => {}
        }
    }

    fn stop_channel(&self, index: usize) {
        match Self::channel_direction(index) {
            DmaDirection::MemToDev => self.write_reg(REG_TX_STOP, Self::start_bit(index)),
            DmaDirection::DevToMem => self.write_reg(REG_RX_STOP, Self::start_bit(index)),
            DmaDirection::MemToMem => {}
        }
        self.disable_channel_interrupts(index);
    }

    fn map_flags(direction: DmaDirection) -> IommuMapFlags {
        match direction {
            DmaDirection::MemToDev => IommuMapFlags::READ | IommuMapFlags::COHERENT,
            DmaDirection::DevToMem => IommuMapFlags::WRITE | IommuMapFlags::COHERENT,
            DmaDirection::MemToMem => {
                IommuMapFlags::READ | IommuMapFlags::WRITE | IommuMapFlags::COHERENT
            }
        }
    }

    fn map_cyclic_config(
        &self,
        config: DmaCyclicConfig,
    ) -> Result<(DmaCyclicConfig, IommuDmaAddr), DmaError> {
        let dma_addr = self
            .inner
            .dma_context
            .map_phys(
                config.buffer_addr,
                config.buffer_len,
                Self::map_flags(config.direction),
            )
            .map_err(|_| DmaError::HardwareError)?;
        let dma_buffer_addr = usize::try_from(dma_addr).map_err(|_| DmaError::InvalidConfig)?;

        Ok((
            DmaCyclicConfig {
                buffer_addr: dma_buffer_addr,
                ..config
            },
            dma_addr,
        ))
    }

    fn unmap_transfer(&self, transfer: &AppleAdmacTransfer) {
        let _ = self
            .inner
            .dma_context
            .unmap(transfer.dma_addr, transfer.dma_len);
    }

    fn write_one_desc(
        &self,
        index: usize,
        transfer: &mut AppleAdmacTransfer,
    ) -> Result<(), DmaError> {
        let config = transfer.config;
        let offset = transfer.submitted_pos % config.buffer_len;
        let addr = config
            .buffer_addr
            .checked_add(offset)
            .ok_or(DmaError::InvalidConfig)?;
        let buffer_end = config
            .buffer_addr
            .checked_add(config.buffer_len)
            .ok_or(DmaError::InvalidConfig)?;
        if addr
            .checked_add(config.period_len)
            .ok_or(DmaError::InvalidConfig)?
            > buffer_end
        {
            return Err(DmaError::InvalidConfig);
        }

        let desc_reg = Self::desc_write_reg(index);
        self.write_reg(desc_reg, addr as u32);
        self.write_reg(desc_reg, (addr >> 32) as u32);
        self.write_reg(desc_reg, config.period_len as u32);
        self.write_reg(desc_reg, FLAG_DESC_NOTIFY);

        let wrap_len = config
            .buffer_len
            .checked_mul(2)
            .ok_or(DmaError::InvalidConfig)?;
        transfer.submitted_pos = (transfer.submitted_pos + config.period_len) % wrap_len;
        Ok(())
    }

    fn write_available_descs(
        &self,
        index: usize,
        transfer: &mut AppleAdmacTransfer,
    ) -> Result<(), DmaError> {
        for _ in 0..4 {
            if self.read_reg(Self::desc_ring_reg(index)) & RING_FULL != 0 {
                break;
            }
            self.write_one_desc(index, transfer)?;
        }
        Ok(())
    }

    fn ring_occupied_slots(ring: u32) -> usize {
        let write_slot = (ring & RING_WRITE_SLOT_MASK) as usize;
        let read_slot = ((ring & RING_READ_SLOT_MASK) >> RING_READ_SLOT_SHIFT) as usize;
        if write_slot != read_slot {
            (write_slot + 4 - read_slot) % 4
        } else if ring & RING_FULL != 0 {
            4
        } else {
            0
        }
    }

    fn read_residue_locked(&self, index: usize, transfer: &AppleAdmacTransfer) -> usize {
        let ring1 = self.read_reg(Self::report_ring_reg(index));
        let residue1 = self.read_reg(Self::residue_reg(index));
        let ring2 = self.read_reg(Self::report_ring_reg(index));
        let residue2 = self.read_reg(Self::residue_reg(index));
        let reports = if residue2 > residue1 {
            Self::ring_occupied_slots(ring1) + 1
        } else {
            Self::ring_occupied_slots(ring2)
        };
        let pos = transfer
            .reclaimed_pos
            .saturating_add(transfer.config.period_len.saturating_mul(reports + 1))
            .saturating_sub(residue2 as usize);

        transfer.config.buffer_len - (pos % transfer.config.buffer_len)
    }

    fn drain_reports(&self, index: usize) -> usize {
        let mut count = 0usize;
        for _ in 0..4 {
            if self.read_reg(Self::report_ring_reg(index)) & RING_EMPTY != 0 {
                break;
            }
            let count_low = self.read_reg(Self::report_read_reg(index));
            let count_high = self.read_reg(Self::report_read_reg(index));
            let unknown = self.read_reg(Self::report_read_reg(index));
            let flags = self.read_reg(Self::report_read_reg(index));
            let _ = (count_low, count_high, unknown, flags);
            count += 1;
        }
        count
    }

    fn handle_status_err(&self, index: usize) {
        let mut handled = false;
        if self.read_reg(Self::desc_ring_reg(index)) & RING_ERR != 0 {
            self.write_reg(Self::desc_ring_reg(index), RING_ERR);
            handled = true;
        }
        if self.read_reg(Self::report_ring_reg(index)) & RING_ERR != 0 {
            self.write_reg(Self::report_ring_reg(index), RING_ERR);
            handled = true;
        }
        if !handled {
            self.modify_reg(
                Self::chan_intmask_reg(index, self.inner.irq_index),
                STATUS_ERR,
                0,
            );
        }
        if let Some(state) = self.inner.channels.get(index) {
            state.error.store(true, Ordering::Release);
        }
    }

    fn handle_status_desc_done(&self, index: usize) {
        self.write_reg(
            Self::chan_intstatus_reg(index, self.inner.irq_index),
            STATUS_DESC_DONE,
        );

        let reports = self.drain_reports(index);
        if reports == 0 {
            return;
        }

        if let Some(state) = self.inner.channels.get(index) {
            let mut transfer = state.transfer.lock();
            if let Some(transfer) = transfer.as_mut() {
                transfer.reclaimed_pos += reports * transfer.config.period_len;
                transfer.reclaimed_pos %= 2 * transfer.config.buffer_len;
                state.completed_periods.fetch_add(reports, Ordering::AcqRel);
                if self.write_available_descs(index, transfer).is_err() {
                    state.error.store(true, Ordering::Release);
                    self.stop_channel(index);
                    state.running.store(false, Ordering::Release);
                }
            }
        }
    }

    fn handle_channel_interrupt(&self, index: usize) {
        let cause = self.read_reg(Self::chan_intstatus_reg(index, self.inner.irq_index));
        if cause & STATUS_ERR != 0 {
            self.handle_status_err(index);
        }
        if cause & STATUS_DESC_DONE != 0 {
            self.handle_status_desc_done(index);
        }
    }
}

impl DmaController for AppleAdmac {
    fn name(&self) -> &'static str {
        "apple-admac"
    }

    fn dma_cells(&self) -> usize {
        APPLE_ADMAC_DMA_CELLS
    }

    fn request_channel(&self, spec: &DmaSpec) -> Result<Arc<dyn DmaChannel>, DmaError> {
        if spec.cells.len() != APPLE_ADMAC_DMA_CELLS {
            return Err(DmaError::InvalidSpec);
        }

        let index = spec.cells[0] as usize;
        if index >= self.inner.channel_count {
            return Err(DmaError::ChannelNotFound);
        }

        let state = self.channel_state(index)?;
        if state
            .in_use
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(DmaError::ChannelBusy);
        }

        let direction = Self::channel_direction(index);
        let carveout = match self.alloc_sram_carveout(direction) {
            Ok(carveout) => carveout,
            Err(error) => {
                state.in_use.store(false, Ordering::Release);
                return Err(error);
            }
        };
        *state.carveout.lock() = Some(carveout);
        self.write_reg(Self::chan_sram_carveout_reg(index), carveout);

        Ok(Arc::new(AppleAdmacChannel {
            controller: self.clone(),
            index,
        }))
    }
}

impl InterruptCapableDevice for AppleAdmac {
    fn handle_interrupt(&self) -> InterruptResult<()> {
        let irq_index = self.inner.irq_index;
        let mut tx_state = self.read_reg(Self::tx_intstate_reg(irq_index));
        let mut rx_state = self.read_reg(Self::rx_intstate_reg(irq_index));
        let global_state = self.read_reg(Self::global_intstate_reg(irq_index));

        if tx_state == 0 && rx_state == 0 && global_state == 0 {
            return Ok(());
        }

        let mut channel = 0usize;
        while channel < self.inner.channel_count {
            if tx_state & 1 != 0 {
                self.handle_channel_interrupt(channel);
            }
            tx_state >>= 1;
            channel += 2;
        }

        channel = 1;
        while channel < self.inner.channel_count {
            if rx_state & 1 != 0 {
                self.handle_channel_interrupt(channel);
            }
            rx_state >>= 1;
            channel += 2;
        }

        if global_state != 0 {
            self.write_reg(Self::global_intstate_reg(irq_index), u32::MAX);
        }

        Ok(())
    }

    fn interrupt_id(&self) -> Option<InterruptId> {
        *self.inner.interrupt_id.lock()
    }
}

impl Clone for AppleAdmac {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

struct AppleAdmacChannel {
    controller: AppleAdmac,
    index: usize,
}

impl Drop for AppleAdmacChannel {
    fn drop(&mut self) {
        if let Ok(state) = self.controller.channel_state(self.index) {
            self.controller.stop_channel(self.index);
            self.controller.reset_rings(self.index);
            if let Some(transfer) = state.transfer.lock().take() {
                self.controller.unmap_transfer(&transfer);
            }
            state.running.store(false, Ordering::Release);
            state.completed_periods.store(0, Ordering::Release);
            state.error.store(false, Ordering::Release);
            if let Some(carveout) = state.carveout.lock().take() {
                self.controller
                    .free_sram_carveout(AppleAdmac::channel_direction(self.index), carveout);
            }
            state.in_use.store(false, Ordering::Release);
        }
    }
}

impl DmaChannel for AppleAdmacChannel {
    fn name(&self) -> &'static str {
        "apple-admac-channel"
    }

    fn prepare_cyclic(&self, config: DmaCyclicConfig) -> Result<(), DmaError> {
        config.validate()?;
        let state = self.controller.channel_state(self.index)?;
        if state.running.load(Ordering::Acquire) {
            return Err(DmaError::ChannelBusy);
        }
        if config.direction != AppleAdmac::channel_direction(self.index) {
            return Err(DmaError::InvalidConfig);
        }

        let old_transfer = state.transfer.lock().take();
        if let Some(transfer) = old_transfer {
            self.controller.unmap_transfer(&transfer);
        }

        let (mapped_config, dma_addr) = self.controller.map_cyclic_config(config)?;
        if let Err(error) = self.controller.configure_channel(self.index, mapped_config) {
            let _ = self
                .controller
                .inner
                .dma_context
                .unmap(dma_addr, config.buffer_len);
            return Err(error);
        }
        state.completed_periods.store(0, Ordering::Release);
        state.error.store(false, Ordering::Release);
        {
            let mut transfer = state.transfer.lock();
            *transfer = Some(AppleAdmacTransfer::new(
                mapped_config,
                dma_addr,
                config.buffer_len,
            ));
        }
        Ok(())
    }

    fn start(&self) -> Result<(), DmaError> {
        let state = self.controller.channel_state(self.index)?;
        if state.error.load(Ordering::Acquire) {
            return Err(DmaError::HardwareError);
        }
        if state.running.load(Ordering::Acquire) {
            return Ok(());
        }

        {
            let mut transfer = state.transfer.lock();
            let transfer = transfer.as_mut().ok_or(DmaError::NotPrepared)?;
            self.controller.reset_rings(self.index);
            self.controller
                .write_reg(AppleAdmac::chan_ctl_reg(self.index), 0);
            self.controller.write_one_desc(self.index, transfer)?;
        }

        state.running.store(true, Ordering::Release);
        self.controller.start_channel(self.index);
        let mut transfer = state.transfer.lock();
        if let Some(transfer) = transfer.as_mut()
            && self
                .controller
                .write_available_descs(self.index, transfer)
                .is_err()
        {
            state.error.store(true, Ordering::Release);
            state.running.store(false, Ordering::Release);
            self.controller.stop_channel(self.index);
            return Err(DmaError::InvalidConfig);
        }

        Ok(())
    }

    fn stop(&self) -> Result<(), DmaError> {
        let state = self.controller.channel_state(self.index)?;
        self.controller.stop_channel(self.index);
        self.controller.reset_rings(self.index);
        state.running.store(false, Ordering::Release);
        Ok(())
    }

    fn pause(&self) -> Result<(), DmaError> {
        let state = self.controller.channel_state(self.index)?;
        self.controller.stop_channel(self.index);
        state.running.store(false, Ordering::Release);
        Ok(())
    }

    fn resume(&self) -> Result<(), DmaError> {
        let state = self.controller.channel_state(self.index)?;
        if state.transfer.lock().is_none() {
            return Err(DmaError::NotPrepared);
        }
        if state.error.load(Ordering::Acquire) {
            return Err(DmaError::HardwareError);
        }
        self.controller.start_channel(self.index);
        state.running.store(true, Ordering::Release);
        Ok(())
    }

    fn residue(&self) -> Result<usize, DmaError> {
        let state = self.controller.channel_state(self.index)?;
        let transfer = state.transfer.lock();
        let transfer = transfer.as_ref().ok_or(DmaError::NotPrepared)?;
        Ok(self.controller.read_residue_locked(self.index, transfer))
    }

    fn take_completed_periods(&self) -> usize {
        self.controller
            .channel_state(self.index)
            .map(|state| state.completed_periods.swap(0, Ordering::AcqRel))
            .unwrap_or(0)
    }

    fn is_running(&self) -> bool {
        self.controller
            .channel_state(self.index)
            .map(|state| state.running.load(Ordering::Acquire))
            .unwrap_or(false)
    }
}

fn read_phandle(device: &PlatformDeviceInfo) -> Result<u32, &'static str> {
    device
        .property("phandle")
        .or_else(|| device.property("linux,phandle"))
        .and_then(|property| property.as_usize())
        .map(|value| value as u32)
        .ok_or("apple-admac: missing phandle")
}

fn read_be_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < 4 {
        return None;
    }

    Some(u32::from_be_bytes(bytes[..4].try_into().ok()?))
}

fn first_irq_resource(
    device: &PlatformDeviceInfo,
) -> Result<&scarlet::device::platform::resource::PlatformDeviceResource, &'static str> {
    let mut fallback = None;
    for resource in device
        .get_resources()
        .iter()
        .filter(|resource| matches!(resource.res_type, PlatformDeviceResourceType::IRQ))
    {
        if fallback.is_none() {
            fallback = Some(resource);
        }
        if resource.irq_metadata.is_some() {
            return Ok(resource);
        }
    }

    fallback.ok_or("apple-admac: no IRQ resource found")
}

fn irq_output_index(device: &PlatformDeviceInfo) -> Option<usize> {
    let property = device.property("interrupts-extended")?;
    let mut offset = 0usize;
    let mut output = 0usize;
    while offset + 4 <= property.value().len() {
        let phandle = read_be_u32(&property.value()[offset..offset + 4])?;
        offset += 4;
        if phandle == 0 {
            output += 1;
            continue;
        }

        return Some(output.min(IRQ_NOUTPUTS - 1));
    }

    None
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resource = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-admac: no memory resource found")?;

    let paddr = mem_resource.start;
    let size = mem_resource.end - mem_resource.start + 1;
    let base = scarlet::vm::ioremap(paddr, size).map_err(|_| "apple-admac: ioremap failed")?;
    let phandle = read_phandle(device)?;
    let channel_count = device
        .property("dma-channels")
        .and_then(|property| property.as_usize())
        .ok_or("apple-admac: missing dma-channels")?;
    if channel_count == 0 || channel_count > NCHANNELS_MAX {
        return Err("apple-admac: invalid dma-channels");
    }

    let dma_cells = device
        .property("#dma-cells")
        .and_then(|property| property.as_usize())
        .unwrap_or(APPLE_ADMAC_DMA_CELLS);

    if dma_cells != APPLE_ADMAC_DMA_CELLS {
        return Err("apple-admac: unsupported #dma-cells");
    }

    let irq_resource = first_irq_resource(device)?;
    let dma_context = DeviceManager::get_manager().resolve_platform_dma_context(
        device,
        IommuDomainConfig {
            domain_type: IommuDomainType::Dma,
            iova_base: 0,
            iova_size: 1u64 << 36,
        },
    )?;
    // SAFETY: `base` is an ioremap'd ADMAC MMIO region and these offsets are
    // fixed hardware register offsets.
    let tx_sram_size = unsafe { mmio::read32(base + REG_TX_SRAM_SIZE) };
    // SAFETY: `base` is an ioremap'd ADMAC MMIO region and these offsets are
    // fixed hardware register offsets.
    let rx_sram_size = unsafe { mmio::read32(base + REG_RX_SRAM_SIZE) };
    let irq_index = irq_output_index(device).unwrap_or_else(|| {
        device
            .get_resources()
            .iter()
            .filter(|resource| matches!(resource.res_type, PlatformDeviceResourceType::IRQ))
            .position(|resource| core::ptr::eq(resource, irq_resource))
            .unwrap_or(0)
            .min(IRQ_NOUTPUTS - 1)
    });

    let controller = Arc::new(AppleAdmac::new(
        base,
        size,
        channel_count,
        tx_sram_size,
        rx_sram_size,
        irq_index,
        dma_context,
    ));
    let interrupt_id = scarlet::interrupt::register_and_enable_platform_irq_device(
        irq_resource,
        controller.clone(),
        scarlet::arch::get_cpu().get_cpuid() as u32,
    )
    .map_err(|_| "apple-admac: failed to register IRQ handler")?;
    controller.set_interrupt_id(interrupt_id);
    DeviceManager::get_manager().register_dma_controller(phandle, controller);

    early_println!(
        "[apple-admac] registered {} at paddr={:#x}, base={:#x}, channels={}, irq={}, tx-sram={}, rx-sram={}",
        device.name(),
        paddr,
        base,
        channel_count,
        interrupt_id,
        tx_sram_size,
        rx_sram_size
    );

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-admac",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-admac",
            "apple,t8112-admac",
            "apple,t6000-admac",
            "apple,t6020-admac",
            "apple,admac",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_driver);

#[used]
static SCARLET_DRIVER_APPLE_ADMAC_ANCHOR: fn() = force_link;

/// Keep the external driver object linked into Scarlet module builds.
#[inline(never)]
pub fn force_link() {}
