pub mod uart;

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::device::Device;

pub static mut DRIVER_TABLE: Vec<Box<dyn DeviceDriver>> = Vec::new();

pub trait DeviceDriver {
    fn name(&self) -> &'static str;
    fn match_device(&self, device: &dyn Device) -> bool;
    fn probe(&self, device: &dyn Device) -> Result<(), &'static str>;
}

pub struct PlatformDeviceDriver {
    name: &'static str,
    match_fn: fn(&dyn Device) -> bool,
    probe_fn: fn(&dyn Device) -> Result<(), &'static str>,
}

impl DeviceDriver for PlatformDeviceDriver {
    fn name(&self) -> &'static str {
        self.name
    }

    fn match_device(&self, device: &dyn Device) -> bool {
        (self.match_fn)(device)
    }

    fn probe(&self, device: &dyn Device) -> Result<(), &'static str> {
        (self.probe_fn)(device)
    }
}

#[allow(static_mut_refs)]
pub fn driver_register(driver: Box<dyn DeviceDriver>) {
    unsafe {
        DRIVER_TABLE.push(driver);
    }
}

#[cfg(test)]
mod tests {

    #[test_case]
    #[allow(static_mut_refs)]
    fn test_driver_register() {
        use super::*;
        let len = unsafe { DRIVER_TABLE.len() };
        let driver = Box::new(PlatformDeviceDriver {
            name: "test",
            match_fn: |_device| _device.name() == "test",
            probe_fn: |_device| Ok(()),
        });
        driver_register(driver);
        assert_eq!(unsafe { DRIVER_TABLE.len() }, len + 1);
    }
}