pub mod platform;

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
}


#[allow(static_mut_refs)]
pub fn driver_register(driver: Box<dyn DeviceDriver>) {
    unsafe {
        DRIVER_TABLE.push(driver);
    }
}
