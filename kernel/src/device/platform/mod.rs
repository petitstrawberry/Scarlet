//! Platform device driver module.
//! 
//! This module provides the implementation of platform device drivers, including
//! device information and driver management. It defines the `PlatformDeviceInfo` and
//! `PlatformDeviceDriver` structs, which represent the device information and driver
//! respectively.
//! 
//! The module implements the `Device` and `DeviceDriver` traits for platform devices,
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
//!     vec!["example,device-v1", "example,device-v2"],
//!     Vec::new() // Add resources as an empty vector
//! );
//!
//! // Creating a platform device driver
//! let driver = PlatformDeviceDriver::new(
//!     "example_driver",
//!     |device_info| { /* probe implementation */ Ok(()) },
//!     |device_info| { /* remove implementation */ Ok(()) },
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
//! respectively. The module also includes the `Device` and `DeviceDriver` traits,
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
    resources: Vec<PlatformDeviceResource>,
}

/// Information about a platform device.
///
/// This structure holds the basic identifying information for platform devices,
/// including a name, unique identifier, compatibility strings, and resources.
///
/// # Fields
/// - `name`: A static string representing the name of the device
/// - `id`: A unique identifier for the device
/// - `compatible`: A list of compatibility strings that describe compatible drivers
/// - `resources`: A list of resources associated with the device
///
/// # Examples
///
/// ```
/// let device_info = PlatformDeviceInfo::new(
///     "uart0",
///     0,
///     vec!["ns16550a", "uart"],
///     Vec::new() // Add resources as an empty vector
/// );
///
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
    pub fn new(name: &'static str, id: usize, compatible: Vec<&'static str>, resources: Vec<PlatformDeviceResource>) -> Self {
        Self {
            name,
            id,
            compatible,
            resources,
        }
    }

    /// Get the `PlatformDeviceResource` associated with the device.
    /// 
    /// # Returns
    /// 
    /// A reference to a vector of `PlatformDeviceResource` objects.
    /// 
    pub fn get_resources(&self) -> &Vec<PlatformDeviceResource> {
        &self.resources
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

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct PlatformDeviceDriver {
    name: &'static str,
    probe_fn: fn(&PlatformDeviceInfo) -> Result<(), &'static str>,
    remove_fn: fn(&PlatformDeviceInfo) -> Result<(), &'static str>,
    compatible: Vec<&'static str>, // Change to Vec<&'static str>
}

impl PlatformDeviceDriver {
    pub fn new(
        name: &'static str,
        probe_fn: fn(&PlatformDeviceInfo) -> Result<(), &'static str>,
        remove_fn: fn(&PlatformDeviceInfo) -> Result<(), &'static str>,
        compatible: Vec<&'static str>,
    ) -> Self {
        Self {
            name,
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
        // Downcast the device to a `PlatformDeviceInfo`
        let platform_device_info = device.as_any()
            .downcast_ref::<PlatformDeviceInfo>()
            .ok_or("Failed to downcast to PlatformDeviceInfo")?;
        // Call the probe function
        (self.probe_fn)(platform_device_info)
    }

    fn remove(&self, _device: &dyn DeviceInfo) -> Result<(), &'static str> {
        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test_case]
    fn test_probe_success() {
        let device = PlatformDeviceInfo::new("test_device", 1, vec!["test,compatible"], vec![]);
        let driver = PlatformDeviceDriver::new(
            "test_driver",
            |device_info| {
                assert_eq!(device_info.name(), "test_device");
                Ok(())
            },
            |_device| Ok(()),
            vec!["test,compatible"],
        );

        let result = driver.probe(&device);
        assert!(result.is_ok());
    }

    #[test_case]
    fn test_probe_failure() {
        struct DummyDevice;
        impl DeviceInfo for DummyDevice {
            fn name(&self) -> &'static str { "dummy" }
            fn id(&self) -> usize { 0 }
            fn compatible(&self) -> Vec<&'static str> { vec![] }
            fn as_any(&self) -> &dyn Any { self }
        }

        let device = DummyDevice;
        let driver = PlatformDeviceDriver::new(
            "test_driver",
            |_device| Ok(()),
            |_device| Ok(()),
            vec!["test,compatible"],
        );

        let result = driver.probe(&device);
        assert!(result.is_err());
    }
}