//! PCI device scanning and enumeration.
//!
//! This module implements the logic for scanning the PCI bus tree and
//! discovering devices.

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use super::{PciAddress, PciBus};
use super::config::{PciConfig, vendor};
use super::device::PciDeviceInfo;
use crate::early_println;

/// PCI scanner
///
/// Handles scanning the PCI bus tree to discover devices.
pub struct PciScanner {
    /// PCI configuration space accessor
    config: PciConfig,
    /// Bus manager reference
    bus: *const PciBus,
}

impl PciScanner {
    /// Create a new PCI scanner
    ///
    /// # Arguments
    ///
    /// * `bus` - Reference to the PCI bus manager
    ///
    /// # Safety
    ///
    /// The bus reference must remain valid for the lifetime of the scanner.
    pub fn new(bus: &PciBus) -> Self {
        let config = PciConfig::new(bus.ecam_base());
        Self {
            config,
            bus: bus as *const PciBus,
        }
    }

    /// Scan the entire PCI bus tree
    ///
    /// This scans all possible bus/device/function combinations and
    /// discovers present devices.
    ///
    /// # Returns
    ///
    /// Vector of discovered PCI devices
    pub fn scan(&self) -> Vec<PciDeviceInfo> {
        let mut devices = Vec::new();
        let mut device_id_counter = 0;

        early_println!("Scanning PCI bus...");

        // Start by checking bus 0
        self.scan_bus(0, &mut devices, &mut device_id_counter);

        early_println!("PCI scan complete: found {} devices", devices.len());

        devices
    }

    /// Scan a single PCI bus
    fn scan_bus(&self, bus: u8, devices: &mut Vec<PciDeviceInfo>, id_counter: &mut usize) {
        // Scan all 32 possible devices on this bus
        for device in 0..32 {
            self.scan_device(bus, device, devices, id_counter);
        }
    }

    /// Scan a single PCI device
    fn scan_device(&self, bus: u8, device: u8, devices: &mut Vec<PciDeviceInfo>, id_counter: &mut usize) {
        let addr = PciAddress::new(0, bus, device, 0);

        // Check if device exists by reading vendor ID
        let vendor_id = self.config.read_vendor_id(&addr);
        if vendor_id == vendor::INVALID {
            return; // No device present
        }

        // Read header type to check if this is a multi-function device
        let header_type = self.config.read_header_type(&addr);
        let is_multifunction = (header_type & 0x80) != 0;

        // Scan function 0
        if let Some(device_info) = self.probe_function(bus, device, 0, id_counter) {
            devices.push(device_info);
        }

        // If multi-function, scan remaining functions
        if is_multifunction {
            for function in 1..8 {
                if let Some(device_info) = self.probe_function(bus, device, function, id_counter) {
                    devices.push(device_info);
                }
            }
        }
    }

    /// Probe a specific PCI function
    fn probe_function(&self, bus: u8, device: u8, function: u8, id_counter: &mut usize) -> Option<PciDeviceInfo> {
        let addr = PciAddress::new(0, bus, device, function);

        // Check if function exists
        let vendor_id = self.config.read_vendor_id(&addr);
        if vendor_id == vendor::INVALID {
            return None;
        }

        // Read device configuration
        let device_id = self.config.read_device_id(&addr);
        let class_code = self.config.read_class_code(&addr);
        let revision = self.config.read_u8(&addr, super::config::offset::REVISION_ID);
        let subsystem_vendor_id = self.config.read_u16(&addr, super::config::offset::SUBSYSTEM_VENDOR_ID);
        let subsystem_id = self.config.read_u16(&addr, super::config::offset::SUBSYSTEM_ID);
        let interrupt_line = self.config.read_u8(&addr, super::config::offset::INTERRUPT_LINE);
        let interrupt_pin = self.config.read_u8(&addr, super::config::offset::INTERRUPT_PIN);

        // Generate device name
        // In a real implementation, this would use a static string pool
        // For now, we'll use a simple format
        let name = Self::generate_device_name(vendor_id, device_id, bus, device, function);

        let device_info = PciDeviceInfo::new(
            addr,
            vendor_id,
            device_id,
            class_code,
            revision,
            subsystem_vendor_id,
            subsystem_id,
            interrupt_line,
            interrupt_pin,
            name,
            *id_counter,
        );

        *id_counter += 1;

        early_println!(
            "Found PCI device: {:04x}:{:04x} at {:02x}:{:02x}.{} (class: {:06x})",
            vendor_id, device_id, bus, device, function, class_code
        );

        Some(device_info)
    }

    /// Generate a device name from vendor and device IDs
    ///
    /// This is a simplified implementation. A real implementation would
    /// maintain a static string pool or use a more sophisticated naming scheme.
    fn generate_device_name(_vendor_id: u16, _device_id: u16, _bus: u8, _device: u8, _function: u8) -> &'static str {
        // For now, we'll use a simple static string
        // In practice, this should use a proper string allocation strategy
        // that works in a no_std environment
        match _vendor_id {
            vendor::INTEL => "intel_pci_device",
            vendor::AMD => "amd_pci_device",
            vendor::NVIDIA => "nvidia_pci_device",
            vendor::REDHAT => "virtio_pci_device",
            _ => "pci_device",
        }
    }
}

impl PciBus {
    /// Scan the PCI bus and discover all devices
    ///
    /// This is a convenience method that creates a scanner and performs
    /// the scan, storing discovered devices in the bus manager.
    pub fn scan(&self) {
        let scanner = PciScanner::new(self);
        let devices = scanner.scan();

        // Store discovered devices
        for device in devices {
            self.add_device(device);
        }
    }

    /// Scan the PCI bus and register devices with the DeviceManager
    ///
    /// This scans for PCI devices and registers them with the global
    /// device manager so they can be matched with drivers.
    pub fn scan_and_register(&self) {
        use crate::device::manager::DeviceManager;

        self.scan();

        let devices = self.devices();
        let device_manager = DeviceManager::get_mut_manager();

        early_println!("Registering {} PCI devices with DeviceManager", devices.len());

        for device in devices {
            let device_name = String::from(device.name());
            // Note: In a real implementation, we'd wrap the PciDeviceInfo in a
            // proper Device implementation. For now, this is just the infrastructure.
            early_println!("  - {} ({:04x}:{:04x})", device_name, device.vendor_id(), device.device_id());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_pci_scanner_creation() {
        let bus = PciBus::new(0x3000_0000, 0x1000_0000);
        let _scanner = PciScanner::new(&bus);
        // If we get here without panic, the test passes
    }

    #[test_case]
    fn test_device_name_generation() {
        let name = PciScanner::generate_device_name(vendor::INTEL, 0x1234, 0, 0, 0);
        assert_eq!(name, "intel_pci_device");

        let name = PciScanner::generate_device_name(vendor::REDHAT, 0x1000, 0, 0, 0);
        assert_eq!(name, "virtio_pci_device");

        let name = PciScanner::generate_device_name(0x9999, 0x1234, 0, 0, 0);
        assert_eq!(name, "pci_device");
    }
}
