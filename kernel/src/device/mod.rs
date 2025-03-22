pub mod manager;
pub mod fdt;
pub mod platform;

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::mutex::Mutex;

pub trait DeviceInfo {
    fn name(&self) -> &'static str;
    fn id(&self) -> usize;
    fn compatible(&self) -> Vec<&'static str>;
}

pub trait DeviceDriver {
    fn name(&self) -> &'static str;
    fn match_table(&self) -> Vec<&'static str>; // Change to Vec<&'static str>
    fn probe(&self, info: &dyn DeviceInfo) -> Result<(), &'static str>;
    fn remove(&self, info: &dyn DeviceInfo) -> Result<(), &'static str>;
}
