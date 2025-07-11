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
//! - `DeviceManager::get_manager()` - Immutable access
//! - `DeviceManager::get_mut_manager()` - Mutable access
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
//! DeviceManager::get_mut_manager().register_driver(my_driver, DriverPriority::Core);
//! ```

extern crate alloc;

use core::sync::atomic::AtomicUsize;
use core::sync::atomic::Ordering;

use alloc::collections::btree_map::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::string::String;
use spin::mutex::Mutex;

use crate::device::platform::resource::PlatformDeviceResource;
use crate::device::platform::resource::PlatformDeviceResourceType;
use crate::device::platform::PlatformDeviceInfo;
use crate::early_println;

use super::fdt::FdtManager;
use super::Device;
use super::DeviceDriver;
use super::DeviceInfo;

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

static mut MANAGER: DeviceManager = DeviceManager::new();

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

    #[allow(static_mut_refs)]
    pub fn get_manager() -> &'static DeviceManager {
        unsafe { &MANAGER }
    }

    #[allow(static_mut_refs)]
    pub fn get_mut_manager() -> &'static mut DeviceManager {
        unsafe { &mut MANAGER }
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
    /// let id = DeviceManager::get_mut_manager().register_device(device);
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
        device_by_name.iter().map(|(name, device)| (name.clone(), device.clone())).collect()
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
    pub fn populate_devices(&mut self) {
        // Use all priority levels in order
        self.populate_devices_by_priority(None);
    }

    /// Populate devices using drivers of specific priority levels
    /// 
    /// # Arguments
    /// 
    /// * `priorities` - Optional slice of priority levels to use. If None, uses all priorities in order.
    pub fn populate_devices_by_priority(&mut self, priorities: Option<&[DriverPriority]>) {
        let fdt_manager = unsafe { FdtManager::get_mut_manager() };
        let fdt = fdt_manager.get_fdt();
        if fdt.is_none() {
            early_println!("FDT not initialized");
            return;
        }
        let fdt = fdt.unwrap();
        
        let priority_list = priorities.unwrap_or(DriverPriority::all());
        
        for &priority in priority_list {
            early_println!("Populating devices with {} drivers from FDT...", priority.description());
            
            let soc = fdt.find_node("/soc");
            if soc.is_none() {
                early_println!("No /soc node found");
                continue;
            }

            let soc = soc.unwrap();
            let mut idx = 0;
            for child in soc.children() {
                let compatible = child.compatible();
                if compatible.is_none() {
                    continue;
                }
                let compatible = compatible.unwrap().all().collect::<Vec<_>>();
                
                // Get drivers for this priority level
                let drivers = self.drivers.lock();
                if let Some(driver_list) = drivers.get(&priority) {
                    for driver in driver_list.iter() {
                        if driver.match_table().iter().any(|&c| compatible.contains(&c)) {
                            let mut resources = Vec::new();
                            
                            // Memory regions
                            if let Some(regions) = child.reg() {
                                for region in regions {
                                    let res = PlatformDeviceResource {
                                        res_type: PlatformDeviceResourceType::MEM,
                                        start: region.starting_address as usize,
                                        end: region.starting_address as usize + region.size.unwrap() - 1,
                                    };
                                    resources.push(res);
                                }
                            }

                            // IRQs
                            if let Some(irqs) = child.interrupts() {
                                for irq in irqs {
                                    let res = PlatformDeviceResource {
                                        res_type: PlatformDeviceResourceType::IRQ,
                                        start: irq,
                                        end: irq,
                                    };
                                    resources.push(res);
                                }
                            }

                            let device: Box<dyn DeviceInfo> = Box::new(PlatformDeviceInfo::new(
                                child.name,
                                idx,
                                compatible.clone(),
                                resources,
                            ));
                            if let Err(e) = driver.probe(&*device) {
                                early_println!("Failed to probe {} device {}: {}", priority.description(), device.name(), e);
                            } else {
                                early_println!("Successfully probed {} device: {}", priority.description(), device.name());
                                idx += 1;
                            }
                            break; // Found matching driver, move to next device
                        }
                    }
                }
            }
        }
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
    /// DeviceManager::get_mut_manager().register_driver(driver, DriverPriority::Standard);
    /// ```
    pub fn register_driver(&mut self, driver: Box<dyn DeviceDriver>, priority: DriverPriority) {
        let mut drivers = self.drivers.lock();
        drivers.entry(priority).or_insert_with(Vec::new).push(driver);
    }

    /// Registers a device driver with default Standard priority.
    /// 
    /// This is a convenience method for backward compatibility.
    /// 
    /// # Arguments
    /// 
    /// * `driver` - A boxed device driver that implements the `DeviceDriver` trait.
    pub fn register_driver_default(&mut self, driver: Box<dyn DeviceDriver>) {
        self.register_driver(driver, DriverPriority::Standard);
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use super::*;
    use crate::device::{platform::*, GenericDevice};

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
            vec!["sifive,test0"]
        ));
        let mut manager = DeviceManager::new();
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
}
