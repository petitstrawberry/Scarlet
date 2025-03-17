pub mod manager;
pub mod platform;

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::mutex::Mutex;

pub trait Device {
    fn name(&self) -> &'static str;
    fn id(&self) -> usize;
}

pub static mut DRIVER_TABLE: Mutex<Vec<Box<dyn DeviceDriver>>> = Mutex::new(Vec::new());

pub trait DeviceDriver {
    fn name(&self) -> &'static str;
    fn match_device(&self, device: &dyn Device) -> bool;
    fn probe(&self, device: &dyn Device) -> Result<(), &'static str>;
    fn remove(&self, device: &dyn Device) -> Result<(), &'static str>;
}


#[allow(static_mut_refs)]
pub fn driver_register(driver: Box<dyn DeviceDriver>) {
    unsafe {
        DRIVER_TABLE.lock().push(driver);
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::platform::*;

    #[test_case]
    #[allow(static_mut_refs)]
    fn test_driver_register() {
        let len = unsafe { DRIVER_TABLE.lock().len() };
        let driver = Box::new(PlatformDeviceDriver::new(
            "test",
            Vec::new(),
            |device| device.name() == "test",
            |_device| Ok(()),
            |_device| Ok(()),
        ));
        driver_register(driver);

        let registered_driver = unsafe { &DRIVER_TABLE.lock()[len] };
        assert_eq!(registered_driver.name(), "test");
    }
}