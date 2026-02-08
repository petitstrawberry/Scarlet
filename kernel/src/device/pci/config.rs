//! PCI configuration space access.
//!
//! This module provides functions for reading and writing PCI configuration space
//! using ECAM (Enhanced Configuration Access Mechanism).

use super::PciAddress;

/// Standard PCI configuration space offsets
pub mod offset {
    /// Vendor ID (16-bit)
    pub const VENDOR_ID: usize = 0x00;
    /// Device ID (16-bit)
    pub const DEVICE_ID: usize = 0x02;
    /// Command register (16-bit)
    pub const COMMAND: usize = 0x04;
    /// Status register (16-bit)
    pub const STATUS: usize = 0x06;
    /// Revision ID (8-bit)
    pub const REVISION_ID: usize = 0x08;
    /// Class code (24-bit)
    pub const CLASS_CODE: usize = 0x09;
    /// Cache line size (8-bit)
    pub const CACHE_LINE_SIZE: usize = 0x0C;
    /// Latency timer (8-bit)
    pub const LATENCY_TIMER: usize = 0x0D;
    /// Header type (8-bit)
    pub const HEADER_TYPE: usize = 0x0E;
    /// BIST (8-bit)
    pub const BIST: usize = 0x0F;
    /// Base Address Register 0 (32-bit)
    pub const BAR0: usize = 0x10;
    /// Subsystem Vendor ID (16-bit)
    pub const SUBSYSTEM_VENDOR_ID: usize = 0x2C;
    /// Subsystem ID (16-bit)
    pub const SUBSYSTEM_ID: usize = 0x2E;
    /// Interrupt Line (8-bit)
    pub const INTERRUPT_LINE: usize = 0x3C;
    /// Interrupt Pin (8-bit)
    pub const INTERRUPT_PIN: usize = 0x3D;
}

/// PCI configuration space accessor
///
/// Provides safe access to PCI configuration space through ECAM mapping.
pub struct PciConfig {
    /// Base address of ECAM region
    ecam_base: usize,
}

impl PciConfig {
    /// Create a new PCI configuration accessor
    ///
    /// # Arguments
    ///
    /// * `ecam_base` - Physical base address of the ECAM region
    ///
    /// # Safety
    ///
    /// The caller must ensure that the ECAM base address is valid and mapped.
    pub const fn new(ecam_base: usize) -> Self {
        Self { ecam_base }
    }

    /// Calculate the physical address for a configuration register
    fn config_address(&self, addr: &PciAddress, offset: usize) -> usize {
        self.ecam_base + addr.ecam_offset() + offset
    }

    /// Read a 32-bit value from PCI configuration space
    ///
    /// # Arguments
    ///
    /// * `addr` - PCI device address
    /// * `offset` - Offset within configuration space (must be 4-byte aligned)
    ///
    /// # Returns
    ///
    /// The 32-bit value read from configuration space
    ///
    /// # Safety
    ///
    /// This performs a volatile MMIO read. The caller must ensure the address is valid.
    pub fn read_u32(&self, addr: &PciAddress, offset: usize) -> u32 {
        let phys_addr = self.config_address(addr, offset);
        unsafe { core::ptr::read_volatile(phys_addr as *const u32) }
    }

    /// Write a 32-bit value to PCI configuration space
    ///
    /// # Arguments
    ///
    /// * `addr` - PCI device address
    /// * `offset` - Offset within configuration space (must be 4-byte aligned)
    /// * `value` - Value to write
    ///
    /// # Safety
    ///
    /// This performs a volatile MMIO write. The caller must ensure the address is valid.
    pub fn write_u32(&self, addr: &PciAddress, offset: usize, value: u32) {
        let phys_addr = self.config_address(addr, offset);
        unsafe { core::ptr::write_volatile(phys_addr as *mut u32, value) }
    }

    /// Read a 16-bit value from PCI configuration space
    ///
    /// # Arguments
    ///
    /// * `addr` - PCI device address
    /// * `offset` - Offset within configuration space (must be 2-byte aligned)
    ///
    /// # Returns
    ///
    /// The 16-bit value read from configuration space
    pub fn read_u16(&self, addr: &PciAddress, offset: usize) -> u16 {
        let phys_addr = self.config_address(addr, offset);
        unsafe { core::ptr::read_volatile(phys_addr as *const u16) }
    }

    /// Write a 16-bit value to PCI configuration space
    ///
    /// # Arguments
    ///
    /// * `addr` - PCI device address
    /// * `offset` - Offset within configuration space (must be 2-byte aligned)
    /// * `value` - Value to write
    pub fn write_u16(&self, addr: &PciAddress, offset: usize, value: u16) {
        let phys_addr = self.config_address(addr, offset);
        unsafe { core::ptr::write_volatile(phys_addr as *mut u16, value) }
    }

    /// Read an 8-bit value from PCI configuration space
    ///
    /// # Arguments
    ///
    /// * `addr` - PCI device address
    /// * `offset` - Offset within configuration space
    ///
    /// # Returns
    ///
    /// The 8-bit value read from configuration space
    pub fn read_u8(&self, addr: &PciAddress, offset: usize) -> u8 {
        let phys_addr = self.config_address(addr, offset);
        unsafe { core::ptr::read_volatile(phys_addr as *const u8) }
    }

    /// Write an 8-bit value to PCI configuration space
    ///
    /// # Arguments
    ///
    /// * `addr` - PCI device address
    /// * `offset` - Offset within configuration space
    /// * `value` - Value to write
    pub fn write_u8(&self, addr: &PciAddress, offset: usize, value: u8) {
        let phys_addr = self.config_address(addr, offset);
        unsafe { core::ptr::write_volatile(phys_addr as *mut u8, value) }
    }

    /// Read vendor ID
    pub fn read_vendor_id(&self, addr: &PciAddress) -> u16 {
        self.read_u16(addr, offset::VENDOR_ID)
    }

    /// Read device ID
    pub fn read_device_id(&self, addr: &PciAddress) -> u16 {
        self.read_u16(addr, offset::DEVICE_ID)
    }

    /// Read class code (24-bit: base class, sub class, interface)
    pub fn read_class_code(&self, addr: &PciAddress) -> u32 {
        self.read_u32(addr, offset::CLASS_CODE) >> 8
    }

    /// Read header type
    pub fn read_header_type(&self, addr: &PciAddress) -> u8 {
        self.read_u8(addr, offset::HEADER_TYPE)
    }
}

/// PCI vendor IDs (commonly used)
pub mod vendor {
    /// Invalid vendor ID (device not present)
    pub const INVALID: u16 = 0xFFFF;
    /// Intel Corporation
    pub const INTEL: u16 = 0x8086;
    /// AMD
    pub const AMD: u16 = 0x1022;
    /// NVIDIA Corporation
    pub const NVIDIA: u16 = 0x10DE;
    /// Red Hat, Inc. (QEMU virtio devices)
    pub const REDHAT: u16 = 0x1AF4;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_pci_config_address_calculation() {
        let config = PciConfig::new(0x3000_0000);
        let addr = PciAddress::new(0, 0, 0, 0);

        // Test base address
        assert_eq!(config.config_address(&addr, 0), 0x3000_0000);

        // Test with offset
        assert_eq!(config.config_address(&addr, 0x10), 0x3000_0010);

        // Test with different bus
        let addr = PciAddress::new(0, 1, 0, 0);
        assert_eq!(config.config_address(&addr, 0), 0x3010_0000);
    }
}
