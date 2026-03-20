//! # Device Manager Module
//!
//! This module provides functionality for managing hardware devices in the kernel.
//!
//! ## Overview
//!
//! The device manager is responsible for:
//! - Tracking available device drivers with priority-based initialization
//! - Device discovery and initialization through FDT
//! - Managing device information and lifecycle
//!
//! ## Key Components
//!
//! - `DeviceManager`: The main device management system that handles all devices and drivers
//! - `DriverPriority`: Priority levels for controlling driver initialization order
//!
//! ## Device Discovery
//!
//! Devices are discovered through the Flattened Device Tree (FDT). The manager:
//! 1. Parses the device tree
//! 2. Matches compatible devices with registered drivers based on priority
//! 3. Probes devices with appropriate drivers in priority order
//!
//! ## Usage
//!
//! The device manager is implemented as a global singleton that can be accessed via:
//! - `DeviceManager::get_manager()` - Shared access (thread-safe via internal Mutex)
//!
//! ### Example: Registering a device driver
//!
//! ```
//! use crate::device::manager::{DeviceManager, DriverPriority};
//!
//! // Create a new device driver
//! let my_driver = Box::new(MyDeviceDriver::new());
//!
//! // Register with the device manager at Core priority
//! DeviceManager::get_manager().register_driver(my_driver, DriverPriority::Core);
//! ```

extern crate alloc;

use core::sync::atomic::AtomicUsize;
use core::sync::atomic::Ordering;

use alloc::boxed::Box;
use alloc::collections::btree_map::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::mutex::Mutex;

use crate::device::platform::PlatformDeviceInfo;
use crate::device::platform::PlatformDeviceProperty;
use crate::device::platform::resource::PlatformDeviceResource;
use crate::device::platform::resource::PlatformDeviceResourceType;
use crate::early_println;

use super::Device;
use super::DeviceDriver;
use super::DeviceInfo;
use crate::DeviceSource;

/// Simplified shared device type
pub type SharedDevice = Arc<dyn Device>;

/// Driver priority levels for initialization order
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DriverPriority {
    /// Critical infrastructure drivers (interrupt controllers, memory controllers)
    Critical = 0,
    /// Core system drivers (timers, basic I/O)
    Core = 1,
    /// Standard device drivers (network, storage)
    Standard = 2,
    /// Late initialization drivers (filesystems, user interface)
    Late = 3,
}

impl DriverPriority {
    /// Get all priority levels in order
    pub fn all() -> &'static [DriverPriority] {
        &[
            DriverPriority::Critical,
            DriverPriority::Core,
            DriverPriority::Standard,
            DriverPriority::Late,
        ]
    }

    /// Get a human-readable description of the priority level
    pub fn description(&self) -> &'static str {
        match self {
            DriverPriority::Critical => "Critical Infrastructure",
            DriverPriority::Core => "Core System",
            DriverPriority::Standard => "Standard Devices",
            DriverPriority::Late => "Late Initialization",
        }
    }
}

static MANAGER: DeviceManager = DeviceManager::new();

/// DeviceManager
///
/// This struct is the main device management system.
/// It handles all devices and drivers with priority-based initialization.
///
/// # Fields
/// - `devices`: A mutex-protected map of all registered devices by ID.
/// - `device_by_name`: A mutex-protected map of devices by name.
/// - `name_to_id`: A mutex-protected map from device name to device ID.
/// - `drivers`: A mutex-protected map of device drivers organized by priority.
/// - `next_device_id`: Atomic counter for generating unique device IDs.
pub struct DeviceManager {
    /* Devices stored by ID */
    devices: Mutex<BTreeMap<usize, SharedDevice>>,
    /* Devices stored by name */
    device_by_name: Mutex<BTreeMap<String, SharedDevice>>,
    /* Name to ID mapping */
    name_to_id: Mutex<BTreeMap<String, usize>>,
    /* Device drivers organized by priority */
    drivers: Mutex<BTreeMap<DriverPriority, Vec<Box<dyn DeviceDriver>>>>,
    /* Next device ID to assign */
    next_device_id: AtomicUsize,
}

impl DeviceManager {
    const fn new() -> Self {
        DeviceManager {
            devices: Mutex::new(BTreeMap::new()),
            device_by_name: Mutex::new(BTreeMap::new()),
            name_to_id: Mutex::new(BTreeMap::new()),
            drivers: Mutex::new(BTreeMap::new()),
            next_device_id: AtomicUsize::new(1), // Start from 1, reserve 0 for invalid
        }
    }

    #[cfg(test)]
    pub const fn new_for_test() -> Self {
        Self::new()
    }

    pub fn get_manager() -> &'static DeviceManager {
        &MANAGER
    }

    fn read_be_u32(bytes: &[u8]) -> Option<u32> {
        if bytes.len() < 4 {
            return None;
        }
        Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn get_u32_prop<'a, 'b>(node: &fdt::node::FdtNode<'a, 'b>, name: &str) -> Option<u32> {
        let prop = node.property(name)?;
        Self::read_be_u32(prop.value)
    }

    fn find_node_by_phandle<'a>(
        fdt: &'a fdt::Fdt<'a>,
        phandle: u32,
    ) -> Option<fdt::node::FdtNode<'a, 'a>> {
        let mut stack: Vec<fdt::node::FdtNode<'a, 'a>> = Vec::new();
        stack.push(fdt.find_node("/")?);

        while let Some(node) = stack.pop() {
            if let Some(p) = Self::get_u32_prop(&node, "phandle") {
                if p == phandle {
                    return Some(node);
                }
            }
            if let Some(p) = Self::get_u32_prop(&node, "linux,phandle") {
                if p == phandle {
                    return Some(node);
                }
            }
            for child in node.children() {
                stack.push(child);
            }
        }

        None
    }

    fn push_irq_resource(
        resources: &mut Vec<PlatformDeviceResource>,
        irq_num: usize,
        metadata: Option<crate::device::platform::resource::IrqMetadata>,
    ) {
        resources.push(PlatformDeviceResource {
            res_type: PlatformDeviceResourceType::IRQ,
            start: irq_num,
            end: irq_num,
            irq_metadata: metadata,
        });
    }

    fn mem_resource_from_region(
        region: fdt::standard_nodes::MemoryRegion,
    ) -> Option<PlatformDeviceResource> {
        let start = region.starting_address as usize;
        let size = region.size?;

        if size == 0 {
            return None;
        }

        let end = start.checked_add(size - 1)?;

        Some(PlatformDeviceResource {
            res_type: PlatformDeviceResourceType::MEM,
            start,
            end,
            irq_metadata: None,
        })
    }

    /// Register a device with the manager
    ///
    /// # Arguments
    /// * `device`: The device to register.
    ///
    /// # Returns
    ///  * The id of the registered device.
    ///
    /// # Example
    ///
    /// ```rust
    /// let device = Arc::new(MyDevice::new());
    /// let id = DeviceManager::get_manager().register_device(device);
    /// ```
    ///
    pub fn register_device(&self, device: Arc<dyn Device>) -> usize {
        let mut devices = self.devices.lock();
        let id = self.next_device_id.fetch_add(1, Ordering::SeqCst);
        devices.insert(id, device);
        id
    }

    /// Register a device with the manager by name
    ///
    /// # Arguments
    /// * `name`: The name of the device.
    /// * `device`: The device to register.
    ///
    /// # Returns
    ///  * The id of the registered device.
    ///
    pub fn register_device_with_name(&self, name: String, device: Arc<dyn Device>) -> usize {
        let mut devices = self.devices.lock();
        let mut device_by_name = self.device_by_name.lock();
        let mut name_to_id = self.name_to_id.lock();

        let id = self.next_device_id.fetch_add(1, Ordering::SeqCst);
        devices.insert(id, device.clone());
        device_by_name.insert(name.clone(), device);
        name_to_id.insert(name, id);
        id
    }

    /// Get a device by ID
    ///
    /// # Arguments
    /// * `id`: The id of the device to get.
    ///
    /// # Returns
    /// * The device if found, or None if not found.
    ///
    pub fn get_device(&self, id: usize) -> Option<SharedDevice> {
        let devices = self.devices.lock();
        devices.get(&id).cloned()
    }

    /// Get a device by name
    ///
    /// # Arguments
    /// * `name`: The name of the device to get.
    ///
    /// # Returns
    /// * The device if found, or None if not found.
    ///
    pub fn get_device_by_name(&self, name: &str) -> Option<SharedDevice> {
        let device_by_name = self.device_by_name.lock();
        device_by_name.get(name).cloned()
    }

    /// Get a device ID by name
    ///
    /// # Arguments
    /// * `name`: The name of the device to find.
    ///
    /// # Returns
    /// * The device ID if found, or None if not found.
    ///
    pub fn get_device_id_by_name(&self, name: &str) -> Option<usize> {
        let name_to_id = self.name_to_id.lock();
        name_to_id.get(name).cloned()
    }

    /// Get the number of devices
    ///
    /// # Returns
    ///
    /// The number of devices.
    ///
    pub fn get_devices_count(&self) -> usize {
        let devices = self.devices.lock();
        devices.len()
    }

    /// Get the first device of a specific type
    ///
    /// # Arguments
    /// * `device_type`: The device type to find.
    ///
    /// # Returns
    /// * The first device ID of the specified type, or None if not found.
    ///
    pub fn get_first_device_by_type(&self, device_type: super::DeviceType) -> Option<usize> {
        let devices = self.devices.lock();
        for (id, device) in devices.iter() {
            if device.device_type() == device_type {
                return Some(*id);
            }
        }
        None
    }

    /// Get all devices registered by name
    ///
    /// Returns an iterator over (name, device) pairs for all devices
    /// that were registered with explicit names.
    ///
    /// # Returns
    ///
    /// Vector of (name, device) tuples
    pub fn get_named_devices(&self) -> Vec<(String, SharedDevice)> {
        let device_by_name = self.device_by_name.lock();
        device_by_name
            .iter()
            .map(|(name, device)| (name.clone(), device.clone()))
            .collect()
    }

    pub fn borrow_drivers(&self) -> &Mutex<BTreeMap<DriverPriority, Vec<Box<dyn DeviceDriver>>>> {
        &self.drivers
    }

    /// Populates devices from the FDT (Flattened Device Tree).
    ///
    /// This function searches for the `/soc` node in the FDT and iterates through its children.
    /// For each child node, it checks if there is a compatible driver registered.
    /// If a matching driver is found, it probes the device using the driver's `probe` method.
    /// If the probe is successful, the device is registered with the driver.
    ///
    /// # Deprecated
    /// Use `populate_devices_from_source` with `DeviceSource::Fdt` instead.
    pub fn populate_devices(&self) {
        use super::fdt::FdtManager;

        let fdt_manager = unsafe { FdtManager::get_mut_manager() };
        let fdt = fdt_manager.get_fdt();
        if fdt.is_none() {
            early_println!("FDT not initialized");
            return;
        }

        self.populate_devices_from_fdt(None);
    }

    /// Populate devices using a specific device source
    ///
    /// # Arguments
    ///
    /// * `device_source` - The source of device information (FDT, UEFI, ACPI, etc.)
    /// * `priorities` - Optional slice of priority levels to use. If None, uses all priorities in order.
    pub fn populate_devices_from_source(
        &self,
        device_source: &DeviceSource,
        priorities: Option<&[DriverPriority]>,
    ) {
        match device_source {
            DeviceSource::Fdt(_addr) => {
                early_println!("Populating devices from FDT...");
                self.populate_devices_from_fdt(priorities);
            }
            DeviceSource::Uefi => {
                early_println!("Populating devices from UEFI...");
                self.populate_devices_from_uefi(priorities);
            }
            DeviceSource::Acpi => {
                early_println!("Populating devices from ACPI...");
                self.populate_devices_from_acpi(priorities);
            }
            DeviceSource::None => {
                early_println!("No device source available - skipping device population");
            }
        }
    }

    /// Populate devices from FDT
    fn populate_devices_from_fdt(&self, priorities: Option<&[DriverPriority]>) {
        use super::fdt::FdtManager;

        let fdt_manager = unsafe { FdtManager::get_mut_manager() };
        let fdt = fdt_manager.get_fdt();
        if fdt.is_none() {
            early_println!("FDT not initialized");
            return;
        }
        let fdt = fdt.unwrap();

        let priority_list = priorities.unwrap_or(DriverPriority::all());

        // Process each priority level separately to reduce stack depth
        for &priority in priority_list {
            self.process_priority_level(fdt, priority);
        }
    }

    /// Process devices for a single priority level - reduces stack nesting
    fn process_priority_level(&self, fdt: &fdt::Fdt, priority: DriverPriority) {
        early_println!(
            "Populating devices with {} drivers from FDT...",
            priority.description()
        );

        // Try /soc node first (RISC-V virt), then fall back to root node (AArch64 virt)
        let parent_node = if let Some(soc) = fdt.find_node("/soc") {
            Some(soc)
        } else {
            // For AArch64 virt and other platforms where devices are at root level
            fdt.find_node("/")
        };

        let parent_node = match parent_node {
            Some(node) => node,
            None => {
                early_println!("No device tree root found");
                return;
            }
        };

        let mut idx = 0;

        for child in parent_node.children() {
            self.process_device_subtree(&child, priority, &mut idx);
        }

        if let Some(chosen_node) = fdt.find_node("/chosen") {
            for child in chosen_node.children() {
                self.process_device_subtree(&child, priority, &mut idx);
            }
        }
    }

    fn process_device_subtree(
        &self,
        node: &fdt::node::FdtNode,
        priority: DriverPriority,
        idx: &mut usize,
    ) {
        self.process_single_device_node(node, priority, idx);

        for child in node.children() {
            self.process_device_subtree(&child, priority, idx);
        }
    }

    /// Process a single device node with minimal stack usage
    fn process_single_device_node(
        &self,
        child: &fdt::node::FdtNode,
        priority: DriverPriority,
        idx: &mut usize,
    ) {
        let compatible = child.compatible();
        if compatible.is_none() {
            return;
        }

        // Minimize stack usage by not collecting all compatible strings at once
        let compatible_iter = compatible.unwrap().all();

        // Check if we have any drivers for this priority level
        let has_drivers = {
            let drivers = self.drivers.lock();
            drivers
                .get(&priority)
                .map_or(false, |list| !list.is_empty())
        };

        if !has_drivers {
            return;
        }

        // Build resources separately to reduce stack usage
        let resources = self.build_minimal_resources(&child);
        let properties = self.build_device_properties(&child);

        // Try to match with drivers
        let compatible_vec: alloc::vec::Vec<&str> = compatible_iter.collect();
        self.try_match_and_probe_device(
            child,
            priority,
            idx,
            compatible_vec,
            resources,
            properties,
        );
    }

    fn build_device_properties(
        &self,
        child: &fdt::node::FdtNode,
    ) -> alloc::vec::Vec<PlatformDeviceProperty> {
        child
            .properties()
            .map(|property| PlatformDeviceProperty::new(property.name, property.value))
            .collect()
    }

    /// Build device resources with minimal stack allocation
    fn build_minimal_resources(
        &self,
        child: &fdt::node::FdtNode,
    ) -> alloc::vec::Vec<PlatformDeviceResource> {
        let mut resources = alloc::vec::Vec::new();

        // Add memory regions
        if let Some(regions) = child.reg() {
            for region in regions {
                if let Some(res) = Self::mem_resource_from_region(region) {
                    resources.push(res);
                }
            }
        }

        // Add IRQs
        let mut parsed_any_irq = false;

        if let Some(irqs) = child.interrupts() {
            // Standard path: fdt-rs successfully parsed interrupts
            for irq in irqs {
                Self::push_irq_resource(&mut resources, irq, None);
                parsed_any_irq = true;
            }
        }

        if !parsed_any_irq {
            if let Some(prop) = child.property("interrupts-extended") {
                if let Some(fdt) = crate::device::fdt::FdtManager::get_manager().get_fdt() {
                    let bytes = prop.value;
                    let mut offset = 0usize;

                    while offset + 4 <= bytes.len() {
                        let phandle = match Self::read_be_u32(&bytes[offset..offset + 4]) {
                            Some(v) => v,
                            None => break,
                        };
                        offset += 4;

                        let intc_node = Self::find_node_by_phandle(fdt, phandle);
                        let interrupt_cells = intc_node
                            .as_ref()
                            .and_then(|n| Self::get_u32_prop(n, "#interrupt-cells"))
                            .unwrap_or(1) as usize;

                        if interrupt_cells == 0 {
                            break;
                        }

                        let needed = interrupt_cells.saturating_mul(4);
                        if offset + needed > bytes.len() {
                            break;
                        }

                        let cell0 = Self::read_be_u32(&bytes[offset..offset + 4]).unwrap_or(0);
                        let cell1 = if interrupt_cells >= 2 {
                            Self::read_be_u32(&bytes[offset + 4..offset + 8]).unwrap_or(0)
                        } else {
                            0
                        };
                        let cell2 = if interrupt_cells >= 3 {
                            Self::read_be_u32(&bytes[offset + 8..offset + 12]).unwrap_or(0)
                        } else {
                            0
                        };

                        let (irq_num, metadata) = match interrupt_cells {
                            3 => (
                                cell1 as usize,
                                Some(crate::device::platform::resource::IrqMetadata {
                                    irq_type: cell0,
                                    irq_number: cell1,
                                    irq_flags: cell2,
                                }),
                            ),
                            2 => (
                                cell0 as usize,
                                Some(crate::device::platform::resource::IrqMetadata {
                                    irq_type: 0,
                                    irq_number: cell0,
                                    irq_flags: cell1,
                                }),
                            ),
                            1 => (cell0 as usize, None),
                            _ => (cell0 as usize, None),
                        };

                        Self::push_irq_resource(&mut resources, irq_num, metadata);
                        parsed_any_irq = true;
                        offset += needed;
                    }
                }
            }
        }

        if !parsed_any_irq {
            if let Some(prop) = child.property("interrupts") {
                // Fallback: Parse raw interrupts property when fdt-rs fails
                // This preserves interrupt controller metadata for later translation
                let value = prop.value;

                // Detect cell format based on property length
                let cell_size = if value.len() % 12 == 0 {
                    3 // 3-cell format (e.g., ARM GIC: <type, number, flags>)
                } else if value.len() % 8 == 0 {
                    2 // 2-cell format
                } else if value.len() % 4 == 0 {
                    1 // 1-cell format (just interrupt number)
                } else {
                    return resources; // Unknown format, skip
                };

                let num_irqs = value.len() / (cell_size * 4);

                for i in 0..num_irqs {
                    let offset = i * cell_size * 4;

                    let (irq_num, metadata) = match cell_size {
                        3 => {
                            // 3-cell format: <type, number, flags>
                            let irq_type = u32::from_be_bytes([
                                value[offset],
                                value[offset + 1],
                                value[offset + 2],
                                value[offset + 3],
                            ]);
                            let irq_number = u32::from_be_bytes([
                                value[offset + 4],
                                value[offset + 5],
                                value[offset + 6],
                                value[offset + 7],
                            ]);
                            let irq_flags = u32::from_be_bytes([
                                value[offset + 8],
                                value[offset + 9],
                                value[offset + 10],
                                value[offset + 11],
                            ]);

                            // Store raw number, let interrupt controller translate
                            (
                                irq_number as usize,
                                Some(crate::device::platform::resource::IrqMetadata {
                                    irq_type,
                                    irq_number,
                                    irq_flags,
                                }),
                            )
                        }
                        2 => {
                            // 2-cell format: <number, flags>
                            let irq_number = u32::from_be_bytes([
                                value[offset],
                                value[offset + 1],
                                value[offset + 2],
                                value[offset + 3],
                            ]);
                            let irq_flags = u32::from_be_bytes([
                                value[offset + 4],
                                value[offset + 5],
                                value[offset + 6],
                                value[offset + 7],
                            ]);

                            (
                                irq_number as usize,
                                Some(crate::device::platform::resource::IrqMetadata {
                                    irq_type: 0, // No type in 2-cell format
                                    irq_number,
                                    irq_flags,
                                }),
                            )
                        }
                        1 => {
                            // 1-cell format: just interrupt number
                            let irq_number = u32::from_be_bytes([
                                value[offset],
                                value[offset + 1],
                                value[offset + 2],
                                value[offset + 3],
                            ]);

                            (irq_number as usize, None)
                        }
                        _ => unreachable!(),
                    };

                    Self::push_irq_resource(&mut resources, irq_num, metadata);
                    parsed_any_irq = true;
                }
            }
        }

        resources
    }

    /// Try to match device with drivers and probe if successful
    fn try_match_and_probe_device(
        &self,
        child: &fdt::node::FdtNode,
        priority: DriverPriority,
        idx: &mut usize,
        compatible: alloc::vec::Vec<&str>,
        resources: alloc::vec::Vec<PlatformDeviceResource>,
        properties: alloc::vec::Vec<PlatformDeviceProperty>,
    ) {
        let drivers = self.drivers.lock();
        if let Some(driver_list) = drivers.get(&priority) {
            for driver in driver_list.iter() {
                if driver
                    .match_table()
                    .iter()
                    .any(|&c| compatible.contains(&c))
                {
                    // Convert borrowed strings to static strings (FDT data is actually static)
                    // This is safe because FDT is loaded at boot and remains in memory
                    let static_name: &'static str = unsafe { core::mem::transmute(child.name) };
                    let static_compatible: alloc::vec::Vec<&'static str> = compatible
                        .into_iter()
                        .map(|s| unsafe { core::mem::transmute(s) })
                        .collect();

                    let device = alloc::boxed::Box::new(PlatformDeviceInfo::new(
                        static_name,
                        *idx,
                        static_compatible,
                        resources,
                        properties,
                    ));

                    match driver.probe(&*device) {
                        Ok(_) => {
                            early_println!(
                                "Successfully probed {} device: {}",
                                priority.description(),
                                device.name()
                            );
                            *idx += 1;
                        }
                        Err(e) => {
                            early_println!(
                                "Failed to probe {} device {}: {}",
                                priority.description(),
                                device.name(),
                                e
                            );
                        }
                    }
                    break; // Found matching driver, move to next device
                }
            }
        }
    }

    /// Populate devices from UEFI (stub implementation)
    ///
    /// # Arguments
    ///
    /// * `priorities` - Optional slice of priority levels to use. If None, uses all priorities in order.
    ///
    /// # Note
    ///
    /// This is currently a stub implementation. UEFI device discovery will be implemented
    /// when UEFI boot support is added.
    fn populate_devices_from_uefi(&self, _priorities: Option<&[DriverPriority]>) {
        early_println!("UEFI device discovery not yet implemented");
        // TODO: Implement UEFI device discovery
        // - Enumerate UEFI protocols
        // - Create PlatformDeviceInfo from UEFI device handles
        // - Probe devices with matching drivers
    }

    /// Populate devices from ACPI (stub implementation)
    ///
    /// # Arguments
    ///
    /// * `priorities` - Optional slice of priority levels to use. If None, uses all priorities in order.
    ///
    /// # Note
    ///
    /// This is currently a stub implementation. ACPI device discovery will be implemented
    /// when x86 support is added.
    fn populate_devices_from_acpi(&self, _priorities: Option<&[DriverPriority]>) {
        early_println!("ACPI device discovery not yet implemented");
        // TODO: Implement ACPI device discovery
        // - Parse ACPI tables (DSDT, etc.)
        // - Create PlatformDeviceInfo from ACPI device objects
        // - Probe devices with matching drivers
    }

    /// Populate devices using drivers of specific priority levels
    ///
    /// # Arguments
    ///
    /// * `priorities` - Optional slice of priority levels to use. If None, uses all priorities in order.
    ///
    /// # Deprecated
    /// Use `populate_devices_from_source` instead.
    pub fn populate_devices_by_priority(&self, priorities: Option<&[DriverPriority]>) {
        self.populate_devices_from_fdt(priorities);
    }

    /// Registers a device driver with the device manager.
    ///
    /// This function takes a boxed device driver and adds it to the list of registered drivers
    /// at the specified priority level.
    ///
    /// # Arguments
    ///
    /// * `driver` - A boxed device driver that implements the `DeviceDriver` trait.
    /// * `priority` - The priority level for this driver.
    ///
    /// # Example
    ///
    /// ```rust
    /// let driver = Box::new(MyDeviceDriver::new());
    /// DeviceManager::get_manager().register_driver(driver, DriverPriority::Standard);
    /// ```
    pub fn register_driver(&self, driver: Box<dyn DeviceDriver>, priority: DriverPriority) {
        let mut drivers = self.drivers.lock();
        drivers
            .entry(priority)
            .or_insert_with(Vec::new)
            .push(driver);
    }

    /// Registers a device driver with default Standard priority.
    ///
    /// This is a convenience method for backward compatibility.
    ///
    /// # Arguments
    ///
    /// * `driver` - A boxed device driver that implements the `DeviceDriver` trait.
    pub fn register_driver_default(&self, driver: Box<dyn DeviceDriver>) {
        self.register_driver(driver, DriverPriority::Standard);
    }

    /// Clear all devices and reset the manager state (for testing only)
    ///
    /// This method is only available in test builds and should only be used
    /// for unit testing to ensure test isolation.
    #[cfg(test)]
    pub fn clear_for_test(&self) {
        let mut devices = self.devices.lock();
        let mut device_by_name = self.device_by_name.lock();
        let mut name_to_id = self.name_to_id.lock();

        devices.clear();
        device_by_name.clear();
        name_to_id.clear();
        self.next_device_id.store(1, Ordering::SeqCst); // Start from 1, reserve 0 for invalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{GenericDevice, platform::*};
    use alloc::vec;

    #[cfg(target_arch = "riscv64")]
    #[test_case]
    fn test_populate_driver() {
        static mut TEST_RESULT: bool = false;
        fn probe_fn(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
            unsafe {
                TEST_RESULT = true;
            }
            Ok(())
        }

        let driver = Box::new(PlatformDeviceDriver::new(
            "test",
            probe_fn,
            |_device| Ok(()),
            vec!["sifive,test0"],
        ));
        let manager = DeviceManager::new();
        manager.register_driver(driver, DriverPriority::Standard);

        manager.populate_devices();
        let result = unsafe { TEST_RESULT };
        assert_eq!(result, true);
    }

    #[test_case]
    fn test_get_device_from_manager() {
        let device = Arc::new(GenericDevice::new("test"));
        let manager = DeviceManager::new();
        let id = manager.register_device(device);
        let retrieved_device = manager.get_device(id);
        assert!(retrieved_device.is_some());
        let retrieved_device = retrieved_device.unwrap();
        assert_eq!(retrieved_device.name(), "test");
    }

    #[test_case]
    fn test_get_device_by_name() {
        let device = Arc::new(GenericDevice::new("test_named"));
        let manager = DeviceManager::new();
        let _id = manager.register_device_with_name("test_device".into(), device);
        let retrieved_device = manager.get_device_by_name("test_device");
        assert!(retrieved_device.is_some());
        let retrieved_device = retrieved_device.unwrap();
        assert_eq!(retrieved_device.name(), "test_named");
    }

    #[test_case]
    fn test_get_first_device_by_type() {
        let device1 = Arc::new(GenericDevice::new("test_char"));
        let device2 = Arc::new(GenericDevice::new("test_block"));
        let manager = DeviceManager::new();
        let _id1 = manager.register_device(device1);
        let _id2 = manager.register_device(device2);

        let char_device_id = manager.get_first_device_by_type(crate::device::DeviceType::Generic);
        assert!(char_device_id.is_some());
        let char_device_id = char_device_id.unwrap();
        let char_device = manager.get_device(char_device_id).unwrap();
        assert_eq!(char_device.name(), "test_char");
    }

    #[test_case]
    fn test_get_device_out_of_bounds() {
        let manager = DeviceManager::new();
        let device = manager.get_device(999);
        assert!(device.is_none());
    }

    #[test_case]
    fn test_get_device_by_name_not_found() {
        let manager = DeviceManager::new();
        let device = manager.get_device_by_name("non_existent");
        assert!(device.is_none());
    }

    #[test_case]
    fn test_mem_resource_from_region_without_size() {
        let region = fdt::standard_nodes::MemoryRegion {
            starting_address: 0x1000 as *const u8,
            size: None,
        };

        assert!(DeviceManager::mem_resource_from_region(region).is_none());
    }

    #[test_case]
    fn test_mem_resource_from_region_with_size() {
        let region = fdt::standard_nodes::MemoryRegion {
            starting_address: 0x1000 as *const u8,
            size: Some(0x100),
        };

        let resource = DeviceManager::mem_resource_from_region(region).unwrap();
        assert_eq!(resource.start, 0x1000);
        assert_eq!(resource.end, 0x10ff);
        assert_eq!(resource.res_type, PlatformDeviceResourceType::MEM);
        assert!(resource.irq_metadata.is_none());
    }
}
