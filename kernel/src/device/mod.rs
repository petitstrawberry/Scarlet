//! Device module.
//! 
//! This module provides a framework for managing devices in the kernel.
//! It includes device information and driver management,
//! as well as platform-specific device handling.


pub mod manager;
pub mod fdt;
pub mod platform;
pub mod block;

extern crate alloc;
use core::any::Any;

use alloc::vec::Vec;

pub trait DeviceInfo {
    fn name(&self) -> &'static str;
    fn id(&self) -> usize;
    fn compatible(&self) -> Vec<&'static str>;
    fn as_any(&self) -> &dyn Any;
}

/// Device driver trait.
/// 
/// This trait defines the interface for device drivers in the kernel.
/// It includes methods for getting the driver's name,
/// matching the driver to devices, and handling device probing and removal.
pub trait DeviceDriver {
    fn name(&self) -> &'static str;
    fn match_table(&self) -> Vec<&'static str>;
    fn probe(&self, device: &dyn DeviceInfo) -> Result<(), &'static str>;
    fn remove(&self, device: &dyn DeviceInfo) -> Result<(), &'static str>;
}

/// Device type enumeration.
/// 
/// This enum defines the types of devices that can be managed by the kernel.
/// It includes block devices, character devices, network devices,
/// and generic devices.
/// 
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub enum DeviceType {
    Block,
    Char,
    Network,
    Generic,
    #[cfg(test)]
    NonExistent,
}

/// Device trait.
/// 
/// This trait defines the interface for devices in the kernel.
/// 
pub trait Device: Send + Sync {
    fn device_type(&self) -> DeviceType;
    fn name(&self) -> &'static str;
    fn id(&self) -> usize;
    fn as_any(&self) -> &dyn Any;
}

pub struct GenericDevice {
    device_type: DeviceType,
    name: &'static str,
    id: usize,
}

impl GenericDevice {
    pub fn new(name: &'static str, id: usize) -> Self {
        Self { device_type: DeviceType::Generic, name, id }
    }
}

impl Device for GenericDevice {
    fn device_type(&self) -> DeviceType {
        self.device_type
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn id(&self) -> usize {
        self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}