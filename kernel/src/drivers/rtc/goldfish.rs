//! Goldfish RTC platform driver for RISC-V QEMU virt systems.

use alloc::{boxed::Box, vec};

use crate::device::manager::{DeviceManager, DriverPriority};
use crate::device::platform::{
    PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
};
use crate::driver_initcall;

const TIME_LOW: usize = 0x00;
const TIME_HIGH: usize = 0x04;

fn reg_read(base: usize, offset: usize) -> u32 {
    unsafe { crate::arch::mmio::read32(base + offset) }
}

fn register_goldfish_rtc() {
    let driver = Box::new(PlatformDeviceDriver::new(
        "goldfish-rtc-driver",
        goldfish_rtc_probe,
        goldfish_rtc_remove,
        vec!["google,goldfish-rtc"],
    ));

    DeviceManager::get_manager().register_driver(driver, DriverPriority::Core);
}

fn goldfish_rtc_probe(device_info: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let memory_resource = device_info
        .get_resources()
        .iter()
        .find(|r| r.res_type == PlatformDeviceResourceType::MEM)
        .ok_or("No memory resource found for goldfish-rtc")?;

    let paddr = memory_resource.start;
    let size = memory_resource.end - memory_resource.start + 1;
    let base = crate::vm::ioremap(paddr, size).inspect_err(|e| {
        crate::early_println!(
            "goldfish-rtc: ioremap({:#x}, {:#x}) failed: {}",
            paddr,
            size,
            e
        );
    })?;

    let mono_before = crate::time::current_time_ns();
    // Reading TIME_LOW latches TIME_HIGH, so the low word must be read first.
    let lo = reg_read(base, TIME_LOW) as u64;
    let hi = reg_read(base, TIME_HIGH) as u64;
    let rtc_epoch_ns = (hi << 32) | lo;
    let mono_after = crate::time::current_time_ns();

    crate::time::initialize_wall_clock_from_rtc_sample(rtc_epoch_ns, mono_before, mono_after)
        .inspect_err(|e| {
            crate::early_println!("goldfish-rtc: failed to seed wall clock: {}", e);
        })?;

    crate::early_println!("goldfish-rtc: seeded wall clock");

    Ok(())
}

fn goldfish_rtc_remove(_device_info: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

#[cfg(target_arch = "riscv64")]
driver_initcall!(register_goldfish_rtc);
