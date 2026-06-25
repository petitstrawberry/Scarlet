#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::arch::mmio;
use scarlet::device::clk::ClkHandle;
use scarlet::device::i2c::{I2cAddress, I2cBus, I2cError, I2cMessage, I2cMessageFlags};
use scarlet::device::{
    DeviceInfo,
    manager::{DeviceManager, DriverPriority, is_probe_defer, probe_defer},
    platform::{PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType},
};
use scarlet::time;
use scarlet::vm;

const REG_MTXFIFO: usize = 0x00;
const REG_MRXFIFO: usize = 0x04;
const REG_MCNT: usize = 0x08;
const REG_XFSTA: usize = 0x0c;
const REG_SADDR: usize = 0x10;
const REG_SMSTA: usize = 0x14;
const REG_IMASK: usize = 0x18;
const REG_CTL: usize = 0x1c;
const REG_REV: usize = 0x28;
const REG_FIFOCTL: usize = 0x44;

const MTXFIFO_DATA_MASK: u32 = 0xff;
const MTXFIFO_START: u32 = 1 << 8;
const MTXFIFO_STOP: u32 = 1 << 9;
const MTXFIFO_READ: u32 = 1 << 10;

const MRXFIFO_DATA_MASK: u32 = 0xff;
const MRXFIFO_EMPTY: u32 = 1 << 8;

const SMSTA_XIP: u32 = 1 << 28;
const _SMSTA_XEN: u32 = 1 << 27;
const SMSTA_JAM: u32 = 1 << 24;
const SMSTA_MTO: u32 = 1 << 23;
const SMSTA_MTA: u32 = 1 << 22;
const SMSTA_MTN: u32 = 1 << 21;
const SMSTA_ERR_MASK: u32 = SMSTA_JAM | SMSTA_MTO | SMSTA_MTA | SMSTA_MTN;

const CTL_EN: u32 = 1 << 11;
const CTL_MRR: u32 = 1 << 10;
const CTL_MTR: u32 = 1 << 9;
const CTL_UJM: u32 = 1 << 8;
const CTL_CLK_MASK: u32 = 0xff;

const DEFAULT_BUS_HZ: u32 = 100_000;
const REF_CLOCK_HZ: u32 = 24_000_000;
const TXN_TIMEOUT_US: u64 = 100_000;
const POLL_INTERVAL_US: u64 = 10;

struct AppleI2cInner {
    bus_hz: u32,
    hw_rev: u32,
}

/// Apple PA Semi I2C master controller.
pub struct AppleI2cController {
    base: usize,
    bus_number: u32,
    _bus_clk: Option<ClkHandle>,
    inner: Mutex<AppleI2cInner>,
    transfer_lock: Mutex<()>,
}

impl AppleI2cController {
    /// Create a controller instance and initialize hardware.
    pub fn new(base: usize, bus_number: u32, bus_clk: Option<ClkHandle>) -> Result<Self, I2cError> {
        let controller = Self {
            base,
            bus_number,
            _bus_clk: bus_clk,
            inner: Mutex::new(AppleI2cInner {
                bus_hz: DEFAULT_BUS_HZ,
                hw_rev: 0,
            }),
            transfer_lock: Mutex::new(()),
        };
        controller.init_hardware()?;
        Ok(controller)
    }

    fn init_hardware(&self) -> Result<(), I2cError> {
        let hw_rev = self.read_reg(REG_REV);
        {
            let mut inner = self.inner.lock();
            inner.hw_rev = hw_rev;
        }

        self.write_reg(REG_IMASK, 0);
        self.write_reg(REG_SADDR, 0);
        self.write_reg(REG_FIFOCTL, 0);
        self.clear_status();
        self.clear_fifos();
        self.program_bus_speed(DEFAULT_BUS_HZ)?;
        Ok(())
    }

    fn read_reg(&self, offset: usize) -> u32 {
        // SAFETY: `self.base` is an ioremap'd MMIO region and `offset` values
        // are fixed controller register offsets.
        unsafe { mmio::read32(self.base + offset) }
    }

    fn write_reg(&self, offset: usize, value: u32) {
        // SAFETY: `self.base` is an ioremap'd MMIO region and `offset` values
        // are fixed controller register offsets.
        unsafe { mmio::write32(self.base + offset, value) }
    }

    fn clear_status(&self) {
        self.write_reg(REG_SMSTA, !0u32);
        self.write_reg(REG_XFSTA, !0u32);
    }

    fn clear_fifos(&self) {
        let inner = self.inner.lock();
        let mut ctl = CTL_MRR | CTL_MTR | CTL_UJM;
        ctl |= self.divider_to_ctl_clock(Self::divider_for_speed(inner.bus_hz));
        if inner.hw_rev >= 6 {
            ctl |= CTL_EN;
        }
        drop(inner);
        self.write_reg(REG_CTL, ctl);
    }

    fn divider_to_ctl_clock(&self, divider: u8) -> u32 {
        (divider as u32) & CTL_CLK_MASK
    }

    fn divider_for_speed(hz: u32) -> u8 {
        let clamped_hz = hz.max(1);
        let mut divider = REF_CLOCK_HZ / (16 * clamped_hz);
        if divider == 0 {
            divider = 1;
        }
        if divider > u8::MAX as u32 {
            divider = u8::MAX as u32;
        }
        divider as u8
    }

    fn program_bus_speed(&self, hz: u32) -> Result<(), I2cError> {
        if hz == 0 {
            return Err(I2cError::InvalidArg);
        }

        let divider = Self::divider_for_speed(hz);
        let mut inner = self.inner.lock();
        inner.bus_hz = hz;

        let mut ctl = CTL_UJM | self.divider_to_ctl_clock(divider);
        if inner.hw_rev >= 6 {
            ctl |= CTL_EN;
        }
        drop(inner);

        self.write_reg(REG_CTL, ctl);
        Ok(())
    }

    fn map_smsta_error(smsta: u32) -> I2cError {
        if smsta & SMSTA_MTN != 0 {
            I2cError::Nack
        } else if smsta & SMSTA_MTA != 0 {
            I2cError::ArbitrationLost
        } else if smsta & SMSTA_MTO != 0 {
            I2cError::Timeout
        } else {
            I2cError::BusError
        }
    }

    fn poll_transfer_done(&self) -> Result<(), I2cError> {
        let start = time::current_time();
        loop {
            let smsta = self.read_reg(REG_SMSTA);
            if smsta & SMSTA_ERR_MASK != 0 {
                return Err(Self::map_smsta_error(smsta));
            }

            if smsta & SMSTA_XIP == 0 {
                return Ok(());
            }

            let now = time::current_time();
            if now.saturating_sub(start) > TXN_TIMEOUT_US {
                return Err(I2cError::Timeout);
            }

            time::udelay(POLL_INTERVAL_US);
        }
    }

    fn wait_rx_available(&self) -> Result<u8, I2cError> {
        let start = time::current_time();
        loop {
            let r = self.read_reg(REG_MRXFIFO);
            if r & MRXFIFO_EMPTY == 0 {
                return Ok((r & MRXFIFO_DATA_MASK) as u8);
            }

            let smsta = self.read_reg(REG_SMSTA);
            if smsta & SMSTA_ERR_MASK != 0 {
                return Err(Self::map_smsta_error(smsta));
            }

            let now = time::current_time();
            if now.saturating_sub(start) > TXN_TIMEOUT_US {
                return Err(I2cError::Timeout);
            }

            time::udelay(POLL_INTERVAL_US);
        }
    }

    fn validate_message(msg: &I2cMessage) -> Result<(), I2cError> {
        if msg.addr.is_ten_bit() {
            return Err(I2cError::InvalidArg);
        }
        if msg.flags.contains(I2cMessageFlags::READ) && msg.data.len() > u8::MAX as usize {
            return Err(I2cError::InvalidArg);
        }
        Ok(())
    }

    fn start_address_byte(addr: I2cAddress, read: bool) -> Result<u8, I2cError> {
        match addr {
            I2cAddress::SevenBit(a) => {
                if a > 0x7f {
                    return Err(I2cError::InvalidArg);
                }
                Ok((a << 1) | u8::from(read))
            }
            I2cAddress::TenBit(_) => Err(I2cError::InvalidArg),
        }
    }

    fn transfer_write(&self, msg: &I2cMessage, start: bool, stop: bool) -> Result<(), I2cError> {
        let addr = Self::start_address_byte(msg.addr, false)?;

        if start {
            self.write_reg(REG_MTXFIFO, MTXFIFO_START | (addr as u32));
        }

        if msg.data.is_empty() {
            if stop {
                self.write_reg(REG_MTXFIFO, MTXFIFO_STOP);
            }
            self.poll_transfer_done()?;
            return Ok(());
        }

        for (index, byte) in msg.data.iter().enumerate() {
            let is_last = index == msg.data.len() - 1;
            let mut value = (*byte as u32) & MTXFIFO_DATA_MASK;
            if !start && index == 0 {
                value |= MTXFIFO_START;
                value = (value & !MTXFIFO_DATA_MASK) | (addr as u32);
            }
            if stop && is_last {
                value |= MTXFIFO_STOP;
            }
            self.write_reg(REG_MTXFIFO, value);
        }

        self.poll_transfer_done()
    }

    fn transfer_read(&self, msg: &mut I2cMessage, start: bool, stop: bool) -> Result<(), I2cError> {
        let addr = Self::start_address_byte(msg.addr, true)?;
        if start {
            self.write_reg(REG_MTXFIFO, MTXFIFO_START | (addr as u32));
        }

        let mut cmd = MTXFIFO_READ | ((msg.data.len() as u32) & MTXFIFO_DATA_MASK);
        if stop {
            cmd |= MTXFIFO_STOP;
        }
        self.write_reg(REG_MTXFIFO, cmd);

        self.poll_transfer_done()?;

        for byte in msg.data.iter_mut() {
            *byte = self.wait_rx_available()?;
        }

        Ok(())
    }
}

impl I2cBus for AppleI2cController {
    fn transfer(&self, msgs: &mut [I2cMessage]) -> Result<(), I2cError> {
        if msgs.is_empty() {
            return Err(I2cError::InvalidArg);
        }

        let _guard = self.transfer_lock.lock();

        self.clear_fifos();
        self.clear_status();

        let total = msgs.len();
        for (index, msg) in msgs.iter_mut().enumerate() {
            Self::validate_message(msg)?;
            let is_last = index == total - 1;
            let start = if index == 0 {
                true
            } else {
                !msg.flags.contains(I2cMessageFlags::NOSTART)
            };
            let stop = msg.flags.contains(I2cMessageFlags::STOP) || is_last;

            if msg.flags.contains(I2cMessageFlags::READ) {
                self.transfer_read(msg, start, stop)?;
            } else {
                self.transfer_write(msg, start, stop)?;
            }
        }

        let smsta = self.read_reg(REG_SMSTA);
        if smsta & SMSTA_ERR_MASK != 0 {
            return Err(Self::map_smsta_error(smsta));
        }

        let _ = self.read_reg(REG_MCNT);
        self.clear_status();
        Ok(())
    }

    fn set_bus_speed(&self, hz: u32) -> Result<(), I2cError> {
        self.program_bus_speed(hz)
    }

    fn bus_speed(&self) -> u32 {
        self.inner.lock().bus_hz
    }

    fn bus_number(&self) -> u32 {
        self.bus_number
    }
}

/// Probe an Apple I2C controller, optionally enabling its bus clock before MMIO setup.
fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resources: Vec<_> = device
        .get_resources()
        .iter()
        .filter(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .collect();

    let resource = mem_resources
        .first()
        .ok_or("apple-i2c: no memory resource")?;

    let paddr = resource.start;
    let size = resource
        .end
        .checked_sub(resource.start)
        .and_then(|v| v.checked_add(1))
        .ok_or("apple-i2c: invalid memory resource")?;

    let base = vm::ioremap(paddr, size).map_err(|_| "apple-i2c: ioremap failed")?;

    // TODO: Confirm Apple I2C DT clock-names on all supported SoCs; current bring-up uses "bus".
    let bus_clk = match DeviceManager::get_manager().resolve_clk(device, "bus") {
        Ok(handle) => {
            let _ = handle.prepare_enable();
            Some(handle)
        }
        Err(e) if is_probe_defer(e) || e == "clk: provider not found" => {
            scarlet::early_println!("[apple-i2c] bus clock provider not ready, deferring");
            return probe_defer();
        }
        Err(
            e @ ("clk: clock-names missing" | "clk: clocks missing" | "clk: clock name not found"),
        ) => {
            scarlet::early_println!("[apple-i2c] warning: bus clock unavailable: {}", e);
            None
        }
        Err(e) => {
            scarlet::early_println!("[apple-i2c] bus clock lookup failed: {}", e);
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
        .ok_or("apple-i2c: no phandle")?;

    let controller = AppleI2cController::new(base, bus_number, bus_clk)
        .map_err(|_| "apple-i2c: controller initialization failed")?;
    let bus: Arc<dyn I2cBus> = Arc::new(controller);

    DeviceManager::get_manager().register_i2c_bus(phandle, bus);
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_apple_i2c_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-i2c",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,t8103-i2c", "apple,t8112-i2c", "apple,i2c"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_apple_i2c_driver);

#[used]
static SCARLET_DRIVER_APPLE_I2C_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
