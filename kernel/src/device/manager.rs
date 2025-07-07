//! # Device Manager Module
//!
//! This module provides functionality for managing hardware devices in the kernel.
//!
//! ## Overview
//!
//! The device manager is responsible for:
//! - Registering and managing serial devices
//! - Tracking available device drivers
//! - Device discovery and initialization
//! - Managing device information
//!
//! ## Key Components
//!
//! - `BasicDeviceManager`: Manages fundamental I/O devices like serial ports
//! - `DeviceManager`: The main device management system that handles all devices and drivers
//!
//! ## Device Discovery
//!
//! Devices are discovered through the Flattened Device Tree (FDT). The manager:
//! 1. Parses the device tree
//! 2. Matches compatible devices with registered drivers
//! 3. Probes devices with appropriate drivers
//!
//! ## Usage
//!
//! The device manager is implemented as a global singleton that can be accessed via:
//! - `DeviceManager::get_manager()` - Immutable access
//! - `DeviceManager::get_mut_manager()` - Mutable access
//!
//! ### Example: Registering a serial device
//!
//! ```
//! use crate::device::manager::register_serial;
//! 
//! // Create a new serial device
//! let my_serial = Box::new(MySerialImplementation::new());
//! 
//! // Register with the device manager
//! register_serial(my_serial);
//! ```

extern crate alloc;

use alloc::collections::btree_map::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::boxed::Box;
use spin::mutex::Mutex;
use spin::rwlock::RwLock;

use crate::device::platform::resource::PlatformDeviceResource;
use crate::device::platform::resource::PlatformDeviceResourceType;
use crate::device::platform::PlatformDeviceInfo;
use crate::println;

use crate::traits::serial::Serial;

use super::fdt::FdtManager;
use super::Device;
use super::DeviceDriver;
use super::DeviceInfo;
use super::DeviceType;

/// BasicDeviceManager
///
/// This struct manages basic I/O devices, such as serial ports.
/// It provides methods to register, borrow, and manage serial devices.
/// It is a part of the DeviceManager, which handles all devices and drivers.
///
/// # Fields
/// - `serials`: A vector of serial devices managed by this manager.
/// 
pub struct BasicDeviceManager {
    /* Basic I/O */
    serials: Vec<Box<dyn Serial>>,
}

impl BasicDeviceManager {
    pub fn register_serial(&mut self, serial: Box<dyn Serial>) {
        self.serials.push(serial);
        println!("Registered serial device");
    }

    pub fn register_serials(&mut self, serias: Vec<Box<dyn Serial>>) {
        let len = serias.len();
        for serial in serias {
            self.serials.push(serial);
        }
        println!("Registered serial devices: {}", len);
    }

    pub fn borrow_serial(&self, idx: usize) -> Option<&Box<dyn Serial>> {
        self.serials.get(idx)
    }

    pub fn borrow_mut_serial(&mut self, idx: usize) -> Option<&mut Box<dyn Serial>> {
        self.serials.get_mut(idx)
    }

    pub fn borrow_serials(&self) -> &Vec<Box<dyn Serial>> {
        &self.serials
    }

    pub fn borrow_mut_serials(&mut self) -> &mut Vec<Box<dyn Serial>> {
        &mut self.serials
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub enum DeviceState {
    Available,
    InUse,
    InUseExclusive,
}

pub struct DeviceHandle {
    device: Arc<RwLock<Box<dyn Device>>>,
    state: RwLock<DeviceState>,
    borrow_count: RwLock<usize>,
}

impl DeviceHandle {
    pub fn new(device: Arc<RwLock<Box<dyn Device>>>) -> Self {
        Self {
            device,
            state: RwLock::new(DeviceState::Available),
            borrow_count: RwLock::new(0),
        }
    }

    pub fn is_in_use(&self) -> bool {
        *self.state.read() == DeviceState::InUse || *self.state.read() == DeviceState::InUseExclusive
    }

    pub fn is_in_use_exclusive(&self) -> bool {
        *self.state.read() == DeviceState::InUseExclusive
    }

    fn set_in_use(&self) {
        let mut state = self.state.write();
        *state = DeviceState::InUse;
    }

    fn set_available(&self) {
        let mut state = self.state.write();
        *state = DeviceState::Available;
    }

    fn set_in_use_exclusive(&self) {
        let mut state = self.state.write();
        *state = DeviceState::InUseExclusive;
    }

    fn increment_borrow_count(&self) {
        let mut count = self.borrow_count.write();
        *count += 1;
    }

    fn decrement_borrow_count(&self) {
        let mut count = self.borrow_count.write();
        if *count > 0 {
            *count -= 1;
        }
    }

    pub fn get_borrow_count(&self) -> usize {
        *self.borrow_count.read()
    }
}

pub type BorrowedDevice = Arc<RwLock<Box<dyn Device>>>;

pub struct BorrowedDeviceGuard {
    handle: Arc<DeviceHandle>,
}

impl BorrowedDeviceGuard {
    pub fn new(handle: Arc<DeviceHandle>) -> Self {
        Self { handle }
    }
}

impl BorrowedDeviceGuard {
    pub fn device(&self) -> BorrowedDevice {
        Arc::clone(&self.handle.device)
    }
}

impl Drop for BorrowedDeviceGuard {
    fn drop(&mut self) {
        self.handle.decrement_borrow_count();
        if self.handle.get_borrow_count() == 0 {
            if self.handle.is_in_use_exclusive() {
                self.handle.set_available();
            } else {
                self.handle.set_available();
            }
        }
    }
}

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
/// It handles all devices and drivers, including basic I/O devices.
/// It provides methods to register devices, populate devices from the FDT,
/// and manage device drivers with priority-based initialization.
/// 
/// # Fields
/// - `basic`: An instance of `BasicDeviceManager` for managing basic I/O devices.
/// - `devices`: A mutex-protected vector of all registered devices.
/// - `drivers`: A mutex-protected map of device drivers organized by priority.
pub struct DeviceManager {
    /* Manager for basic devices */
    pub basic: BasicDeviceManager,
    /* Other devices */
    devices: Mutex<Vec<Arc<DeviceHandle>>>,
    /* Device drivers organized by priority */
    drivers: Mutex<BTreeMap<DriverPriority, Vec<Box<dyn DeviceDriver>>>>,
}

impl DeviceManager {
    const fn new() -> Self {
        DeviceManager {
            basic: BasicDeviceManager {
                serials: Vec::new(),
            },
            devices: Mutex::new(Vec::new()),
            drivers: Mutex::new(BTreeMap::new()),
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
    /// let device = Box::new(MyDevice::new());
    /// let id = DeviceManager::get_mut_manager().register_device(device);
    /// ```
    /// 
    pub fn register_device(&self, device: Box<dyn Device>) -> usize {
        let mut devices = self.devices.lock();
        let device = Arc::new(RwLock::new(device));
        let handle = DeviceHandle::new(device).into();
        let id = devices.len();
        devices.push(handle);
        id
    }

    /// Borrow a device by type and index
    /// 
    /// # Arguments
    /// 
    /// * `id`: The id of the device to borrow.
    /// 
    /// # Returns
    /// 
    /// A result containing a reference to the borrowed device, or an error if the device type is not found or the index is out of bounds.
    /// 
    pub fn borrow_device(&self, id: usize) -> Result<BorrowedDeviceGuard, &'static str> {
        let devices = self.devices.lock();    
        if id < devices.len() {
            let device = &devices[id];
            if device.is_in_use_exclusive() {
                return Err("Device is already in use exclusively");
            }
            device.increment_borrow_count(); // Increment borrow count
            device.set_in_use(); // Mark the device as in use
            return Ok(BorrowedDeviceGuard::new(Arc::clone(device)));
        } else {
            return Err("Index out of bounds");
        }
    }

    /// Borrow an exclusive device by type and index
    /// 
    /// # Arguments
    /// 
    /// * `id`: The id of the device to borrow.
    /// 
    /// # Returns
    /// 
    /// A result containing a reference to the borrowed device, or an error if the device type is not found or the index is out of bounds.
    /// 
    pub fn borrow_exclusive_device(&self, id: usize) -> Result<BorrowedDeviceGuard, &'static str> {
    let devices = self.devices.lock();
        if id < devices.len() {
            let handle = &devices[id];
            if handle.is_in_use() {
                return Err("Device is already in use");
            }
            handle.increment_borrow_count(); // Increment borrow count
            handle.set_in_use_exclusive(); // Mark the device as in use
            return Ok(BorrowedDeviceGuard::new(Arc::clone(handle)));
        } else {
            return Err("Index out of bounds");
        }
    }




    /// Get the number of devices of a specific type
    /// 
    /// # Returns
    /// 
    /// The number of devices of the specified type.
    /// 
    pub fn get_devices_count(&self) -> usize {
        let devices = self.devices.lock();
        devices.len()
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
            println!("FDT not initialized");
            return;
        }
        let fdt = fdt.unwrap();
        
        let priority_list = priorities.unwrap_or(DriverPriority::all());
        
        for &priority in priority_list {
            println!("Populating devices with {} drivers from FDT...", priority.description());
            
            let soc = fdt.find_node("/soc");
            if soc.is_none() {
                println!("No /soc node found");
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
                                println!("Failed to probe {} device {}: {}", priority.description(), device.name(), e);
                            } else {
                                println!("Successfully probed {} device: {}", priority.description(), device.name());
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
/// Registers a serial device with the device manager.
/// 
/// This function takes a boxed serial device and adds it to the list of registered serial devices.
/// It is used to register serial devices that can be used for I/O operations.
/// 
/// # Arguments
/// 
/// * `serial` - A boxed serial device that implements the `Serial` trait.
/// 
/// # Example
/// 
/// ```rust
/// let serial = Box::new(MySerialDevice::new());
/// register_serial(serial);
/// ```
pub fn register_serial(serial: Box<dyn Serial>) {
    let manager = DeviceManager::get_mut_manager();
    manager.basic.register_serial(serial);
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
        DeviceManager::get_mut_manager().register_driver(driver, DriverPriority::Standard);

        DeviceManager::get_mut_manager().populate_devices();
        let result = unsafe { TEST_RESULT };
        assert_eq!(result, true);
    }

    #[test_case]
    fn test_borrow_device_from_manager() {
        let device = Box::new(GenericDevice::new(
            "test",
            1,
        ));
        let manager = DeviceManager::get_mut_manager();
        let id = manager.register_device(device);
        let borrowed_device = manager.borrow_device(id);
        assert!(borrowed_device.is_ok());
        let borrowed_device = borrowed_device.unwrap();
        let device = borrowed_device.device();
        assert_eq!(device.read().name(), "test");
    }

    #[test_case]
    fn test_borrow_exclusive_device_from_manager() {
        let device = Box::new(GenericDevice::new(
            "test",
            1,
        ));
        let manager = DeviceManager::get_mut_manager();
        let id = manager.register_device(device);
        let borrowed_device = manager.borrow_exclusive_device(id);
        assert!(borrowed_device.is_ok());
        let borrowed_device = borrowed_device.unwrap();
        let device = borrowed_device.device();
        assert_eq!(device.read().name(), "test");
    }

    #[test_case]
    fn test_borrow_exclusive_device_from_manager_fail() {
        let device = Box::new(GenericDevice::new(
            "test",
            1,
        ));
        let manager = DeviceManager::get_mut_manager();
        let id = manager.register_device(device);
        let borrowed_device = manager.borrow_exclusive_device(id);
        assert!(borrowed_device.is_ok());
        let borrowed_device = borrowed_device.unwrap();
        let device = borrowed_device.device();
        assert_eq!(device.read().name(), "test");
        let borrowed_device2 = manager.borrow_exclusive_device(id);
        assert!(borrowed_device2.is_err());
    }

    #[test_case]
    fn test_drop_borrowed_device() {
        let device = Box::new(GenericDevice::new(
            "test",
            1,
        ));
        let manager = DeviceManager::get_mut_manager();
        let id = manager.register_device(device);
        {
            let borrowed_device = manager.borrow_device(id);
            assert!(borrowed_device.is_ok());
            let borrowed_device = borrowed_device.unwrap();
            let device = borrowed_device.device();
            assert_eq!(device.read().name(), "test");
            // The device should be in use now
            assert!(borrowed_device.handle.is_in_use());
            assert_eq!(borrowed_device.handle.get_borrow_count(), 1);
        }
        // After the borrowed device goes out of scope, we should be able to borrow it again
        let borrowed_device = manager.borrow_exclusive_device(id);
        assert!(borrowed_device.is_ok());
    }

    #[test_case]
    fn test_drop_multiple_borrowed_device() {
        let device = Box::new(GenericDevice::new(
            "test",
            1,
        ));
        let manager = DeviceManager::get_mut_manager();
        let id = manager.register_device(device);
        {
            let borrowed_device = manager.borrow_device(id);
            assert!(borrowed_device.is_ok());
            let borrowed_device = borrowed_device.unwrap();
            let device = borrowed_device.device();
            assert_eq!(device.read().name(), "test");
            // The device should be in use now
            assert!(borrowed_device.handle.is_in_use());
            assert_eq!(borrowed_device.handle.get_borrow_count(), 1);
            
            {
                // Second borrow
                let borrowed_device = manager.borrow_device(id).unwrap();
                let device = borrowed_device.device();
                assert_eq!(device.read().name(), "test");
                // The device should still be in use
                assert!(borrowed_device.handle.is_in_use());
                assert_eq!(borrowed_device.handle.get_borrow_count(), 2);
                // Drop the second borrow
            }
            // The device should still be in use
            assert!(borrowed_device.handle.is_in_use());
            // The borrow count should be 1
            assert_eq!(borrowed_device.handle.get_borrow_count(), 1);
        }
        // After the borrowed device goes out of scope, we should be able to borrow it again
        let borrowed_device = manager.borrow_exclusive_device(id);
        assert!(borrowed_device.is_ok());
    }

    #[test_case]
    fn test_borrow_out_of_bounds() {
        let manager = DeviceManager::get_manager();
        let borrowed_device = manager.borrow_device(999);
        assert!(borrowed_device.is_err());
    }

    #[test_case]
    fn test_borrow_while_exclusive() {
        let device = Box::new(GenericDevice::new("test", 1));
        let manager = DeviceManager::get_mut_manager();
        let id = manager.register_device(device);

        let borrowed_device = manager.borrow_exclusive_device(id);
        assert!(borrowed_device.is_ok());

        let borrowed_device2 = manager.borrow_device(id);
        assert!(borrowed_device2.is_err());
    }
}
