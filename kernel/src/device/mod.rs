//! Device module.
//!
//! This module provides a framework for managing devices in the kernel.
//! It includes device information and driver management,
//! as well as platform-specific device handling.

pub mod audio;
pub mod block;
pub mod char;
pub mod clk;
pub mod events;
pub mod fdt;
pub mod gpio;
pub mod gpu;
pub mod graphics;
pub mod i2c;
pub mod input;
pub mod manager;
pub mod network;
pub mod pci;
pub mod platform;
pub mod power;
pub mod spi;
pub mod usb;

extern crate alloc;
use core::any::Any;

use crate::device::events::EventCapableDevice;
use crate::object::capability::selectable::Selectable;
use crate::object::capability::{ControlOps, MemoryMappingOps};
use alloc::vec::Vec;

/// Device capability flags for neutral feature discovery across ABIs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceCapability {
    /// Device behaves like a terminal/TTY (byte stream with line discipline hooks)
    Tty,
    /// Device provides raw serial I/O (low-level byte stream, no line discipline)
    Serial,
    /// Device provides native Scarlet PCM audio.
    Audio,
}

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
///
/// All device drivers must be Send + Sync to be stored in global DeviceManager.
pub trait DeviceDriver: Send + Sync {
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
pub trait Device: Send + Sync + ControlOps + MemoryMappingOps + Selectable {
    /// Called when a device file object is opened.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the open may proceed.
    fn open(&self) -> Result<(), &'static str> {
        Ok(())
    }

    /// Called when a device file object is closed.
    ///
    /// # Behavior
    ///
    /// Implementations may release per-open resources. The default implementation
    /// does nothing.
    fn close(&self) {}

    fn device_type(&self) -> DeviceType;
    fn name(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Optional capabilities exposed by this device (default: none)
    fn capabilities(&self) -> &'static [DeviceCapability] {
        &[]
    }

    /// Cast to EventCapableDevice if this device can emit events
    fn as_event_capable(&self) -> Option<&dyn EventCapableDevice> {
        None
    }

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

    /// Cast to GpuDevice if this device provides GPU acceleration.
    fn as_gpu_device(&self) -> Option<&dyn gpu::GpuDevice> {
        None
    }

    /// Cast to NetworkDevice if this device is a network device
    fn as_network_device(&self) -> Option<&dyn network::NetworkDevice> {
        None
    }

    /// Cast Arc<Self> to Arc<dyn BlockDevice> if this device is a block device
    /// This allows direct ownership of the block device for efficient I/O operations
    fn into_block_device(
        self: alloc::sync::Arc<Self>,
    ) -> Option<alloc::sync::Arc<dyn block::BlockDevice>> {
        None
    }

    /// Cast Arc<Self> to Arc<dyn CharDevice> if this device is a character device
    /// This allows direct ownership of the char device for efficient I/O operations
    fn into_char_device(
        self: alloc::sync::Arc<Self>,
    ) -> Option<alloc::sync::Arc<dyn char::CharDevice>> {
        None
    }

    /// Cast Arc<Self> to Arc<dyn GraphicsDevice> if this device is a graphics device
    /// This allows direct ownership of the graphics device for efficient operations
    fn into_graphics_device(
        self: alloc::sync::Arc<Self>,
    ) -> Option<alloc::sync::Arc<dyn graphics::GraphicsDevice>> {
        None
    }

    /// Cast Arc<Self> to Arc<dyn GpuDevice> if this device provides GPU acceleration.
    /// This allows direct ownership of the GPU device for efficient operations.
    fn into_gpu_device(
        self: alloc::sync::Arc<Self>,
    ) -> Option<alloc::sync::Arc<dyn gpu::GpuDevice>> {
        None
    }

    /// Cast Arc<Self> to Arc<dyn NetworkDevice> if this device is a network device
    /// This allows direct ownership of the network device for efficient operations
    fn into_network_device(
        self: alloc::sync::Arc<Self>,
    ) -> Option<alloc::sync::Arc<dyn network::NetworkDevice>> {
        None
    }
}

pub struct GenericDevice {
    device_type: DeviceType,
    name: &'static str,
}

impl GenericDevice {
    pub fn new(name: &'static str) -> Self {
        Self {
            device_type: DeviceType::Generic,
            name,
        }
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

impl Selectable for GenericDevice {
    fn wait_until_ready(
        &self,
        _interest: crate::object::capability::selectable::ReadyInterest,
        _trapframe: &mut crate::arch::Trapframe,
        _timeout_ticks: Option<u64>,
        _min_wait_ticks: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        crate::object::capability::selectable::SelectWaitOutcome::Ready
    }
}

impl ControlOps for GenericDevice {
    // Generic devices don't support control operations by default
    fn control(&self, _command: u32, _arg: usize) -> Result<i32, &'static str> {
        Err("Control operations not supported")
    }
}

impl MemoryMappingOps for GenericDevice {
    fn get_mapping_info(
        &self,
        _offset: usize,
        _length: usize,
    ) -> Result<(usize, usize, bool), &'static str> {
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
