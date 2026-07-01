#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use scarlet::sync::Mutex;

use scarlet::arch::mmio;
use scarlet::device::clk::ClkHandle;
use scarlet::device::spi::{SpiBus, SpiError, SpiTransfer, SpiTransferFlags};
use scarlet::device::{
    DeviceInfo,
    manager::{DeviceManager, DriverPriority, is_probe_defer, probe_defer},
    platform::{PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType},
};
use scarlet::time;
use scarlet::vm;

const REG_CTRL: usize = 0x000;
const REG_CFG: usize = 0x004;
const REG_STATUS: usize = 0x008;
const REG_PIN: usize = 0x00c;
const REG_TXDATA: usize = 0x010;
const REG_RXDATA: usize = 0x020;
const REG_CLKDIV: usize = 0x030;
const REG_RXCNT: usize = 0x034;
const REG_WORD_DELAY: usize = 0x038;
const REG_TXCNT: usize = 0x04c;
const REG_FIFOSTAT: usize = 0x10c;
const REG_IE_XFER: usize = 0x130;
const REG_IF_XFER: usize = 0x134;
const REG_IE_FIFO: usize = 0x138;
const REG_IF_FIFO: usize = 0x13c;
const REG_SHIFTCFG: usize = 0x150;
const REG_PINCFG: usize = 0x154;
const REG_DELAY_PRE: usize = 0x160;
const REG_SCKCFG: usize = 0x164;
const REG_DELAY_POST: usize = 0x168;

const CTRL_RUN: u32 = 1 << 0;
const CTRL_TX_RESET: u32 = 1 << 2;
const CTRL_RX_RESET: u32 = 1 << 3;

const CFG_CPHA: u32 = 1 << 1;
const CFG_CPOL: u32 = 1 << 2;
const CFG_MODE_SHIFT: u32 = 5;
const CFG_MODE_MASK: u32 = 0b11 << CFG_MODE_SHIFT;
const CFG_MODE_IRQ: u8 = 1;
const CFG_IE_RXCOMPLETE: u32 = 1 << 7;
const CFG_IE_TXRXTHRESH: u32 = 1 << 8;
const CFG_LSB_FIRST: u32 = 1 << 13;
const CFG_WORD_SIZE_SHIFT: u32 = 15;
const CFG_WORD_SIZE_MASK: u32 = 0b11 << CFG_WORD_SIZE_SHIFT;
const CFG_WORD_SIZE_8BIT: u32 = 0;
const CFG_FIFO_THRESH_SHIFT: u32 = 17;
const CFG_FIFO_THRESH_MASK: u32 = 0b11 << CFG_FIFO_THRESH_SHIFT;

const STATUS_RXCOMPLETE: u32 = 1 << 0;
const STATUS_TXCOMPLETE: u32 = 1 << 2;

const PIN_CS: u32 = 1 << 1;

const FIFOSTAT_TXFULL: u32 = 1 << 4;
const FIFOSTAT_LEVEL_TX_SHIFT: u32 = 8;
const FIFOSTAT_LEVEL_TX_MASK: u32 = 0xff << FIFOSTAT_LEVEL_TX_SHIFT;
const FIFOSTAT_RXEMPTY: u32 = 1 << 20;
const FIFOSTAT_LEVEL_RX_SHIFT: u32 = 24;
const FIFOSTAT_LEVEL_RX_MASK: u32 = 0xff << FIFOSTAT_LEVEL_RX_SHIFT;

const SHIFTCFG_CLK_ENABLE: u32 = 1 << 0;
const SHIFTCFG_CS_ENABLE: u32 = 1 << 1;
const SHIFTCFG_RX_FIXUP: u32 = 1 << 2;
const SHIFTCFG_TX_ENABLE: u32 = 1 << 10;
const SHIFTCFG_RX_ENABLE: u32 = 1 << 11;
const SHIFTCFG_OVERRIDE_CS: u32 = 1 << 24;

const PINCFG_KEEP_CS: u32 = 1 << 1;
const PINCFG_CS_IDLE_VAL: u32 = 1 << 9;

const DELAY_ENABLE: u32 = 1 << 0;
const DELAY_NO_INTERBYTE: u32 = 1 << 1;
const DELAY_SET_SCK: u32 = 1 << 4;
const DELAY_SET_MOSI: u32 = 1 << 6;
const DELAY_SCK_VAL: u32 = 1 << 8;
const DELAY_MOSI_VAL: u32 = 1 << 12;
const DELAY_CYCLES_SHIFT: u32 = 16;

const CLKDIV_MASK: u32 = 0x7ff;
const CLKDIV_MIN: u32 = 0x002;

const FIFO_DEPTH: usize = 16;
const TIMEOUT_MS: u64 = 200;
const TIMEOUT_US: u64 = TIMEOUT_MS * 1_000;
const POLL_INTERVAL_US: u64 = 5;
const DEFAULT_PARENT_CLOCK_HZ: u32 = 200_000_000;
const DEFAULT_BUS_SPEED_HZ: u32 = 1_000_000;
const TIMEOUT_LOG_LIMIT: u32 = 8;
const INIT_LOG_LIMIT: u32 = 8;
const XFER_LOG_LIMIT: u32 = 8;

static TIMEOUT_LOGS: AtomicU32 = AtomicU32::new(0);
static INIT_LOGS: AtomicU32 = AtomicU32::new(0);
static XFER_LOGS: AtomicU32 = AtomicU32::new(0);

struct AppleSpiInner {
    speed_hz: u32,
    mode: u8,
    lsb_first: bool,
}

pub struct AppleSpiController {
    base: usize,
    bus_number: u32,
    parent_clock_hz: u32,
    _bus_clk: Option<ClkHandle>,
    inner: Mutex<AppleSpiInner>,
    transfer_lock: Mutex<()>,
}

impl AppleSpiController {
    pub fn new(base: usize, bus_number: u32, bus_clk: Option<ClkHandle>) -> Result<Self, SpiError> {
        let parent_clock_hz = bus_clk
            .as_ref()
            .map(|clk| clk.rate() as u32)
            .filter(|rate| *rate != 0)
            .unwrap_or(DEFAULT_PARENT_CLOCK_HZ);
        let controller = Self {
            base,
            bus_number,
            parent_clock_hz,
            _bus_clk: bus_clk,
            inner: Mutex::new(AppleSpiInner {
                speed_hz: DEFAULT_BUS_SPEED_HZ,
                mode: 0,
                lsb_first: false,
            }),
            transfer_lock: Mutex::new(()),
        };
        controller.init_hardware()?;
        Ok(controller)
    }

    fn init_hardware(&self) -> Result<(), SpiError> {
        self.write_reg(REG_IE_XFER, 0);
        self.write_reg(REG_IE_FIFO, 0);
        self.clear_interrupt_flags();

        self.write_reg(REG_PIN, PIN_CS);
        let shiftcfg = self.read_reg(REG_SHIFTCFG) & !SHIFTCFG_OVERRIDE_CS;
        self.write_reg(REG_SHIFTCFG, shiftcfg);
        let pincfg = (self.read_reg(REG_PINCFG) & !PINCFG_CS_IDLE_VAL) | PINCFG_KEEP_CS;
        self.write_reg(REG_PINCFG, pincfg);

        let delay_cfg = Self::compose_delay(false, false, false, false, false, 0);
        self.write_reg(REG_DELAY_PRE, delay_cfg);
        self.write_reg(REG_DELAY_POST, delay_cfg);

        self.program_format(CFG_MODE_IRQ, false)?;
        let speed_hz = self.inner.lock().speed_hz;
        self.program_bus_speed(speed_hz)?;
        self.reset_fifos();
        if INIT_LOGS.fetch_add(1, Ordering::Relaxed) < INIT_LOG_LIMIT {
            scarlet::early_println!(
                "[apple-spi] init bus={} base={:#x} parent_clk={} pin={:#x} cfg={:#x} shiftcfg={:#x} pincfg={:#x} clkdiv={:#x} fifostat={:#x}",
                self.bus_number,
                self.base,
                self.parent_clock_hz,
                self.read_reg(REG_PIN),
                self.read_reg(REG_CFG),
                self.read_reg(REG_SHIFTCFG),
                self.read_reg(REG_PINCFG),
                self.read_reg(REG_CLKDIV),
                self.read_reg(REG_FIFOSTAT)
            );
        }
        Ok(())
    }

    fn read_reg(&self, offset: usize) -> u32 {
        unsafe { mmio::read32(self.base + offset) }
    }

    fn write_reg(&self, offset: usize, value: u32) {
        unsafe { mmio::write32(self.base + offset, value) }
    }

    fn compose_delay(
        enable: bool,
        no_interbyte: bool,
        set_sck: bool,
        set_mosi: bool,
        force_sck_high: bool,
        delay_cycles: u16,
    ) -> u32 {
        let mut value = ((delay_cycles as u32) << DELAY_CYCLES_SHIFT) & 0xffff_0000;
        if enable {
            value |= DELAY_ENABLE;
        }
        if no_interbyte {
            value |= DELAY_NO_INTERBYTE;
        }
        if set_sck {
            value |= DELAY_SET_SCK;
        }
        if set_mosi {
            value |= DELAY_SET_MOSI;
        }
        if force_sck_high {
            value |= DELAY_SCK_VAL;
        }
        if set_mosi {
            value |= DELAY_MOSI_VAL;
        }
        value
    }

    fn clear_interrupt_flags(&self) {
        self.write_reg(REG_IF_XFER, !0u32);
        self.write_reg(REG_IF_FIFO, !0u32);
        self.write_reg(REG_STATUS, !0u32);
    }

    fn reset_fifos(&self) {
        self.write_reg(REG_CTRL, CTRL_TX_RESET | CTRL_RX_RESET);
    }

    fn set_cs_active(&self) {
        self.write_reg(REG_PIN, 0);
    }

    fn set_cs_inactive(&self) {
        self.write_reg(REG_PIN, PIN_CS);
    }

    fn clkdiv_for_hz(parent_clock_hz: u32, hz: u32) -> Result<u32, SpiError> {
        if hz == 0 {
            return Err(SpiError::InvalidArg);
        }
        if parent_clock_hz == 0 {
            return Err(SpiError::InvalidArg);
        }

        let divisor = parent_clock_hz.div_ceil(hz);
        if divisor == 0 {
            return Err(SpiError::InvalidArg);
        }

        Ok(divisor.clamp(CLKDIV_MIN, CLKDIV_MASK))
    }

    fn clkdiv_to_speed(parent_clock_hz: u32, divider: u32) -> u32 {
        let clamped = core::cmp::min(divider, CLKDIV_MASK);
        parent_clock_hz / (clamped + 1)
    }

    fn program_bus_speed(&self, hz: u32) -> Result<(), SpiError> {
        let divider = Self::clkdiv_for_hz(self.parent_clock_hz, hz)?;
        self.write_reg(REG_CLKDIV, divider & CLKDIV_MASK);
        let actual = Self::clkdiv_to_speed(self.parent_clock_hz, divider);
        self.inner.lock().speed_hz = actual;
        Ok(())
    }

    fn program_format(&self, controller_mode: u8, lsb_first: bool) -> Result<(), SpiError> {
        if controller_mode > 3 {
            return Err(SpiError::InvalidArg);
        }

        let mut cfg = self.read_reg(REG_CFG);
        cfg &= !(CFG_CPHA
            | CFG_CPOL
            | CFG_MODE_MASK
            | CFG_IE_RXCOMPLETE
            | CFG_IE_TXRXTHRESH
            | CFG_LSB_FIRST
            | CFG_WORD_SIZE_MASK
            | CFG_FIFO_THRESH_MASK);

        cfg |= ((controller_mode as u32) << CFG_MODE_SHIFT) & CFG_MODE_MASK;
        if lsb_first {
            cfg |= CFG_LSB_FIRST;
        }
        cfg |= (CFG_WORD_SIZE_8BIT << CFG_WORD_SIZE_SHIFT) & CFG_WORD_SIZE_MASK;
        cfg |= (0 << CFG_FIFO_THRESH_SHIFT) & CFG_FIFO_THRESH_MASK;
        self.write_reg(REG_CFG, cfg);

        let mut inner = self.inner.lock();
        inner.mode = controller_mode;
        inner.lsb_first = lsb_first;
        Ok(())
    }

    fn program_shiftcfg(&self, tx_enable: bool, rx_enable: bool) {
        let mut shiftcfg = self.read_reg(REG_SHIFTCFG);
        shiftcfg &= !(SHIFTCFG_TX_ENABLE | SHIFTCFG_RX_ENABLE | SHIFTCFG_OVERRIDE_CS);
        shiftcfg |= SHIFTCFG_CLK_ENABLE | SHIFTCFG_CS_ENABLE | SHIFTCFG_RX_FIXUP;
        if tx_enable {
            shiftcfg |= SHIFTCFG_TX_ENABLE;
        }
        if rx_enable {
            shiftcfg |= SHIFTCFG_RX_ENABLE;
        }
        self.write_reg(REG_SHIFTCFG, shiftcfg);
    }

    fn tx_fifo_level(fifostat: u32) -> usize {
        ((fifostat & FIFOSTAT_LEVEL_TX_MASK) >> FIFOSTAT_LEVEL_TX_SHIFT) as usize
    }

    fn rx_fifo_level(fifostat: u32) -> usize {
        ((fifostat & FIFOSTAT_LEVEL_RX_MASK) >> FIFOSTAT_LEVEL_RX_SHIFT) as usize
    }

    fn can_accept_tx(fifostat: u32) -> bool {
        (fifostat & FIFOSTAT_TXFULL) == 0 && Self::tx_fifo_level(fifostat) < FIFO_DEPTH
    }

    fn has_rx_data(fifostat: u32) -> bool {
        (fifostat & FIFOSTAT_RXEMPTY) == 0 && Self::rx_fifo_level(fifostat) > 0
    }

    fn fill_tx_fifo(&self, tx_buf: &[u8], tx_written: &mut usize, tx_len: usize) {
        while *tx_written < tx_len {
            let fifostat = self.read_reg(REG_FIFOSTAT);
            if !Self::can_accept_tx(fifostat) {
                break;
            }

            let value = tx_buf.get(*tx_written).copied().unwrap_or(0);
            self.write_reg(REG_TXDATA, value as u32);
            *tx_written += 1;
        }
    }

    fn drain_rx_fifo(&self, rx_buf: &mut [u8], rx_read: &mut usize, rx_len: usize) {
        while *rx_read < rx_len {
            let fifostat = self.read_reg(REG_FIFOSTAT);
            if !Self::has_rx_data(fifostat) {
                break;
            }

            let value = self.read_reg(REG_RXDATA) as u8;
            rx_buf[*rx_read] = value;
            *rx_read += 1;
        }
    }

    fn run_segment(&self, tx_buf: &[u8], rx_buf: &mut [u8]) -> Result<(), SpiError> {
        let tx_len = tx_buf.len();
        let rx_len = rx_buf.len();

        self.reset_fifos();
        self.clear_interrupt_flags();
        self.write_reg(REG_TXCNT, tx_len as u32);
        self.write_reg(REG_RXCNT, rx_len as u32);
        self.program_shiftcfg(tx_len > 0, rx_len > 0);

        let mut tx_written = 0usize;
        let mut rx_read = 0usize;
        self.fill_tx_fifo(tx_buf, &mut tx_written, tx_len);

        self.write_reg(REG_CTRL, CTRL_RUN);

        let start = time::current_time();
        let mut last_tx_written = tx_written;
        let mut last_rx_read = rx_read;
        loop {
            self.drain_rx_fifo(rx_buf, &mut rx_read, rx_len);
            self.fill_tx_fifo(tx_buf, &mut tx_written, tx_len);

            if tx_written == tx_len && rx_read == rx_len {
                break;
            }

            if time::current_time().saturating_sub(start) >= TIMEOUT_US {
                let status = self.read_reg(REG_STATUS);
                let fifostat = self.read_reg(REG_FIFOSTAT);
                let if_xfer = self.read_reg(REG_IF_XFER);
                let if_fifo = self.read_reg(REG_IF_FIFO);
                let shiftcfg = self.read_reg(REG_SHIFTCFG);
                if TIMEOUT_LOGS.fetch_add(1, Ordering::Relaxed) < TIMEOUT_LOG_LIMIT {
                    scarlet::early_println!(
                        "[apple-spi] timeout bus={} tx={}/{} rx={}/{} status={:#x} txdone={} rxdone={} fifostat={:#x} txlvl={} rxlvl={} if_xfer={:#x} if_fifo={:#x} shiftcfg={:#x}",
                        self.bus_number,
                        tx_written,
                        tx_len,
                        rx_read,
                        rx_len,
                        status,
                        (status & STATUS_TXCOMPLETE) != 0,
                        (status & STATUS_RXCOMPLETE) != 0,
                        fifostat,
                        Self::tx_fifo_level(fifostat),
                        Self::rx_fifo_level(fifostat),
                        if_xfer,
                        if_fifo,
                        shiftcfg
                    );
                }
                self.write_reg(REG_CTRL, 0);
                return Err(SpiError::Timeout);
            }

            if tx_written == last_tx_written && rx_read == last_rx_read {
                time::udelay(POLL_INTERVAL_US);
            }
            last_tx_written = tx_written;
            last_rx_read = rx_read;
        }

        self.write_reg(REG_CTRL, 0);
        self.drain_rx_fifo(rx_buf, &mut rx_read, rx_len);
        if rx_read != rx_len {
            return Err(SpiError::FifoError);
        }

        if XFER_LOGS.fetch_add(1, Ordering::Relaxed) < XFER_LOG_LIMIT {
            scarlet::early_println!(
                "[apple-spi] xfer bus={} tx={} rx={} speed={} clkdiv={:#x} pin={:#x} status={:#x} fifostat={:#x} if_xfer={:#x} if_fifo={:#x} shiftcfg={:#x} rx0={:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
                self.bus_number,
                tx_len,
                rx_len,
                self.inner.lock().speed_hz,
                self.read_reg(REG_CLKDIV),
                self.read_reg(REG_PIN),
                self.read_reg(REG_STATUS),
                self.read_reg(REG_FIFOSTAT),
                self.read_reg(REG_IF_XFER),
                self.read_reg(REG_IF_FIFO),
                self.read_reg(REG_SHIFTCFG),
                rx_buf.first().copied().unwrap_or(0),
                rx_buf.get(1).copied().unwrap_or(0),
                rx_buf.get(2).copied().unwrap_or(0),
                rx_buf.get(3).copied().unwrap_or(0),
                rx_buf.get(4).copied().unwrap_or(0),
                rx_buf.get(5).copied().unwrap_or(0),
                rx_buf.get(6).copied().unwrap_or(0),
                rx_buf.get(7).copied().unwrap_or(0)
            );
        }

        self.clear_interrupt_flags();
        Ok(())
    }

    fn transfer_segment(&self, segment: &mut SpiTransfer) -> Result<(), SpiError> {
        if segment.data.is_empty() {
            return Err(SpiError::InvalidArg);
        }

        let is_read = segment.flags.contains(SpiTransferFlags::READ);
        let is_write = segment.flags.contains(SpiTransferFlags::WRITE);
        if !is_read && !is_write {
            return Err(SpiError::InvalidArg);
        }

        if segment.speed_hz != 0 {
            self.program_bus_speed(segment.speed_hz)?;
        }

        if is_read && is_write {
            let tx_buf = segment.data.clone();
            let mut rx_buf = alloc::vec![0u8; segment.data.len()];
            self.run_segment(&tx_buf, &mut rx_buf)?;
            segment.data.copy_from_slice(&rx_buf);
            return Ok(());
        }

        if is_read {
            let tx_dummy = alloc::vec![0u8; segment.data.len()];
            let mut rx_buf = alloc::vec![0u8; segment.data.len()];
            self.run_segment(&tx_dummy, &mut rx_buf)?;
            segment.data.copy_from_slice(&rx_buf);
            return Ok(());
        }

        let mut rx_dummy = alloc::vec![0u8; segment.data.len()];
        self.run_segment(&segment.data, &mut rx_dummy)
    }
}

impl SpiBus for AppleSpiController {
    fn transfer(&self, segments: &mut [SpiTransfer]) -> Result<(), SpiError> {
        if segments.is_empty() {
            return Err(SpiError::InvalidArg);
        }

        let _guard = self.transfer_lock.lock();
        self.set_cs_active();
        let mut result = Ok(());
        for segment in segments.iter_mut() {
            if segment.delay_before_us != 0 {
                time::udelay(segment.delay_before_us);
            }
            if let Err(err) = self.transfer_segment(segment) {
                result = Err(err);
                break;
            }
            if segment.delay_after_us != 0 {
                time::udelay(segment.delay_after_us);
            }
        }
        self.set_cs_inactive();

        result
    }

    fn set_bus_speed(&self, hz: u32) -> Result<(), SpiError> {
        self.program_bus_speed(hz)
    }

    fn bus_speed(&self) -> u32 {
        self.inner.lock().speed_hz
    }

    fn bus_number(&self) -> u32 {
        self.bus_number
    }
}

/// Probe an Apple SPI controller, optionally enabling its bus clock before MMIO setup.
fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resources: Vec<_> = device
        .get_resources()
        .iter()
        .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    let resource = mem_resources
        .first()
        .ok_or("apple-spi: no memory resource")?;

    let paddr = resource.start;
    let size = resource
        .end
        .checked_sub(resource.start)
        .and_then(|v| v.checked_add(1))
        .ok_or("apple-spi: invalid memory resource")?;

    let base = vm::ioremap(paddr, size).map_err(|_| "apple-spi: ioremap failed")?;

    // TODO: Confirm Apple SPI DT clock-names on all supported SoCs; current bring-up uses "bus".
    let bus_clk = match DeviceManager::get_manager().resolve_clk(device, "bus") {
        Ok(handle) => {
            let _ = handle.prepare_enable();
            Some(handle)
        }
        Err(e) if is_probe_defer(e) || e == "clk: provider not found" => {
            scarlet::early_println!("[apple-spi] bus clock provider not ready, deferring");
            return probe_defer();
        }
        Err(
            e @ ("clk: clock-names missing" | "clk: clocks missing" | "clk: clock name not found"),
        ) => {
            scarlet::early_println!("[apple-spi] warning: bus clock unavailable: {}", e);
            None
        }
        Err(e) => {
            scarlet::early_println!("[apple-spi] bus clock lookup failed: {}", e);
            return Err(e);
        }
    };

    let bus_number = device
        .property("reg")
        .and_then(|p| p.as_usize())
        .map(|v| v as u32)
        .unwrap_or(device.id() as u32);

    let phandle = device
        .property("phandle")
        .and_then(|p| p.as_usize())
        .map(|v| v as u32)
        .ok_or("apple-spi: no phandle")?;

    let controller =
        AppleSpiController::new(base, bus_number, bus_clk).map_err(|_| "apple-spi: init failed")?;
    let pinctrl0 = device
        .property("pinctrl-0")
        .and_then(|p| p.as_usize())
        .unwrap_or(0);
    scarlet::early_println!(
        "[apple-spi] probed name={} paddr={:#x} base={:#x} phandle={:#x} pinctrl0={:#x} bus={} parent_clk={} speed={}",
        device.name(),
        paddr,
        base,
        phandle,
        pinctrl0,
        controller.bus_number(),
        controller.parent_clock_hz,
        controller.bus_speed()
    );
    let bus: Arc<dyn SpiBus> = Arc::new(controller);

    DeviceManager::get_manager().register_spi_bus(phandle, bus);
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_apple_spi_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-spi",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,t8103-spi", "apple,t8112-spi", "apple,spi"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_apple_spi_driver);

#[used]
static SCARLET_DRIVER_APPLE_SPI_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
