//! Device module.
//! 
//! This module provides a framework for managing devices in the kernel.
//! It includes device information and driver management,
//! as well as platform-specific device handling.


pub mod manager;
pub mod fdt;
pub mod platform;
pub mod block;
pub mod virtio;

extern crate alloc;
use alloc::vec::Vec;

pub trait Device {
    fn name(&self) -> &'static str;
    fn id(&self) -> usize;
    fn compatible(&self) -> Vec<&'static str>;
}

/// Device driver trait.
/// 
/// This trait defines the interface for device drivers in the kernel.
/// It includes methods for getting the driver's name,
/// matching the driver to devices, and handling device probing and removal.
pub trait DeviceDriver {
    fn name(&self) -> &'static str;
    fn match_table(&self) -> Vec<&'static str>; // Change to Vec<&'static str>
    fn probe(&self, info: &dyn Device) -> Result<(), &'static str>;
    fn remove(&self, info: &dyn Device) -> Result<(), &'static str>;
}
