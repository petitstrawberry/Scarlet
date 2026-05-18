//! In-kernel MMIO device emulation framework
//!
//! Provides a trait for emulating MMIO devices that are registered with a VM.
//! When a guest accesses a registered address range, the trap handler dispatches
//! to the device's read/write methods instead of exiting to userspace.
//!
//! This is architecture-agnostic — arch-specific device implementations (e.g.
//! VgicDist, VgicRedist) implement this trait and live in `arch/<arch>/hv/`.

extern crate alloc;

use alloc::sync::Arc;

/// Trait for in-kernel MMIO device emulation.
///
/// Implementors handle guest MMIO accesses within a specific address range.
/// The trap handler calls these methods when a data abort falls within the
/// device's registered range.
pub trait VirtualMmioDevice: Send + Sync {
    /// Handle a read from the given offset within this device's address range.
    /// Returns the value to supply to the guest.
    fn read(&self, offset: u64, size: u8) -> u64;

    /// Handle a write to the given offset within this device's address range.
    fn write(&self, offset: u64, size: u8, value: u64);

    /// Returns the (base, size) of this device's address range in GPA space.
    fn addr_range(&self) -> (u64, u64);

    /// Convenience: check if an IPA falls within this device's range.
    fn handles(&self, ipa: u64) -> bool {
        let (base, size) = self.addr_range();
        ipa >= base && ipa < base + size
    }
}

/// Type alias for a shared reference to a VirtualMmioDevice.
pub type VirtualMmioDeviceRef = Arc<dyn VirtualMmioDevice>;
