//! Apple EFUSE read-only memory driver.
//!
//! Provides access to factory calibration data stored in on-chip eFUSE memory.
//! Used primarily to read ATC PHY tunables for USB-C/DisplayPort calibration
//! on Apple Silicon (t8103/M1, t6000/M1 Pro/Max).
//!
//! EFUSE is a simple MMIO region with 32-bit read-only words.
//! Calibration values are bitfields within these words, defined as
//! nvmem-cells in the device tree with `reg` (byte offset) and `bits`
//! (bit offset, bit count) properties.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::mmio;
use crate::device::manager::{DeviceManager, DriverPriority};
use crate::device::platform::resource::PlatformDeviceResourceType;
use crate::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use crate::driver_initcall;
use crate::early_println;
use crate::vm;

/// An EFUSE nvmem cell — a bitfield extracted from the EFUSE MMIO region.
#[derive(Debug, Clone)]
pub struct EfuseCell {
    pub name: String,
    pub offset: usize,
    pub bit_offset: u32,
    pub bit_count: u32,
}

impl EfuseCell {
    pub fn extract(&self, word: u32) -> u32 {
        let mask = (1u32 << self.bit_count) - 1;
        (word >> self.bit_offset) & mask
    }
}

/// Apple EFUSE driver instance.
pub struct AppleEfuse {
    base: usize,
}

impl AppleEfuse {
    fn new(base: usize) -> Self {
        Self { base }
    }

    /// Read a raw 32-bit word from the EFUSE region.
    pub fn read32(&self, offset: usize) -> u32 {
        // SAFETY: `self.base + offset` points to a mapped EFUSE MMIO region.
        unsafe { mmio::read32(self.base + offset) }
    }

    /// Read and extract a cell value.
    pub fn read_cell(&self, cell: &EfuseCell) -> u32 {
        cell.extract(self.read32(cell.offset))
    }
}

static EFUSE_REGISTRY: Mutex<Vec<Arc<AppleEfuse>>> = Mutex::new(Vec::new());

/// Get a probed EFUSE instance by index.
pub fn get_apple_efuse(id: u32) -> Option<Arc<AppleEfuse>> {
    EFUSE_REGISTRY.lock().get(id as usize).map(Arc::clone)
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let resource = device
        .get_resources()
        .iter()
        .find(|r| matches!(r.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-efuse: no memory resource")?;

    let paddr = resource.start;
    let size = resource
        .end
        .checked_sub(resource.start)
        .and_then(|v| v.checked_add(1))
        .ok_or("apple-efuse: invalid memory resource")?;

    let base = vm::ioremap(paddr, size).map_err(|_| "apple-efuse: ioremap failed")?;

    early_println!("[apple-efuse] probed at {:#x} ({} bytes)", paddr, size);

    EFUSE_REGISTRY.lock().push(Arc::new(AppleEfuse::new(base)));

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_apple_efuse_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-efuse",
        probe_fn,
        remove_fn,
        alloc::vec!["apple,t8103-efuses", "apple,t6000-efuses", "apple,efuses"],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

driver_initcall!(register_apple_efuse_driver);
