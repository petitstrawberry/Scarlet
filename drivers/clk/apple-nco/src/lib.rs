#![no_std]
#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use scarlet::{
    device::{
        DeviceInfo,
        clk::{Clk, ClkError, ClkHandle, ClkProvider},
        manager::{DeviceManager, DriverPriority},
        platform::{
            PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
        },
    },
    early_println,
};

const APPLE_NCO_CLOCK_CELLS: usize = 1;
const APPLE_NCO_T8103_CLOCKS: usize = 6;
const APPLE_NCO_T6000_CLOCKS: usize = 4;

struct AppleNcoClock {
    index: u32,
    parent: Option<ClkHandle>,
}

impl AppleNcoClock {
    fn new(index: u32, parent: Option<ClkHandle>) -> Self {
        Self { index, parent }
    }
}

impl Clk for AppleNcoClock {
    fn name(&self) -> &'static str {
        "apple-nco"
    }

    fn enable(&self) -> Result<(), ClkError> {
        Ok(())
    }

    fn disable(&self) {}

    fn is_enabled(&self) -> bool {
        true
    }

    fn recalc_rate(&self, parent_rate: u64) -> u64 {
        parent_rate
    }

    fn round_rate(&self, rate: u64, _parent_rate: u64) -> Result<u64, ClkError> {
        Ok(rate)
    }

    fn set_rate(&self, _rate: u64, _parent_rate: u64) -> Result<u64, ClkError> {
        Err(ClkError::Unsupported)
    }

    fn parent(&self) -> Option<ClkHandle> {
        self.parent.clone()
    }
}

struct AppleNcoProvider {
    base: usize,
    size: usize,
    clocks: Vec<ClkHandle>,
}

impl AppleNcoProvider {
    fn new(base: usize, size: usize, clock_count: usize, parent: Option<ClkHandle>) -> Self {
        let mut clocks = Vec::new();
        for index in 0..clock_count {
            clocks.push(ClkHandle::new(Arc::new(AppleNcoClock::new(
                index as u32,
                parent.clone(),
            ))));
        }

        Self { base, size, clocks }
    }
}

impl ClkProvider for AppleNcoProvider {
    fn name(&self) -> &'static str {
        "apple-nco"
    }

    fn clock_cells(&self) -> usize {
        APPLE_NCO_CLOCK_CELLS
    }

    fn get_clk(&self, spec: &[u32]) -> Result<ClkHandle, ClkError> {
        if spec.len() != APPLE_NCO_CLOCK_CELLS {
            return Err(ClkError::InvalidSpecifier);
        }

        let index = spec[0] as usize;
        self.clocks
            .get(index)
            .cloned()
            .ok_or(ClkError::ClockNotFound)
    }
}

fn read_phandle(device: &PlatformDeviceInfo) -> Result<u32, &'static str> {
    device
        .property("phandle")
        .or_else(|| device.property("linux,phandle"))
        .and_then(|property| property.as_usize())
        .map(|value| value as u32)
        .ok_or("apple-nco: missing phandle")
}

fn clock_count(device: &PlatformDeviceInfo) -> usize {
    let compatible = device.compatible();
    if compatible
        .iter()
        .any(|entry| *entry == "apple,t6000-nco" || *entry == "apple,t6020-nco")
    {
        APPLE_NCO_T6000_CLOCKS
    } else {
        APPLE_NCO_T8103_CLOCKS
    }
}

fn resolve_parent_clock(device: &PlatformDeviceInfo) -> Result<Option<ClkHandle>, &'static str> {
    if device.property("clocks").is_none() {
        return Ok(None);
    }

    let parent = DeviceManager::get_manager()
        .resolve_clk(device, "ref")
        .map_err(|_| "apple-nco: failed to resolve reference clock")?;
    parent
        .prepare_enable()
        .map_err(|_| "apple-nco: failed to enable reference clock")?;
    Ok(Some(parent))
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let mem_resource = device
        .get_resources()
        .iter()
        .find(|resource| matches!(resource.res_type, PlatformDeviceResourceType::MEM))
        .ok_or("apple-nco: no memory resource found")?;

    let paddr = mem_resource.start;
    let size = mem_resource.end - mem_resource.start + 1;
    let base = scarlet::vm::ioremap(paddr, size).map_err(|_| "apple-nco: ioremap failed")?;
    let phandle = read_phandle(device)?;
    let clock_cells = device
        .property("#clock-cells")
        .and_then(|property| property.as_usize())
        .unwrap_or(APPLE_NCO_CLOCK_CELLS);
    if clock_cells != APPLE_NCO_CLOCK_CELLS {
        return Err("apple-nco: unsupported #clock-cells");
    }

    let parent = resolve_parent_clock(device)?;
    let count = clock_count(device);
    let provider = Arc::new(AppleNcoProvider::new(base, size, count, parent));
    DeviceManager::get_manager().register_clk_provider(phandle, provider);

    early_println!(
        "[apple-nco] registered {} at paddr={:#x}, base={:#x}, clocks={}",
        device.name(),
        paddr,
        base,
        count
    );

    Ok(())
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new(
        "apple-nco",
        probe_fn,
        remove_fn,
        alloc::vec![
            "apple,t8103-nco",
            "apple,t8112-nco",
            "apple,t6000-nco",
            "apple,t6020-nco",
            "apple,nco",
        ],
    );

    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

scarlet::driver_initcall!(register_driver);

#[used]
static SCARLET_DRIVER_APPLE_NCO_ANCHOR: fn() = force_link;

/// Keep the external driver object linked into Scarlet module builds.
#[inline(never)]
pub fn force_link() {}
