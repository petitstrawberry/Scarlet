//! Device module.
//! 
//! This module provides a framework for managing devices in the kernel.
//! It includes device information and driver management,
//! as well as platform-specific device handling.


pub mod manager;
pub mod fdt;
pub mod platform;
pub mod block;
pub mod char;
pub mod graphics;
pub mod network;
pub mod events;

extern crate alloc;
use core::any::Any;

use alloc::vec::Vec;
use crate::object::capability::{ControlOps, MemoryMappingOps};

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
    Graphics,
    Generic,
    #[cfg(test)]
    NonExistent,
}

/// Device trait.
/// 
/// This trait defines the interface for devices in the kernel.
/// Device IDs are assigned by DeviceManager when devices are registered.
/// All devices must support control operations through the ControlOps trait
/// and memory mapping operations through the MemoryMappingOps trait.
/// 
pub trait Device: Send + Sync + ControlOps + MemoryMappingOps {
    fn device_type(&self) -> DeviceType;
    fn name(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    
    /// Cast to CharDevice if this device is a character device
    fn as_char_device(&self) -> Option<&dyn char::CharDevice> {
        None
    }
    
    /// Cast to BlockDevice if this device is a block device  
    fn as_block_device(&self) -> Option<&dyn block::BlockDevice> {
        None
    }
    
    /// Cast to GraphicsDevice if this device is a graphics device
    fn as_graphics_device(&self) -> Option<&dyn graphics::GraphicsDevice> {
        None
    }
    
    /// Cast to NetworkDevice if this device is a network device
    fn as_network_device(&self) -> Option<&dyn network::NetworkDevice> {
        None
    }
}

pub struct GenericDevice {
    device_type: DeviceType,
    name: &'static str,
}

impl GenericDevice {
    pub fn new(name: &'static str) -> Self {
        Self { device_type: DeviceType::Generic, name }
    }
}

impl Device for GenericDevice {
    fn device_type(&self) -> DeviceType {
        self.device_type
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
    
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl ControlOps for GenericDevice {
    // Generic devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for GenericDevice {
    fn get_mapping_info(&self, _offset: usize, _length: usize) 
                       -> Result<(usize, usize, bool), &'static str> {
        Err("Memory mapping not supported by this generic device")
    }
    
    fn on_mapped(&self, _vaddr: usize, _paddr: usize, _length: usize, _offset: usize) {
        // Generic devices don't support memory mapping
    }
    
    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Generic devices don't support memory mapping
    }
    
    fn supports_mmap(&self) -> bool {
        false
    }
}