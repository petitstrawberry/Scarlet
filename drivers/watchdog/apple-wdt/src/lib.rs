#![no_std]

extern crate alloc;

use alloc::boxed::Box;

use scarlet::{
    arch::mmio,
    device::{
        DeviceInfo,
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_println,
};

const APPLE_WDT_WD1_CTRL: usize = 0x1c;

struct AppleWdt {
    base_addr: usize,
}

impl AppleWdt {
    const fn new(base_addr: usize) -> Self {
        Self { base_addr }
    }

    #[inline]
    fn ctrl_addr(&self) -> usize {
        self.base_addr + APPLE_WDT_WD1_CTRL
    }

    fn disable(&self) {
        // SAFETY: `base_addr` comes from an MMIO mapping of the watchdog register block.
        // Writing 0 to WD1 CTRL is the documented disable sequence for Apple Silicon.
        unsafe {
            mmio::write32(self.ctrl_addr(), 0);
        }

        // SAFETY: Same mapped register block as above. Readback confirms the posted write.
        let ctrl = unsafe { mmio::read32(self.ctrl_addr()) };
        early_println!("[apple-wdt] WD1 control after disable = {:#x}", ctrl);
    }
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resource = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("Apple watchdog: no memory resource found")?;

    let paddr = mem_resource.start;
    let size = mem_resource.end - mem_resource.start + 1;
    early_println!(
        "[apple-wdt] probe {} at paddr={:#x}, size={:#x}",
        device.name(),
        paddr,
        size
    );

    let base_addr = scarlet::vm::ioremap(paddr, size).map_err(|e| {
        early_println!("[apple-wdt] ioremap failed: {}", e);
        e
    })?;

    let watchdog = AppleWdt::new(base_addr);
    watchdog.disable();
    scarlet::vm::iounmap(base_addr);

    early_println!("[apple-wdt] watchdog disabled");
    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-wdt",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,wdt",
            "apple,t8103-wdt",
            "apple,t8112-wdt",
            "apple,t6000-wdt",
            "apple,t6020-wdt",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Critical);
}

scarlet::driver_initcall!(register_driver);

#[used]
static SCARLET_DRIVER_APPLE_WDT_ANCHOR: fn() = force_link;

#[inline(never)]
pub fn force_link() {}
