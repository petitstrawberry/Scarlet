//! ARM PrimeCell PL031 RTC platform driver for AArch64 systems.

use alloc::{boxed::Box, vec};

use crate::device::manager::{DeviceManager, DriverPriority};
use crate::device::platform::{
    PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
};
use crate::driver_initcall;

const DR: usize = 0x00;
const CR: usize = 0x0c;
const CR_EN: u32 = 0x1;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

fn reg_read(base: usize, offset: usize) -> u32 {
    unsafe { crate::arch::mmio::read32(base + offset) }
}

fn reg_write(base: usize, offset: usize, value: u32) {
    unsafe { crate::arch::mmio::write32(base + offset, value) }
}

fn register_pl031_rtc() {
    // Match only "arm,pl031", never the generic "arm,primecell": PL061/PL011
    // are also PrimeCell and would be falsely bound to this RTC driver.
    let driver = Box::new(PlatformDeviceDriver::new(
        "pl031-rtc-driver",
        pl031_rtc_probe,
        pl031_rtc_remove,
        vec!["arm,pl031"],
    ));

    DeviceManager::get_manager().register_driver(driver, DriverPriority::Core);
}

fn pl031_rtc_probe(device_info: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let memory_resource = device_info
        .get_resources()
        .iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("No memory resource found for pl031-rtc")?;

    let paddr = memory_resource.start;
    let size = memory_resource.end - memory_resource.start + 1;
    let base = crate::vm::ioremap(paddr, size).inspect_err(|e| {
        crate::early_println!(
            "pl031-rtc: ioremap({:#x}, {:#x}) failed: {}",
            paddr,
            size,
            e
        );
    })?;

    let mono_before = crate::time::current_time_ns();
    reg_write(base, CR, reg_read(base, CR) | CR_EN);
    let rtc_epoch_ns = (reg_read(base, DR) as u64) * NANOS_PER_SECOND;
    let mono_after = crate::time::current_time_ns();

    crate::time::initialize_wall_clock_from_rtc_sample(rtc_epoch_ns, mono_before, mono_after)
        .inspect_err(|e| {
            crate::early_println!("pl031-rtc: failed to seed wall clock: {}", e);
        })?;

    crate::early_println!("pl031-rtc: seeded wall clock");

    Ok(())
}

fn pl031_rtc_remove(_device_info: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

#[cfg(target_arch = "aarch64")]
driver_initcall!(register_pl031_rtc);
