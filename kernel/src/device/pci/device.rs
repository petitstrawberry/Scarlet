//! PCI device information.
//!
//! This module defines the PCI device information structure that represents
//! a PCI device discovered on the system.

extern crate alloc;

use alloc::vec::Vec;
use core::any::Any;

use super::PciAddress;
use crate::device::{DeviceInfo, DeviceType};

/// PCI device class codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PciClass {
    /// Unclassified device
    Unclassified = 0x00,
    /// Mass storage controller
    MassStorage = 0x01,
    /// Network controller
    Network = 0x02,
    /// Display controller
    Display = 0x03,
    /// Multimedia controller
    Multimedia = 0x04,
    /// Memory controller
    Memory = 0x05,
    /// Bridge device
    Bridge = 0x06,
    /// Simple communication controller
    Communication = 0x07,
    /// Base system peripheral
    SystemPeripheral = 0x08,
    /// Input device controller
    Input = 0x09,
    /// Docking station
    DockingStation = 0x0A,
    /// Processor
    Processor = 0x0B,
    /// Serial bus controller
    SerialBus = 0x0C,
    /// Wireless controller
    Wireless = 0x0D,
    /// Intelligent controller
    IntelligentIO = 0x0E,
    /// Satellite communication controller
    Satellite = 0x0F,
    /// Encryption controller
    Encryption = 0x10,
    /// Signal processing controller
    SignalProcessing = 0x11,
    /// Processing accelerator
    Accelerator = 0x12,
    /// Non-essential instrumentation
    Instrumentation = 0x13,
    /// Unknown class
    Unknown = 0xFF,
}

impl From<u8> for PciClass {
    fn from(value: u8) -> Self {
        match value {
            0x00 => PciClass::Unclassified,
            0x01 => PciClass::MassStorage,
            0x02 => PciClass::Network,
            0x03 => PciClass::Display,
            0x04 => PciClass::Multimedia,
            0x05 => PciClass::Memory,
            0x06 => PciClass::Bridge,
            0x07 => PciClass::Communication,
            0x08 => PciClass::SystemPeripheral,
            0x09 => PciClass::Input,
            0x0A => PciClass::DockingStation,
            0x0B => PciClass::Processor,
            0x0C => PciClass::SerialBus,
            0x0D => PciClass::Wireless,
            0x0E => PciClass::IntelligentIO,
            0x0F => PciClass::Satellite,
            0x10 => PciClass::Encryption,
            0x11 => PciClass::SignalProcessing,
            0x12 => PciClass::Accelerator,
            0x13 => PciClass::Instrumentation,
            _ => PciClass::Unknown,
        }
    }
}

/// PCI device information
///
/// Contains all relevant information about a discovered PCI device.
#[derive(Debug, Clone)]
pub struct PciDeviceInfo {
    /// PCI address (bus, device, function)
    address: PciAddress,
    /// Vendor ID
    vendor_id: u16,
    /// Device ID
    device_id: u16,
    /// Class code (base class, subclass, interface)
    class_code: u32,
    /// Revision ID
    revision: u8,
    /// Subsystem vendor ID
    subsystem_vendor_id: u16,
    /// Subsystem ID
    subsystem_id: u16,
    /// Interrupt line
    interrupt_line: u8,
    /// Interrupt pin
    interrupt_pin: u8,
    routed_irq: Option<u32>,
    /// Device name (generated from vendor/device ID)
    name: &'static str,
    /// Unique device ID in the system
    id: usize,
}

impl PciDeviceInfo {
    /// Create a new PCI device information structure
    ///
    /// # Arguments
    ///
    /// * `address` - PCI address
    /// * `vendor_id` - Vendor ID
    /// * `device_id` - Device ID
    /// * `class_code` - Class code
    /// * `revision` - Revision ID
    /// * `subsystem_vendor_id` - Subsystem vendor ID
    /// * `subsystem_id` - Subsystem ID
    /// * `interrupt_line` - Interrupt line
    /// * `interrupt_pin` - Interrupt pin
    /// * `name` - Device name
    /// * `id` - Unique device ID
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        address: PciAddress,
        vendor_id: u16,
        device_id: u16,
        class_code: u32,
        revision: u8,
        subsystem_vendor_id: u16,
        subsystem_id: u16,
        interrupt_line: u8,
        interrupt_pin: u8,
        routed_irq: Option<u32>,
        name: &'static str,
        id: usize,
    ) -> Self {
        Self {
            address,
            vendor_id,
            device_id,
            class_code,
            revision,
            subsystem_vendor_id,
            subsystem_id,
            interrupt_line,
            interrupt_pin,
            routed_irq,
            name,
            id,
        }
    }

    /// Get the PCI address
    pub fn address(&self) -> PciAddress {
        self.address
    }

    /// Get the vendor ID
    pub fn vendor_id(&self) -> u16 {
        self.vendor_id
    }

    /// Get the device ID
    pub fn device_id(&self) -> u16 {
        self.device_id
    }

    /// Get the class code
    pub fn class_code(&self) -> u32 {
        self.class_code
    }

    /// Get the base class
    pub fn base_class(&self) -> u8 {
        ((self.class_code >> 16) & 0xFF) as u8
    }

    /// Get the subclass
    pub fn subclass(&self) -> u8 {
        ((self.class_code >> 8) & 0xFF) as u8
    }

    /// Get the interface
    pub fn interface(&self) -> u8 {
        (self.class_code & 0xFF) as u8
    }

    /// Get the PCI class
    pub fn pci_class(&self) -> PciClass {
        PciClass::from(self.base_class())
    }

    /// Get the revision ID
    pub fn revision(&self) -> u8 {
        self.revision
    }

    /// Get the subsystem vendor ID
    pub fn subsystem_vendor_id(&self) -> u16 {
        self.subsystem_vendor_id
    }

    /// Get the subsystem ID
    pub fn subsystem_id(&self) -> u16 {
        self.subsystem_id
    }

    /// Get the interrupt line
    pub fn interrupt_line(&self) -> u8 {
        self.interrupt_line
    }

    /// Get the interrupt pin
    pub fn interrupt_pin(&self) -> u8 {
        self.interrupt_pin
    }

    pub fn routed_irq(&self) -> Option<u32> {
        self.routed_irq
    }

    /// Check if device matches vendor and device ID
    pub fn matches(&self, vendor_id: u16, device_id: u16) -> bool {
        self.vendor_id == vendor_id && self.device_id == device_id
    }

    /// Check if device matches class code
    pub fn matches_class(&self, base_class: u8, subclass: Option<u8>) -> bool {
        if self.base_class() != base_class {
            return false;
        }
        if let Some(sc) = subclass {
            return self.subclass() == sc;
        }
        true
    }

    /// Convert to DeviceType based on PCI class
    pub fn to_device_type(&self) -> DeviceType {
        match self.pci_class() {
            PciClass::MassStorage => DeviceType::Block,
            PciClass::Network => DeviceType::Network,
            PciClass::Display => DeviceType::Graphics,
            _ => DeviceType::Generic,
        }
    }

    /// Get the device name
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Get the device ID
    pub fn id(&self) -> usize {
        self.id
    }
}

impl DeviceInfo for PciDeviceInfo {
    fn name(&self) -> &'static str {
        self.name
    }

    fn id(&self) -> usize {
        self.id
    }

    fn compatible(&self) -> Vec<&'static str> {
        // PCI devices use vendor:device ID matching rather than string-based
        // compatibility matching. The PciDeviceDriver uses its own matches_device()
        // method with PciDeviceId structures for matching, so this returns an empty
        // vector. This is intentional and does not break driver matching for PCI devices.
        Vec::new()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_pci_device_info_creation() {
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

        assert_eq!(device.vendor_id(), 0x8086);
        assert_eq!(device.device_id(), 0x1234);
        assert_eq!(device.base_class(), 0x02);
        assert_eq!(device.pci_class(), PciClass::Network);
        assert_eq!(device.to_device_type(), DeviceType::Network);
    }

    #[test_case]
    fn test_pci_device_matching() {
        let addr = PciAddress::new(0, 0, 1, 0);
        let device = PciDeviceInfo::new(
            addr,
            0x8086,
            0x1234,
            0x030000, // Display controller
            0x01,
            0x0000,
            0x0000,
            0x0B,
            0x01,
            "pci_device",
            1,
        );

        assert!(device.matches(0x8086, 0x1234));
        assert!(!device.matches(0x8086, 0x5678));
        assert!(device.matches_class(0x03, None));
        assert!(device.matches_class(0x03, Some(0x00)));
        assert!(!device.matches_class(0x02, None));
    }

    #[test_case]
    fn test_pci_class_conversion() {
        assert_eq!(PciClass::from(0x01), PciClass::MassStorage);
        assert_eq!(PciClass::from(0x02), PciClass::Network);
        assert_eq!(PciClass::from(0x03), PciClass::Display);
        assert_eq!(PciClass::from(0xFF), PciClass::Unknown);
    }
}
