//! PCI (Peripheral Component Interconnect) bus module.
//!
//! This module provides support for PCI device discovery and management.
//! It implements PCI configuration space access, device enumeration, and
//! integration with the device manager.
//!
//! # Overview
//!
//! PCI is a standard bus for connecting peripheral devices to a computer system.
//! This implementation focuses on PCIe (PCI Express) using ECAM (Enhanced Configuration
//! Access Mechanism) which is commonly used on RISC-V and ARM platforms.
//!
//! # Architecture
//!
//! The PCI subsystem consists of several components:
//! - **Configuration Space Access**: Reading and writing PCI configuration registers
//! - **Device Enumeration**: Scanning the PCI bus tree to discover devices
//! - **Device Information**: Representing PCI device properties (vendor, device ID, etc.)
//! - **Driver Matching**: Matching PCI devices with appropriate drivers
//!
//! # Integration with DeviceManager
//!
//! PCI devices are discovered and registered with the DeviceManager, similar to
//! platform devices. The PCI subsystem provides:
//! - `PciDeviceInfo`: Device information structure implementing `DeviceInfo` trait
//! - `PciDeviceDriver`: Driver structure implementing `DeviceDriver` trait
//!
//! # Usage
//!
//! ```rust,no_run
//! use crate::device::pci::PciBus;
//! use crate::device::manager::DeviceManager;
//!
//! // Initialize PCI bus with ECAM base address from device tree
//! let pci_bus = PciBus::new(ecam_base_addr, ecam_size);
//!
//! // Scan for devices and register with DeviceManager
//! pci_bus.scan_and_register();
//! ```
//!
//! # Configuration Space Layout
//!
//! PCI configuration space is 256 bytes (4KB for PCIe) per function:
//! - 0x00-0x3F: Standard PCI configuration header
//! - 0x40-0xFF: Device-specific configuration
//! - 0x100-0xFFF: PCIe extended configuration (PCIe only)
//!

pub mod config;
pub mod device;
pub mod driver;
pub mod scan;

extern crate alloc;

use alloc::vec::Vec;
use spin::mutex::Mutex;

/// PCI device address components
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciAddress {
    /// PCI segment/domain (usually 0)
    pub segment: u16,
    /// Bus number (0-255)
    pub bus: u8,
    /// Device number (0-31)
    pub device: u8,
    /// Function number (0-7)
    pub function: u8,
}

impl PciAddress {
    /// Create a new PCI address
    pub const fn new(segment: u16, bus: u8, device: u8, function: u8) -> Self {
        Self {
            segment,
            bus,
            device,
            function,
        }
    }

    /// Calculate ECAM offset for this address
    ///
    /// ECAM uses a flat memory mapping where each function gets 4KB:
    /// offset = (bus << 20) | (device << 15) | (function << 12)
    pub const fn ecam_offset(&self) -> usize {
        ((self.bus as usize) << 20)
            | ((self.device as usize) << 15)
            | ((self.function as usize) << 12)
    }
}

/// PCI bus manager
///
/// Manages PCI device discovery and configuration space access.
pub struct PciBus {
    /// ECAM (Enhanced Configuration Access Mechanism) base address
    ecam_base: usize,
    /// ECAM region size in bytes
    ecam_size: usize,
    /// List of discovered PCI devices
    devices: Mutex<Vec<device::PciDeviceInfo>>,
}

impl PciBus {
    /// Create a new PCI bus manager
    ///
    /// # Arguments
    ///
    /// * `ecam_base` - Physical address of the ECAM region
    /// * `ecam_size` - Size of the ECAM region in bytes
    ///
    /// # Returns
    ///
    /// A new `PciBus` instance
    pub const fn new(ecam_base: usize, ecam_size: usize) -> Self {
        Self {
            ecam_base,
            ecam_size,
            devices: Mutex::new(Vec::new()),
        }
    }

    /// Get the ECAM base address
    pub const fn ecam_base(&self) -> usize {
        self.ecam_base
    }

    /// Get the ECAM size
    pub const fn ecam_size(&self) -> usize {
        self.ecam_size
    }

    /// Check if a PCI address is within the ECAM region
    pub fn is_valid_address(&self, addr: &PciAddress) -> bool {
        let offset = addr.ecam_offset();
        offset + 0x1000 <= self.ecam_size // Each function needs 4KB
    }

    /// Get the list of discovered devices
    pub fn devices(&self) -> Vec<device::PciDeviceInfo> {
        let devices = self.devices.lock();
        devices.clone()
    }

    /// Add a device to the list of discovered devices
    pub fn add_device(&self, device: device::PciDeviceInfo) {
        let mut devices = self.devices.lock();
        devices.push(device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_pci_address_ecam_offset() {
        let addr = PciAddress::new(0, 0, 0, 0);
        assert_eq!(addr.ecam_offset(), 0);

        let addr = PciAddress::new(0, 1, 0, 0);
        assert_eq!(addr.ecam_offset(), 1 << 20);

        let addr = PciAddress::new(0, 0, 1, 0);
        assert_eq!(addr.ecam_offset(), 1 << 15);

        let addr = PciAddress::new(0, 0, 0, 1);
        assert_eq!(addr.ecam_offset(), 1 << 12);

        let addr = PciAddress::new(0, 1, 2, 3);
        assert_eq!(addr.ecam_offset(), (1 << 20) | (2 << 15) | (3 << 12));
    }

    #[test_case]
    fn test_pci_bus_creation() {
        let pci_bus = PciBus::new(0x3000_0000, 0x1000_0000);
        assert_eq!(pci_bus.ecam_base(), 0x3000_0000);
        assert_eq!(pci_bus.ecam_size(), 0x1000_0000);
    }

    #[test_case]
    fn test_pci_address_validity() {
        let pci_bus = PciBus::new(0x3000_0000, 0x100000); // 1MB ECAM region

        // Valid addresses
        let addr = PciAddress::new(0, 0, 0, 0);
        assert!(pci_bus.is_valid_address(&addr));

        // Address at the edge (offset + 4KB <= size)
        let max_offset = 0x100000 - 0x1000; // 1MB - 4KB
        let max_bus = (max_offset >> 20) as u8;
        let addr = PciAddress::new(0, max_bus, 0, 0);
        assert!(pci_bus.is_valid_address(&addr));

        // Invalid address (beyond ECAM region)
        let addr = PciAddress::new(0, 255, 31, 7);
        assert!(!pci_bus.is_valid_address(&addr));
    }

    #[test_case]
    fn test_pci_bus_virtio_integration() {
        use crate::device::DeviceDriver;
        use crate::device::pci::device::PciDeviceInfo;
        use crate::device::pci::driver::{PciDeviceDriver, PciDeviceId};

        // Simulate a PCI bus with virtio devices
        let pci_bus = PciBus::new(0x3000_0000, 0x1000_0000);

        static mut VIRTIO_PROBED_COUNT: usize = 0;

        // Create stub virtio-pci driver
        let id_table = alloc::vec![
            PciDeviceId::new(0x1AF4, 0x1000), // VirtIO net
            PciDeviceId::new(0x1AF4, 0x1001), // VirtIO block
        ];

        let driver = PciDeviceDriver::new(
            "virtio-pci-integration",
            id_table,
            |device| {
                unsafe {
                    VIRTIO_PROBED_COUNT += 1;
                }
                // Verify this is a virtio device
                assert_eq!(device.vendor_id(), 0x1AF4);
                assert!(device.device_id() == 0x1000 || device.device_id() == 0x1001);
                Ok(())
            },
            |_device| Ok(()),
        );

        // Simulate discovered virtio-net device
        let addr1 = PciAddress::new(0, 0, 1, 0);
        let virtio_net = PciDeviceInfo::new(
            addr1,
            0x1AF4,
            0x1000,
            0x020000,
            0x00,
            0x1AF4,
            0x0001,
            0x0B,
            0x01,
            "virtio_net",
            1,
        );
        pci_bus.add_device(virtio_net.clone());

        // Simulate discovered virtio-blk device
        let addr2 = PciAddress::new(0, 0, 2, 0);
        let virtio_blk = PciDeviceInfo::new(
            addr2,
            0x1AF4,
            0x1001,
            0x010000,
            0x00,
            0x1AF4,
            0x0002,
            0x0B,
            0x01,
            "virtio_blk",
            2,
        );
        pci_bus.add_device(virtio_blk.clone());

        // Verify devices were added
        let devices = pci_bus.devices();
        assert_eq!(devices.len(), 2);

        // Probe devices with driver using DeviceDriver trait
        for device in devices {
            if driver.matches_device(&device) {
                // Use the DeviceDriver trait's probe method
                let result = DeviceDriver::probe(&driver, &device);
                assert!(result.is_ok());
            }
        }

        // Verify both devices were probed
        assert_eq!(unsafe { VIRTIO_PROBED_COUNT }, 2);
    }
}
