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
pub mod mmc;
pub mod network;
pub mod nvmem;
pub mod pci;
pub mod phy;
pub mod pinctrl;
pub mod platform;
pub mod power;
pub mod remoteproc;
pub mod reset;
pub mod sensor;
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
    /// Device provides a Scarlet sensor event stream and metadata ABI.
    Sensor,
}

/// Discovery information supplied to a device driver during matching and probing.
pub trait DeviceInfo {
    /// Return the discovered device's name.
    ///
    /// # Arguments
    /// * `self` - Discovery descriptor to inspect.
    ///
    /// # Returns
    /// The descriptor's static name.
    fn name(&self) -> &'static str;
    /// Return the identifier supplied by this discovery descriptor.
    ///
    /// # Arguments
    /// * `self` - Discovery descriptor to inspect.
    ///
    /// # Returns
    /// A descriptor-specific identifier, not necessarily a registered-device ID.
    fn id(&self) -> usize;
    /// Return compatible strings used for driver matching.
    ///
    /// # Arguments
    /// * `self` - Discovery descriptor to inspect.
    ///
    /// # Returns
    /// An owned list of static compatible strings.
    fn compatible(&self) -> Vec<&'static str>;
    /// Borrow the concrete descriptor for downcasting.
    ///
    /// # Arguments
    /// * `self` - Discovery descriptor to inspect.
    ///
    /// # Returns
    /// A type-erased borrow with the same lifetime as `self`.
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
    /// Return the driver's diagnostic name.
    ///
    /// # Arguments
    /// * `self` - Driver to inspect.
    ///
    /// # Returns
    /// The driver's static name.
    fn name(&self) -> &'static str;
    /// Return the compatible strings recognized by this driver.
    ///
    /// # Arguments
    /// * `self` - Driver whose match table is requested.
    ///
    /// # Returns
    /// An owned list used to match discovery descriptors.
    fn match_table(&self) -> Vec<&'static str>;
    /// Probe and initialize a discovered device.
    ///
    /// # Arguments
    /// * `device` - Discovery information for the device to probe.
    ///
    /// # Returns
    /// `Ok(())` on successful initialization, or a driver-provided error. The
    /// driver is responsible for cleaning up any partially initialized resources.
    fn probe(&self, device: &dyn DeviceInfo) -> Result<(), &'static str>;
    /// Tear down a device previously handled by this driver.
    ///
    /// # Arguments
    /// * `device` - Discovery information identifying the device to remove.
    ///
    /// # Returns
    /// `Ok(())` on successful removal, or a driver-provided error.
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
/// All devices must implement the `ControlOps`, `MemoryMappingOps`, and
/// `Selectable` interfaces. Implementing an interface does not imply every
/// operation is supported; individual operations may report unsupported behavior.
///
pub trait Device: Send + Sync + ControlOps + MemoryMappingOps + Selectable {
    /// Called when a device file object is opened.
    ///
    /// # Arguments
    ///
    /// * `self` - Owned reference to the device backing the new endpoint.
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
    /// # Arguments
    ///
    /// * `self` - Device notified that an open endpoint is being released.
    ///
    /// # Returns
    ///
    /// No value. This hook does not itself unregister the device.
    ///
    /// # Behavior
    ///
    /// Implementations may release per-open resources. The default implementation
    /// does nothing.
    fn close(&self) {}

    /// Return the device's broad category.
    ///
    /// # Arguments
    /// * `self` - Device to inspect.
    ///
    /// # Returns
    /// The category advertised by this implementation.
    fn device_type(&self) -> DeviceType;
    /// Return the device's diagnostic name.
    ///
    /// # Arguments
    /// * `self` - Device to inspect.
    ///
    /// # Returns
    /// A static name; use the manager's ID when a registration identity is needed.
    fn name(&self) -> &'static str;
    /// Borrow the concrete device for downcasting.
    ///
    /// # Arguments
    /// * `self` - Device to inspect.
    ///
    /// # Returns
    /// A type-erased shared borrow tied to `self`.
    fn as_any(&self) -> &dyn Any;
    /// Mutably borrow the concrete device for downcasting.
    ///
    /// # Arguments
    /// * `self` - Exclusively borrowed device.
    ///
    /// # Returns
    /// A type-erased exclusive borrow tied to `self`.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Optional capabilities exposed by this device (default: none)
    ///
    /// # Arguments
    /// * `self` - Device to inspect.
    ///
    /// # Returns
    /// Static discovery flags; the default implementation returns an empty slice.
    fn capabilities(&self) -> &'static [DeviceCapability] {
        &[]
    }

    /// Cast to EventCapableDevice if this device can emit events
    ///
    /// # Arguments
    /// * `self` - Device to borrow through an event interface.
    ///
    /// # Returns
    /// A borrowed event interface, or `None` by default.
    fn as_event_capable(&self) -> Option<&dyn EventCapableDevice> {
        None
    }

    /// Cast to CharDevice if this device is a character device
    ///
    /// # Arguments
    /// * `self` - Device to borrow through a character interface.
    ///
    /// # Returns
    /// A borrowed character interface, or `None` by default.
    fn as_char_device(&self) -> Option<&dyn char::CharDevice> {
        None
    }

    /// Cast to BlockDevice if this device is a block device
    ///
    /// # Arguments
    /// * `self` - Device to borrow through a block interface.
    ///
    /// # Returns
    /// A borrowed block interface, or `None` by default.
    fn as_block_device(&self) -> Option<&dyn block::BlockDevice> {
        None
    }

    /// Cast to GraphicsDevice if this device is a graphics device
    ///
    /// # Arguments
    /// * `self` - Device to borrow through a graphics interface.
    ///
    /// # Returns
    /// A borrowed graphics interface, or `None` by default.
    fn as_graphics_device(&self) -> Option<&dyn graphics::GraphicsDevice> {
        None
    }

    /// Cast to NetworkDevice if this device is a network device
    ///
    /// # Arguments
    /// * `self` - Device to borrow through a network interface.
    ///
    /// # Returns
    /// A borrowed network interface, or `None` by default.
    fn as_network_device(&self) -> Option<&dyn network::NetworkDevice> {
        None
    }

    /// Cast `Arc<Self>` to `Arc<dyn BlockDevice>` if this device is a block device
    /// This allows direct ownership of the block device for efficient I/O operations
    ///
    /// # Arguments
    /// * `self` - Owned reference consumed by the conversion, including on failure.
    ///
    /// # Returns
    /// An owned block interface, or `None` by default, releasing the supplied reference.
    fn into_block_device(
        self: alloc::sync::Arc<Self>,
    ) -> Option<alloc::sync::Arc<dyn block::BlockDevice>> {
        None
    }

    /// Cast `Arc<Self>` to `Arc<dyn CharDevice>` if this device is a character device
    /// This allows direct ownership of the char device for efficient I/O operations
    ///
    /// # Arguments
    /// * `self` - Owned reference consumed by the conversion, including on failure.
    ///
    /// # Returns
    /// An owned character interface, or `None` by default, releasing the supplied reference.
    fn into_char_device(
        self: alloc::sync::Arc<Self>,
    ) -> Option<alloc::sync::Arc<dyn char::CharDevice>> {
        None
    }

    /// Cast `Arc<Self>` to `Arc<dyn GraphicsDevice>` if this device is a graphics device
    /// This allows direct ownership of the graphics device for efficient operations
    ///
    /// # Arguments
    /// * `self` - Owned reference consumed by the conversion, including on failure.
    ///
    /// # Returns
    /// An owned graphics interface, or `None` by default, releasing the supplied reference.
    fn into_graphics_device(
        self: alloc::sync::Arc<Self>,
    ) -> Option<alloc::sync::Arc<dyn graphics::GraphicsDevice>> {
        None
    }

    /// Cast `Arc<Self>` to `Arc<dyn NetworkDevice>` if this device is a network device
    /// This allows direct ownership of the network device for efficient operations
    ///
    /// # Arguments
    /// * `self` - Owned reference consumed by the conversion, including on failure.
    ///
    /// # Returns
    /// An owned network interface, or `None` by default, releasing the supplied reference.
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

    fn on_mapped(&self, vaddr: usize, paddr: u64, length: usize, offset: usize) {
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

    fn on_mapped(&self, _vaddr: usize, _paddr: u64, _length: usize, _offset: usize) {
        // Generic devices don't support memory mapping
    }

    fn on_unmapped(&self, _vaddr: usize, _length: usize) {
        // Generic devices don't support memory mapping
    }

    fn supports_mmap(&self) -> bool {
        false
    }
}
