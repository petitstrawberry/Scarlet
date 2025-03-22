//! Platform device driver module.
//! 
//! This module provides the implementation of platform device drivers, including
//! device information and driver management. It defines the `PlatformDeviceInfo` and
//! `PlatformDeviceDriver` structs, which represent the device information and driver
//! respectively.
//! 
//! The module implements the `DeviceInfo` and `DeviceDriver` traits for platform devices,
//! allowing them to be integrated into the device management system.
//!
//! # Components
//!
//! - `PlatformDeviceInfo`: Stores information about a platform device, including its name,
//!   ID, and compatible device strings.
//!
//! - `PlatformDeviceDriver`: Implements a driver for platform devices, containing resources,
//!   probe and remove functions, and compatibility information.
//!
//! # Submodules
//!
//! - `resource`: Defines platform-specific device resources.
//!
//! # Examples
//!
//! ```rust
//! // Creating a platform device info
//! let device_info = PlatformDeviceInfo::new(
//!     "example_device",
//!     1,
//!     vec!["example,device-v1", "example,device-v2"]
//! );
//!
//! // Creating a platform device driver
//! let driver = PlatformDeviceDriver::new(
//!     "example_driver",
//!     vec![],
//!     |device| { /* probe implementation */ Ok(()) },
//!     |device| { /* remove implementation */ Ok(()) },
//!     vec!["example,device-v1", "example,device-v2"]
//! );
//! ```
//!
//! # Implementation Details
//!
//! Platform devices are hardware components that are directly connected to the
//! system bus or integrated into the SoC. They are typically discovered during
//! boot time through firmware tables (like ACPI or Device Tree).
//!
//! The driver model allows for dynamic matching between devices and their drivers
//! based on the compatible strings, enabling a flexible plug-and-play architecture.
//! respectively. The module also includes the `DeviceInfo` and `DeviceDriver` traits,
//! which define the interface for device information and drivers.
//!


pub mod resource;

extern crate alloc;
use alloc::vec::Vec;

use super::*;
use resource::*;

/// Struct representing platform device information.
pub struct PlatformDeviceInfo {
    name: &'static str,
    id: usize,
    compatible: Vec<&'static str>,
}

/// Information about a platform device.
///
/// This structure holds the basic identifying information for platform devices,
/// including a name, unique identifier, and compatibility strings.
///
/// # Fields
/// - `name`: A static string representing the name of the device
/// - `id`: A unique identifier for the device
/// - `compatible`: A list of compatibility strings that describe compatible drivers
///
/// # Examples
///
/// ```
/// let device_info = PlatformDeviceInfo::new(
///     "uart0",
///     0,
///     vec!["ns16550a", "uart"]
/// );
/// ```
impl PlatformDeviceInfo {
    /// Creates a new `PlatformDeviceInfo` instance.
    ///
    /// # Arguments
    ///
    /// * `name` - Static string identifier for the device
    /// * `id` - Unique identifier number
    /// * `compatible` - List of compatible device identifiers
    ///
    /// # Returns
    ///
    /// A new `PlatformDeviceInfo` instance with the provided values.
    pub fn new(name: &'static str, id: usize, compatible: Vec<&'static str>) -> Self {
        Self {
            name,
            id,
            compatible,
        }
    }
}

impl DeviceInfo for PlatformDeviceInfo {
    fn name(&self) -> &'static str {
        self.name
    }

    fn id(&self) -> usize {
        self.id
    }

    fn compatible(&self) -> Vec<&'static str> {
        self.compatible.clone()
    }
}

pub struct PlatformDeviceDriver {
    name: &'static str,
    resources: Vec<PlatformDeviceResource>,
    probe_fn: fn(&dyn DeviceInfo) -> Result<(), &'static str>,
    remove_fn: fn(&dyn DeviceInfo) -> Result<(), &'static str>,
    compatible: Vec<&'static str>, // Change to Vec<&'static str>
}

impl PlatformDeviceDriver {
    pub fn new(
        name: &'static str,
        resources: Vec<PlatformDeviceResource>,
        probe_fn: fn(&dyn DeviceInfo) -> Result<(), &'static str>,
        remove_fn: fn(&dyn DeviceInfo) -> Result<(), &'static str>,
        compatible: Vec<&'static str>,
    ) -> Self {
        Self {
            name,
            resources,
            probe_fn,           
            remove_fn,
            compatible,
        }
    }
}

impl DeviceDriver for PlatformDeviceDriver {
    fn name(&self) -> &'static str {
        self.name
    }

    fn match_table(&self) -> Vec<&'static str> {
        self.compatible.clone()
    }

    fn probe(&self, device: &dyn DeviceInfo) -> Result<(), &'static str> {
        (self.probe_fn)(device)
    }

    fn remove(&self, _device: &dyn DeviceInfo) -> Result<(), &'static str> {
        Ok(())
    }
}

