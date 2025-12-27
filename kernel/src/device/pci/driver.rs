//! PCI device driver support.
//!
//! This module defines the PCI driver structure that can match and probe PCI devices.

extern crate alloc;

use alloc::vec::Vec;
use crate::device::{DeviceDriver, DeviceInfo};
use super::device::PciDeviceInfo;

/// PCI device ID for driver matching
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciDeviceId {
    /// Vendor ID (or 0xFFFF for any)
    pub vendor: u16,
    /// Device ID (or 0xFFFF for any)
    pub device: u16,
    /// Subvendor ID (or 0xFFFF for any)
    pub subvendor: u16,
    /// Subdevice ID (or 0xFFFF for any)
    pub subdevice: u16,
    /// Class code (or 0xFFFFFF for any)
    pub class: u32,
    /// Class mask (which bits of class to match)
    pub class_mask: u32,
}

impl PciDeviceId {
    /// Match any vendor and device
    pub const ANY: u16 = 0xFFFF;

    /// Match any class
    pub const ANY_CLASS: u32 = 0xFFFFFF;

    /// Create a new PCI device ID matcher
    pub const fn new(vendor: u16, device: u16) -> Self {
        Self {
            vendor,
            device,
            subvendor: Self::ANY,
            subdevice: Self::ANY,
            class: Self::ANY_CLASS,
            class_mask: 0,
        }
    }

    /// Create a matcher for a specific class
    pub const fn from_class(class: u32, mask: u32) -> Self {
        Self {
            vendor: Self::ANY,
            device: Self::ANY,
            subvendor: Self::ANY,
            subdevice: Self::ANY,
            class,
            class_mask: mask,
        }
    }

    /// Check if this ID matches a PCI device
    pub fn matches(&self, device: &PciDeviceInfo) -> bool {
        // Check vendor/device ID
        if self.vendor != Self::ANY && self.vendor != device.vendor_id() {
            return false;
        }
        if self.device != Self::ANY && self.device != device.device_id() {
            return false;
        }

        // Check subsystem vendor/device ID
        if self.subvendor != Self::ANY && self.subvendor != device.subsystem_vendor_id() {
            return false;
        }
        if self.subdevice != Self::ANY && self.subdevice != device.subsystem_id() {
            return false;
        }

        // Check class code with mask
        if self.class_mask != 0 {
            let device_class = device.class_code();
            if (device_class & self.class_mask) != (self.class & self.class_mask) {
                return false;
            }
        }

        true
    }
}

/// PCI device driver
///
/// Implements the DeviceDriver trait for PCI devices.
pub struct PciDeviceDriver {
    /// Driver name
    name: &'static str,
    /// List of PCI device IDs this driver supports
    id_table: Vec<PciDeviceId>,
    /// Probe function
    probe_fn: fn(&PciDeviceInfo) -> Result<(), &'static str>,
    /// Remove function
    remove_fn: fn(&PciDeviceInfo) -> Result<(), &'static str>,
}

impl PciDeviceDriver {
    /// Create a new PCI device driver
    ///
    /// # Arguments
    ///
    /// * `name` - Driver name
    /// * `id_table` - List of supported PCI device IDs
    /// * `probe_fn` - Function to probe devices
    /// * `remove_fn` - Function to remove devices
    pub fn new(
        name: &'static str,
        id_table: Vec<PciDeviceId>,
        probe_fn: fn(&PciDeviceInfo) -> Result<(), &'static str>,
        remove_fn: fn(&PciDeviceInfo) -> Result<(), &'static str>,
    ) -> Self {
        Self {
            name,
            id_table,
            probe_fn,
            remove_fn,
        }
    }

    /// Get the device ID table
    pub fn id_table(&self) -> &[PciDeviceId] {
        &self.id_table
    }

    /// Check if this driver matches a PCI device
    pub fn matches_device(&self, device: &PciDeviceInfo) -> bool {
        self.id_table.iter().any(|id| id.matches(device))
    }
}

impl DeviceDriver for PciDeviceDriver {
    fn name(&self) -> &'static str {
        self.name
    }

    fn match_table(&self) -> Vec<&'static str> {
        // PCI drivers use device IDs (PciDeviceId) rather than string matching.
        // The actual matching is done by the matches_device() method which checks
        // vendor/device IDs and class codes. This returns an empty vector as
        // string-based matching is not used for PCI devices.
        Vec::new()
    }

    fn probe(&self, device: &dyn DeviceInfo) -> Result<(), &'static str> {
        // Downcast to PciDeviceInfo
        let pci_device = device
            .as_any()
            .downcast_ref::<PciDeviceInfo>()
            .ok_or("Failed to downcast to PciDeviceInfo")?;

        // Check if this driver matches the device
        if !self.matches_device(pci_device) {
            return Err("Device does not match driver");
        }

        // Call the probe function
        (self.probe_fn)(pci_device)
    }

    fn remove(&self, device: &dyn DeviceInfo) -> Result<(), &'static str> {
        // Downcast to PciDeviceInfo
        let pci_device = device
            .as_any()
            .downcast_ref::<PciDeviceInfo>()
            .ok_or("Failed to downcast to PciDeviceInfo")?;

        // Call the remove function
        (self.remove_fn)(pci_device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::pci::PciAddress;

    #[test_case]
    fn test_pci_device_id_matching() {
        let addr = PciAddress::new(0, 0, 1, 0);
        let device = PciDeviceInfo::new(
            addr,
            0x8086, // Intel
            0x1234,
            0x020000, // Network controller
            0x01,
            0x0000,
            0x0000,
            0x0B,
            0x01,
            "pci_device",
            1,
        );

        // Test exact match
        let id = PciDeviceId::new(0x8086, 0x1234);
        assert!(id.matches(&device));

        // Test vendor mismatch
        let id = PciDeviceId::new(0x1234, 0x1234);
        assert!(!id.matches(&device));

        // Test device mismatch
        let id = PciDeviceId::new(0x8086, 0x5678);
        assert!(!id.matches(&device));

        // Test ANY vendor
        let id = PciDeviceId::new(PciDeviceId::ANY, 0x1234);
        assert!(id.matches(&device));

        // Test class matching
        let id = PciDeviceId::from_class(0x020000, 0xFF0000); // Match network class
        assert!(id.matches(&device));

        // Test class mismatch
        let id = PciDeviceId::from_class(0x030000, 0xFF0000); // Display class
        assert!(!id.matches(&device));
    }

    #[test_case]
    fn test_pci_driver_matching() {
        let addr = PciAddress::new(0, 0, 1, 0);
        let device = PciDeviceInfo::new(
            addr,
            0x8086,
            0x1234,
            0x020000,
            0x01,
            0x0000,
            0x0000,
            0x0B,
            0x01,
            "pci_device",
            1,
        );

        let id_table = alloc::vec![
            PciDeviceId::new(0x8086, 0x1234),
            PciDeviceId::new(0x8086, 0x5678),
        ];

        let driver = PciDeviceDriver::new(
            "test_driver",
            id_table,
            |_device| Ok(()),
            |_device| Ok(()),
        );

        assert!(driver.matches_device(&device));
    }

    #[test_case]
    fn test_pci_driver_probe() {
        let addr = PciAddress::new(0, 0, 1, 0);
        let device = PciDeviceInfo::new(
            addr,
            0x8086,
            0x1234,
            0x020000,
            0x01,
            0x0000,
            0x0000,
            0x0B,
            0x01,
            "pci_device",
            1,
        );

        static mut PROBE_CALLED: bool = false;

        let id_table = alloc::vec![PciDeviceId::new(0x8086, 0x1234)];

        let driver = PciDeviceDriver::new(
            "test_driver",
            id_table,
            |dev| {
                unsafe { PROBE_CALLED = true; }
                assert_eq!(dev.vendor_id(), 0x8086);
                Ok(())
            },
            |_device| Ok(()),
        );

        let result = driver.probe(&device);
        assert!(result.is_ok());
        assert!(unsafe { PROBE_CALLED });
    }

    #[test_case]
    fn test_virtio_pci_stub_driver_probe() {
        // Test simulating virtio-pci devices (Red Hat vendor ID 0x1AF4)
        // This verifies that PCI probe works with real-world device IDs
        
        static mut VIRTIO_NET_PROBED: bool = false;
        static mut VIRTIO_BLK_PROBED: bool = false;
        
        // Create stub virtio-pci driver that supports common virtio devices
        let id_table = alloc::vec![
            PciDeviceId::new(0x1AF4, 0x1000), // VirtIO net (legacy)
            PciDeviceId::new(0x1AF4, 0x1001), // VirtIO block (legacy)
            PciDeviceId::new(0x1AF4, 0x1041), // VirtIO net (modern)
            PciDeviceId::new(0x1AF4, 0x1042), // VirtIO block (modern)
        ];
        
        let driver = PciDeviceDriver::new(
            "virtio-pci-stub",
            id_table,
            |device| {
                // Stub probe function - just verify device info
                match device.device_id() {
                    0x1000 | 0x1041 => {
                        // VirtIO net
                        unsafe { VIRTIO_NET_PROBED = true; }
                        assert_eq!(device.vendor_id(), 0x1AF4);
                        assert_eq!(device.base_class(), 0x02); // Network
                    }
                    0x1001 | 0x1042 => {
                        // VirtIO block
                        unsafe { VIRTIO_BLK_PROBED = true; }
                        assert_eq!(device.vendor_id(), 0x1AF4);
                        assert_eq!(device.base_class(), 0x01); // Storage
                    }
                    _ => return Err("Unknown device"),
                }
                Ok(())
            },
            |_device| Ok(()),
        );
        
        // Test 1: VirtIO net device (legacy)
        let addr = PciAddress::new(0, 0, 1, 0);
        let virtio_net = PciDeviceInfo::new(
            addr,
            0x1AF4,    // Red Hat vendor
            0x1000,    // VirtIO net (legacy)
            0x020000,  // Network controller
            0x00,
            0x1AF4,
            0x0001,
            0x0B,
            0x01,
            "virtio_pci_device",
            1,
        );
        
        assert!(driver.matches_device(&virtio_net));
        let result = driver.probe(&virtio_net);
        assert!(result.is_ok());
        assert!(unsafe { VIRTIO_NET_PROBED });
        
        // Test 2: VirtIO block device (legacy)
        let addr = PciAddress::new(0, 0, 2, 0);
        let virtio_blk = PciDeviceInfo::new(
            addr,
            0x1AF4,    // Red Hat vendor
            0x1001,    // VirtIO block (legacy)
            0x010000,  // Storage controller
            0x00,
            0x1AF4,
            0x0002,
            0x0B,
            0x01,
            "virtio_pci_device",
            2,
        );
        
        assert!(driver.matches_device(&virtio_blk));
        let result = driver.probe(&virtio_blk);
        assert!(result.is_ok());
        assert!(unsafe { VIRTIO_BLK_PROBED });
        
        // Test 3: Non-matching device should not be probed
        let addr = PciAddress::new(0, 0, 3, 0);
        let intel_device = PciDeviceInfo::new(
            addr,
            0x8086,    // Intel vendor
            0x1234,
            0x020000,
            0x00,
            0x0000,
            0x0000,
            0x0B,
            0x01,
            "intel_device",
            3,
        );
        
        assert!(!driver.matches_device(&intel_device));
    }

    #[test_case]
    fn test_virtio_pci_class_based_matching() {
        // Test class-based matching for virtio devices
        // This is useful when you want to match all devices of a certain class
        // regardless of vendor
        
        static mut MATCHED: bool = false;
        
        // Create driver that matches all network controllers
        let id_table = alloc::vec![
            PciDeviceId::from_class(0x020000, 0xFF0000), // Network class
        ];
        
        let driver = PciDeviceDriver::new(
            "network-stub",
            id_table,
            |device| {
                unsafe { MATCHED = true; }
                assert_eq!(device.base_class(), 0x02);
                Ok(())
            },
            |_device| Ok(()),
        );
        
        // Should match virtio-net
        let addr = PciAddress::new(0, 0, 1, 0);
        let virtio_net = PciDeviceInfo::new(
            addr,
            0x1AF4,
            0x1000,
            0x020000,  // Network controller
            0x00,
            0x1AF4,
            0x0001,
            0x0B,
            0x01,
            "virtio_net",
            1,
        );
        
        assert!(driver.matches_device(&virtio_net));
        let result = driver.probe(&virtio_net);
        assert!(result.is_ok());
        assert!(unsafe { MATCHED });
    }
}
