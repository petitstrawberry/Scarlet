//! PCI configuration space access.
//!
//! This module provides functions for reading and writing PCI configuration space
//! using ECAM (Enhanced Configuration Access Mechanism).

extern crate alloc;

use alloc::vec::Vec;

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
    /// Primary bus number for PCI-to-PCI bridge headers.
    pub const PRIMARY_BUS_NUMBER: usize = 0x18;
    /// Secondary bus number for PCI-to-PCI bridge headers.
    pub const SECONDARY_BUS_NUMBER: usize = 0x19;
    /// Subordinate bus number for PCI-to-PCI bridge headers.
    pub const SUBORDINATE_BUS_NUMBER: usize = 0x1A;
    /// Subsystem Vendor ID (16-bit)
    pub const SUBSYSTEM_VENDOR_ID: usize = 0x2C;
    /// Subsystem ID (16-bit)
    pub const SUBSYSTEM_ID: usize = 0x2E;
    /// Capabilities pointer (8-bit)
    pub const CAPABILITIES_POINTER: usize = 0x34;
    /// Interrupt Line (8-bit)
    pub const INTERRUPT_LINE: usize = 0x3C;
    /// Interrupt Pin (8-bit)
    pub const INTERRUPT_PIN: usize = 0x3D;
}

/// PCI command register bits.
pub mod command {
    /// Enable I/O space accesses.
    pub const IO_SPACE: u16 = 1 << 0;
    /// Enable memory space accesses.
    pub const MEMORY_SPACE: u16 = 1 << 1;
    /// Enable bus mastering for DMA-capable devices.
    pub const BUS_MASTER: u16 = 1 << 2;
}

/// PCI status register bits.
pub mod status {
    /// Device implements a PCI capability list.
    pub const CAPABILITIES_LIST: u16 = 1 << 4;
}

/// PCI capability IDs.
pub mod capability {
    /// Vendor-specific capability.
    pub const VENDOR_SPECIFIC: u8 = 0x09;
}

/// Number of BAR slots in a type-0 PCI header.
pub const BAR_COUNT: usize = 6;

/// Normal endpoint PCI header type.
pub const HEADER_TYPE_ENDPOINT: u8 = 0x00;

/// PCI-to-PCI bridge header type.
pub const HEADER_TYPE_BRIDGE: u8 = 0x01;

/// Decoded PCI BAR resource kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PciBarKind {
    /// I/O port BAR.
    Io,
    /// 32-bit memory BAR.
    Memory32,
    /// 64-bit memory BAR using this BAR and the following BAR slot.
    Memory64,
}

/// Decoded PCI BAR resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciBar {
    /// BAR index in the PCI header.
    pub index: u8,
    /// Raw PCI bus address programmed in the BAR.
    pub base: u64,
    /// BAR aperture size discovered with the standard all-ones probe.
    pub size: u64,
    /// Resource kind.
    pub kind: PciBarKind,
    /// True if the memory BAR is prefetchable.
    pub prefetchable: bool,
}

impl PciBar {
    /// Returns true if this BAR is an MMIO resource.
    pub const fn is_memory(&self) -> bool {
        matches!(self.kind, PciBarKind::Memory32 | PciBarKind::Memory64)
    }

    /// Returns true if this BAR is currently assigned a non-zero base.
    pub const fn is_assigned(&self) -> bool {
        self.base != 0
    }

    /// Returns true if this BAR is a 64-bit memory resource.
    pub const fn is_64bit(&self) -> bool {
        matches!(self.kind, PciBarKind::Memory64)
    }

    /// Returns true if this BAR is in I/O address space.
    pub const fn is_io(&self) -> bool {
        matches!(self.kind, PciBarKind::Io)
    }
}

/// PCI BAR resource validation issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PciBarIssue {
    /// BAR size is not a power of two.
    NonPowerOfTwoSize {
        /// BAR index.
        index: u8,
        /// Reported BAR size.
        size: u64,
    },
    /// Assigned BAR base is not aligned to the BAR size.
    MisalignedBase {
        /// BAR index.
        index: u8,
        /// Assigned BAR base.
        base: u64,
        /// Reported BAR size.
        size: u64,
    },
    /// Assigned BAR range overflows `u64`.
    AddressOverflow {
        /// BAR index.
        index: u8,
        /// Assigned BAR base.
        base: u64,
        /// Reported BAR size.
        size: u64,
    },
    /// Two assigned BAR ranges overlap in the same address space.
    Overlap {
        /// First BAR index.
        first: u8,
        /// Second BAR index.
        second: u8,
        /// Resource kind of the first BAR.
        first_kind: PciBarKind,
        /// Resource kind of the second BAR.
        second_kind: PciBarKind,
    },
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
        let phys_addr = self.config_address(addr, offset & !0x3);
        // SAFETY: ECAM config space is mapped MMIO and DWORD accesses are aligned.
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
        let phys_addr = self.config_address(addr, offset & !0x3);
        // SAFETY: ECAM config space is mapped MMIO and DWORD accesses are aligned.
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
        let shift = ((offset & 0x2) * 8) as u32;
        ((self.read_u32(addr, offset) >> shift) & 0xffff) as u16
    }

    /// Write a 16-bit value to PCI configuration space
    ///
    /// # Arguments
    ///
    /// * `addr` - PCI device address
    /// * `offset` - Offset within configuration space (must be 2-byte aligned)
    /// * `value` - Value to write
    pub fn write_u16(&self, addr: &PciAddress, offset: usize, value: u16) {
        let shift = ((offset & 0x2) * 8) as u32;
        let mask = 0xffffu32 << shift;
        let current = self.read_u32(addr, offset);
        self.write_u32(
            addr,
            offset,
            (current & !mask) | (u32::from(value) << shift),
        );
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
        let shift = ((offset & 0x3) * 8) as u32;
        ((self.read_u32(addr, offset) >> shift) & 0xff) as u8
    }

    /// Write an 8-bit value to PCI configuration space
    ///
    /// # Arguments
    ///
    /// * `addr` - PCI device address
    /// * `offset` - Offset within configuration space
    /// * `value` - Value to write
    pub fn write_u8(&self, addr: &PciAddress, offset: usize, value: u8) {
        let shift = ((offset & 0x3) * 8) as u32;
        let mask = 0xffu32 << shift;
        let current = self.read_u32(addr, offset);
        self.write_u32(
            addr,
            offset,
            (current & !mask) | (u32::from(value) << shift),
        );
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
        self.read_u32(addr, offset::REVISION_ID) >> 8
    }

    /// Read header type
    pub fn read_header_type(&self, addr: &PciAddress) -> u8 {
        self.read_u8(addr, offset::HEADER_TYPE)
    }

    fn bar_offset(index: u8) -> Option<usize> {
        if index as usize >= BAR_COUNT {
            None
        } else {
            Some(offset::BAR0 + index as usize * 4)
        }
    }

    fn bar_size32_from_mask(mask: u32) -> u64 {
        if mask == 0 {
            0
        } else {
            u64::from((!mask).wrapping_add(1))
        }
    }

    fn bar_size64_from_mask(mask: u64) -> u64 {
        if mask == 0 {
            0
        } else {
            (!mask).wrapping_add(1)
        }
    }

    fn read_bar_unchecked(&self, addr: &PciAddress, index: u8, bar_count: u8) -> Option<PciBar> {
        let bar_offset = Self::bar_offset(index)?;
        let low = self.read_u32(addr, bar_offset);

        if low & 0x1 != 0 {
            let base = u64::from(low & !0x3);
            self.write_u32(addr, bar_offset, 0xffff_ffff);
            let size_mask = self.read_u32(addr, bar_offset) & !0x3;
            self.write_u32(addr, bar_offset, low);

            let size = Self::bar_size32_from_mask(size_mask);
            if size == 0 {
                return None;
            }

            return Some(PciBar {
                index,
                base,
                size,
                kind: PciBarKind::Io,
                prefetchable: false,
            });
        }

        let bar_type = (low >> 1) & 0x3;
        let prefetchable = (low & 0x8) != 0;
        match bar_type {
            0x0 => {
                let base = u64::from(low & !0xf);
                self.write_u32(addr, bar_offset, 0xffff_ffff);
                let size_mask = self.read_u32(addr, bar_offset) & !0xf;
                self.write_u32(addr, bar_offset, low);

                let size = Self::bar_size32_from_mask(size_mask);
                if size == 0 {
                    return None;
                }

                Some(PciBar {
                    index,
                    base,
                    size,
                    kind: PciBarKind::Memory32,
                    prefetchable,
                })
            }
            0x2 if index + 1 < bar_count => {
                let high_offset = bar_offset + 4;
                let high = self.read_u32(addr, high_offset);
                let base = (u64::from(high) << 32) | u64::from(low & !0xf);

                self.write_u32(addr, bar_offset, 0xffff_ffff);
                self.write_u32(addr, high_offset, 0xffff_ffff);
                let size_low = self.read_u32(addr, bar_offset);
                let size_high = self.read_u32(addr, high_offset);
                self.write_u32(addr, bar_offset, low);
                self.write_u32(addr, high_offset, high);

                let size_mask = (u64::from(size_high) << 32) | u64::from(size_low & !0xf);
                let size = Self::bar_size64_from_mask(size_mask);
                if size == 0 {
                    return None;
                }

                Some(PciBar {
                    index,
                    base,
                    size,
                    kind: PciBarKind::Memory64,
                    prefetchable,
                })
            }
            _ => None,
        }
    }

    /// Read and validate all base address registers for a PCI function.
    ///
    /// # Arguments
    ///
    /// * `addr` - PCI function address
    ///
    /// # Returns
    ///
    /// Decoded BAR resources with size and assignment state.
    pub fn read_bars(&self, addr: &PciAddress) -> Vec<PciBar> {
        let header_type = self.read_header_type(addr) & 0x7f;
        let bar_count = match header_type {
            HEADER_TYPE_ENDPOINT => BAR_COUNT as u8,
            HEADER_TYPE_BRIDGE => 2,
            _ => return Vec::new(),
        };

        let command = self.read_u16(addr, offset::COMMAND);
        self.write_u16(
            addr,
            offset::COMMAND,
            command & !(command::IO_SPACE | command::MEMORY_SPACE),
        );

        let mut bars = Vec::new();
        let mut index = 0;
        while index < bar_count {
            if let Some(bar) = self.read_bar_unchecked(addr, index, bar_count) {
                bars.push(bar);
                index += if bar.is_64bit() { 2 } else { 1 };
            } else {
                index += 1;
            }
        }

        self.write_u16(addr, offset::COMMAND, command);
        bars
    }

    /// Validate decoded BAR resources.
    ///
    /// # Arguments
    ///
    /// * `bars` - BAR resources decoded from one PCI function
    ///
    /// # Returns
    ///
    /// A list of validation issues. An empty list means the BAR resources are
    /// internally consistent.
    pub fn validate_bars(bars: &[PciBar]) -> Vec<PciBarIssue> {
        let mut issues = Vec::new();

        for bar in bars {
            if !bar.size.is_power_of_two() {
                issues.push(PciBarIssue::NonPowerOfTwoSize {
                    index: bar.index,
                    size: bar.size,
                });
            }

            if !bar.is_assigned() {
                continue;
            }

            if bar.base.checked_add(bar.size).is_none() {
                issues.push(PciBarIssue::AddressOverflow {
                    index: bar.index,
                    base: bar.base,
                    size: bar.size,
                });
            }

            if bar.size != 0 && bar.base % bar.size != 0 {
                issues.push(PciBarIssue::MisalignedBase {
                    index: bar.index,
                    base: bar.base,
                    size: bar.size,
                });
            }
        }

        for (left_index, left) in bars.iter().enumerate() {
            if !left.is_assigned() {
                continue;
            }

            let Some(left_end) = left.base.checked_add(left.size) else {
                continue;
            };

            for right in bars.iter().skip(left_index + 1) {
                if !right.is_assigned() {
                    continue;
                }
                if left.is_memory() != right.is_memory() || left.is_io() != right.is_io() {
                    continue;
                }

                let Some(right_end) = right.base.checked_add(right.size) else {
                    continue;
                };

                if left.base < right_end && right.base < left_end {
                    issues.push(PciBarIssue::Overlap {
                        first: left.index,
                        second: right.index,
                        first_kind: left.kind,
                        second_kind: right.kind,
                    });
                }
            }
        }

        issues
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

    #[test_case]
    fn test_pci_bar_validation_accepts_separate_memory_and_io_spaces() {
        let bars = alloc::vec![
            PciBar {
                index: 0,
                base: 0x1000,
                size: 0x1000,
                kind: PciBarKind::Memory32,
                prefetchable: false,
            },
            PciBar {
                index: 1,
                base: 0x1000,
                size: 0x100,
                kind: PciBarKind::Io,
                prefetchable: false,
            },
        ];

        assert!(PciConfig::validate_bars(&bars).is_empty());
    }

    #[test_case]
    fn test_pci_bar_validation_detects_overlap() {
        let bars = alloc::vec![
            PciBar {
                index: 0,
                base: 0x1000,
                size: 0x1000,
                kind: PciBarKind::Memory32,
                prefetchable: false,
            },
            PciBar {
                index: 2,
                base: 0x1800,
                size: 0x1000,
                kind: PciBarKind::Memory64,
                prefetchable: false,
            },
        ];

        assert!(
            PciConfig::validate_bars(&bars)
                .iter()
                .any(|issue| matches!(issue, PciBarIssue::Overlap { .. }))
        );
    }

    #[test_case]
    fn test_pci_bar_validation_detects_misalignment() {
        let bars = alloc::vec![PciBar {
            index: 0,
            base: 0x1800,
            size: 0x1000,
            kind: PciBarKind::Memory32,
            prefetchable: false,
        }];

        assert!(
            PciConfig::validate_bars(&bars)
                .iter()
                .any(|issue| matches!(issue, PciBarIssue::MisalignedBase { .. }))
        );
    }
}
