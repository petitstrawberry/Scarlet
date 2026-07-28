//! Device module.
//!
//! This module provides a framework for managing devices in the kernel.
//! It includes device information and driver management,
//! as well as platform-specific device handling.

pub mod audio;
pub mod block;
pub mod char;
pub mod clk;
pub mod cpufreq;
pub mod dma;
pub mod events;
pub mod fdt;
pub mod gpio;
pub mod gpu;
pub mod graphics;
pub mod i2c;
pub mod input;
pub mod iommu;
pub mod mailbox;
pub mod manager;
pub mod network;
pub mod nvmem;
pub mod pci;
pub mod phy;
pub mod platform;
pub mod power;
pub mod remoteproc;
pub mod reset;
pub mod spi;
pub mod usb;
pub mod video;
pub mod watchdog;

extern crate alloc;
use alloc::{sync::Arc, vec::Vec};
use core::any::Any;

use crate::device::events::EventCapableDevice;
use crate::object::capability::memory_mapping::{ResolveFaultError, ResolveFaultResult};
use crate::object::capability::selectable::Selectable;
use crate::object::capability::{ControlOps, MemoryMappingInfo, MemoryMappingOps};

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
    /// Per-open device endpoint. The default endpoint delegates operations to
    /// the registered device and calls [`Device::close`] when the endpoint is
    /// dropped.
    fn open(self: Arc<Self>) -> Result<Arc<dyn Device>, &'static str>
    where
        Self: 'static,
    {
        Ok(Arc::new(DefaultDeviceOpen::new(self)))
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

    /// Cast Arc<Self> to Arc<dyn NetworkDevice> if this device is a network device
    /// This allows direct ownership of the network device for efficient operations
    fn into_network_device(
        self: alloc::sync::Arc<Self>,
    ) -> Option<alloc::sync::Arc<dyn network::NetworkDevice>> {
        None
    }
}

/// Default per-open endpoint for devices that do not need private open state.
///
/// The endpoint owns a reference to the registered device and delegates all
/// device operations to it. Dropping the endpoint invokes [`Device::close`] on
/// the registered device, preserving the old open/close lifecycle for existing
/// device implementations.
pub(crate) struct DefaultDeviceOpen<T: Device + ?Sized> {
    device: Arc<T>,
}

impl<T: Device + ?Sized> DefaultDeviceOpen<T> {
    /// Create a default per-open endpoint.
    ///
    /// # Arguments
    ///
    /// * `device` - Registered device backing this open endpoint.
    ///
    /// # Returns
    ///
    /// A delegating per-open endpoint.
    pub(crate) fn new(device: Arc<T>) -> Self {
        Self { device }
    }
}

impl<T: Device + ?Sized> Drop for DefaultDeviceOpen<T> {
    fn drop(&mut self) {
        self.device.close();
    }
}

impl<T: Device + ?Sized> ControlOps for DefaultDeviceOpen<T> {
    fn control(&self, command: u32, arg: usize) -> Result<i32, &'static str> {
        self.device.control(command, arg)
    }

    fn supported_control_commands(&self) -> Vec<(u32, &'static str)> {
        self.device.supported_control_commands()
    }
}

impl<T: Device + ?Sized> MemoryMappingOps for DefaultDeviceOpen<T> {
    fn get_mapping_info(
        &self,
        offset: usize,
        length: usize,
    ) -> Result<MemoryMappingInfo, &'static str> {
        self.device.get_mapping_info(offset, length)
    }

    fn get_mapping_info_with(
        &self,
        offset: usize,
        length: usize,
        is_shared: bool,
    ) -> Result<MemoryMappingInfo, &'static str> {
        self.device.get_mapping_info_with(offset, length, is_shared)
    }

    fn on_mapped(&self, vaddr: usize, paddr: usize, length: usize, offset: usize) {
        self.device.on_mapped(vaddr, paddr, length, offset);
    }

    fn on_unmapped(&self, vaddr: usize, length: usize) {
        self.device.on_unmapped(vaddr, length);
    }

    fn supports_mmap(&self) -> bool {
        self.device.supports_mmap()
    }

    fn mmap_owner_name(&self) -> alloc::string::String {
        self.device.mmap_owner_name()
    }

    fn can_extend_vma_on_fault(&self) -> bool {
        self.device.can_extend_vma_on_fault()
    }

    fn resolve_fault(
        &self,
        access: &crate::object::capability::memory_mapping::AccessKind,
        page_idx: usize,
        vm_start: usize,
    ) -> core::result::Result<ResolveFaultResult, ResolveFaultError> {
        self.device.resolve_fault(access, page_idx, vm_start)
    }

    fn fault_page_permissions(
        &self,
        access: &crate::object::capability::memory_mapping::AccessKind,
        default_permissions: usize,
    ) -> usize {
        self.device
            .fault_page_permissions(access, default_permissions)
    }

    fn private_fault_requires_copy(
        &self,
        access: &crate::object::capability::memory_mapping::AccessKind,
    ) -> bool {
        self.device.private_fault_requires_copy(access)
    }

    fn release_pages(&self, start_page_idx: usize, page_count: usize) {
        self.device.release_pages(start_page_idx, page_count);
    }

    fn fork_clone(&self) -> Option<Arc<dyn MemoryMappingOps>> {
        self.device.fork_clone()
    }
}

impl<T: Device + ?Sized> Selectable for DefaultDeviceOpen<T> {
    fn current_ready(
        &self,
        interest: crate::object::capability::selectable::ReadyInterest,
    ) -> crate::object::capability::selectable::ReadySet {
        self.device.current_ready(interest)
    }

    fn wait_until_ready(
        &self,
        interest: crate::object::capability::selectable::ReadyInterest,
        trapframe: &mut crate::arch::Trapframe,
        timeout_ticks: Option<u64>,
        min_wait_ticks: u64,
    ) -> crate::object::capability::selectable::SelectWaitOutcome {
        self.device
            .wait_until_ready(interest, trapframe, timeout_ticks, min_wait_ticks)
    }

    fn set_nonblocking(&self, enabled: bool) {
        self.device.set_nonblocking(enabled);
    }

    fn is_nonblocking(&self) -> bool {
        self.device.is_nonblocking()
    }
}

impl<T: Device + ?Sized + 'static> Device for DefaultDeviceOpen<T> {
    fn open(self: Arc<Self>) -> Result<Arc<dyn Device>, &'static str> {
        Ok(self)
    }

    fn device_type(&self) -> DeviceType {
        self.device.device_type()
    }

    fn name(&self) -> &'static str {
        self.device.name()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn capabilities(&self) -> &'static [DeviceCapability] {
        self.device.capabilities()
    }

    fn as_event_capable(&self) -> Option<&dyn EventCapableDevice> {
        self.device.as_event_capable()
    }

    fn as_char_device(&self) -> Option<&dyn char::CharDevice> {
        self.device.as_char_device()
    }

    fn as_block_device(&self) -> Option<&dyn block::BlockDevice> {
        self.device.as_block_device()
    }

    fn as_graphics_device(&self) -> Option<&dyn graphics::GraphicsDevice> {
        self.device.as_graphics_device()
    }

    fn as_network_device(&self) -> Option<&dyn network::NetworkDevice> {
        self.device.as_network_device()
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
    ) -> Result<crate::object::capability::MemoryMappingInfo, &'static str> {
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
