//! Fixed-rate clock provider driver.
//!
//! This driver registers Device Tree `fixed-clock` nodes as clk providers with
//! zero specifier cells.

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;

use crate::device::DeviceInfo;
use crate::device::clk::{ClkError, ClkFixedRate, ClkHandle, ClkProvider};
use crate::device::manager::{DeviceManager, DriverPriority};
use crate::device::platform::{PlatformDeviceDriver, PlatformDeviceInfo};
use crate::driver_initcall;

/// Clock provider for a Device Tree `fixed-clock` node.
pub struct FixedClockProvider {
    clock_cells: usize,
    clk: ClkHandle,
}

impl FixedClockProvider {
    /// Create a fixed-clock provider.
    ///
    /// # Arguments
    ///
    /// * `clock_cells` - Number of clock specifier cells accepted by the provider.
    /// * `clk` - Fixed-rate clock handle returned by the provider.
    ///
    /// # Returns
    ///
    /// A fixed-clock provider instance.
    pub fn new(clock_cells: usize, clk: ClkHandle) -> Self {
        Self { clock_cells, clk }
    }
}

impl ClkProvider for FixedClockProvider {
    fn name(&self) -> &'static str {
        self.clk.name()
    }

    fn clock_cells(&self) -> usize {
        self.clock_cells
    }

    fn get_clk(&self, spec: &[u32]) -> Result<ClkHandle, ClkError> {
        if spec.len() == self.clock_cells {
            Ok(self.clk.clone())
        } else {
            Err(ClkError::InvalidSpecifier)
        }
    }
}

fn read_phandle(device: &PlatformDeviceInfo) -> Result<u32, &'static str> {
    device
        .property("phandle")
        .or_else(|| device.property("linux,phandle"))
        .and_then(|property| property.as_usize())
        .map(|value| value as u32)
        .ok_or("fixed-clock: missing phandle")
}

fn read_clock_name(device: &PlatformDeviceInfo) -> &'static str {
    if let Some(name) = device
        .property("clock-output-names")
        .and_then(|property| property.as_string_list())
        .and_then(|names| names.first().map(|name| String::from(*name)))
    {
        Box::leak(name.into_boxed_str())
    } else {
        device.name()
    }
}

fn register_fixed_clock_provider(
    manager: &DeviceManager,
    device: &PlatformDeviceInfo,
) -> Result<(), &'static str> {
    let rate = device
        .property("clock-frequency")
        .and_then(|property| property.as_usize())
        .ok_or("fixed-clock: missing clock-frequency")? as u64;
    let phandle = read_phandle(device)?;
    let clock_cells = device
        .property("#clock-cells")
        .and_then(|property| property.as_usize())
        .unwrap_or(0);
    let clock_name = read_clock_name(device);
    let clk = ClkHandle::new(Arc::new(ClkFixedRate::new(clock_name, rate)));
    let provider = Arc::new(FixedClockProvider::new(clock_cells, clk));

    manager.register_clk_provider(phandle, provider);
    Ok(())
}

fn probe_fn(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    register_fixed_clock_provider(DeviceManager::get_manager(), device)
}

fn remove_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}

fn register_driver() {
    let driver = PlatformDeviceDriver::new("fixed-clock", probe_fn, remove_fn, vec!["fixed-clock"]);
    DeviceManager::get_manager().register_driver(Box::new(driver), DriverPriority::Core);
}

driver_initcall!(register_driver);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::platform::PlatformDeviceProperty;

    fn be_cells(cells: &[u32]) -> alloc::vec::Vec<u8> {
        let mut bytes = alloc::vec::Vec::new();
        for cell in cells {
            bytes.extend_from_slice(&cell.to_be_bytes());
        }
        bytes
    }

    #[test_case]
    fn test_fixed_clock_probe_registers_provider() {
        let manager = DeviceManager::new_for_test();
        let device = PlatformDeviceInfo::new(
            "fixed-clock-test",
            0,
            vec!["fixed-clock"],
            vec![],
            vec![
                PlatformDeviceProperty::new("phandle", &be_cells(&[0x55])),
                PlatformDeviceProperty::new("#clock-cells", &be_cells(&[0])),
                PlatformDeviceProperty::new("clock-frequency", &be_cells(&[24_000_000])),
                PlatformDeviceProperty::new("clock-output-names", b"osc24m\0"),
            ],
            None,
        );

        assert!(register_fixed_clock_provider(&manager, &device).is_ok());
        let provider = manager.get_clk_provider_by_phandle(0x55).unwrap();
        assert_eq!(provider.clock_cells(), 0);
        assert_eq!(provider.get_clk(&[]).unwrap().rate(), 24_000_000);
    }
}
