#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet::sync::Mutex;

use scarlet::arch::mmio;
use scarlet::device::manager::{DeviceManager, DriverPriority};
use scarlet::device::platform::resource::PlatformDeviceResourceType;
use scarlet::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use scarlet::early_println;
use scarlet::vm;

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

pub struct AppleEfuse {
    base: usize,
}

impl AppleEfuse {
    fn new(base: usize) -> Self {
        Self { base }
    }

    pub fn read32(&self, offset: usize) -> u32 {
        // SAFETY: `self.base + offset` points to a mapped EFUSE MMIO region.
        unsafe { mmio::read32(self.base + offset) }
    }

    pub fn read_cell(&self, cell: &EfuseCell) -> u32 {
        cell.extract(self.read32(cell.offset))
    }
}

static EFUSE_REGISTRY: Mutex<Vec<Arc<AppleEfuse>>> = Mutex::new(Vec::new());

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

scarlet::driver_initcall!(register_apple_efuse_driver);

#[used]
static SCARLET_DRIVER_APPLE_EFUSE_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
