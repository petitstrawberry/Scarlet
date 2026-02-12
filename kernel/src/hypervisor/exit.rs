//! VM exit reasons (architecture-independent)

/// Represents the reason a guest VM exited back to the hypervisor
#[derive(Debug, Clone)]
pub enum VmExit {
    /// Guest performed an MMIO read
    MmioRead {
        /// Guest physical address of the access
        addr: u64,
        /// Size of the access in bytes
        size: u8,
    },
    /// Guest performed an MMIO write
    MmioWrite {
        /// Guest physical address of the access
        addr: u64,
        /// Size of the access in bytes
        size: u8,
        /// Data written
        data: u64,
    },
    /// Guest halted
    Hlt,
    /// Guest requested shutdown
    Shutdown,
    /// Guest system event (e.g., ecall from VS-mode)
    SystemEvent {
        /// Event type code
        event_type: u64,
    },
    /// Failed to enter the guest
    FailEntry {
        /// Hardware-specific failure reason
        hardware_entry_failure_reason: u64,
    },
    /// Internal hypervisor error
    InternalError,
    /// Unknown exit reason
    Unknown(u64),
}
