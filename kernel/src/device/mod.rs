pub mod platform;
pub mod resource;

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

pub trait Device {
    fn name(&self) -> &'static str;
    fn id(&self) -> usize;
}

pub static mut DRIVER_TABLE: Vec<Box<dyn DeviceDriver>> = Vec::new();

pub trait DeviceDriver {
    fn name(&self) -> &'static str;
    fn match_device(&self, device: &dyn Device) -> bool;
    fn probe(&self, device: &dyn Device) -> Result<(), &'static str>;
    fn remove(&self, device: &dyn Device) -> Result<(), &'static str>;
}


#[allow(static_mut_refs)]
pub fn driver_register(driver: Box<dyn DeviceDriver>) {
    unsafe {
        DRIVER_TABLE.push(driver);
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::platform::*;

    #[test_case]
    #[allow(static_mut_refs)]
    fn test_driver_register() {
        let len = unsafe { DRIVER_TABLE.len() };
        let driver = Box::new(PlatformDeviceDriver::new(
            "test",
            Vec::new(),
            |device| device.name() == "test",
            |_device| Ok(()),
            |_device| Ok(()),
        ));
        driver_register(driver);
        assert_eq!(unsafe { DRIVER_TABLE.len() }, len + 1);
    }
}