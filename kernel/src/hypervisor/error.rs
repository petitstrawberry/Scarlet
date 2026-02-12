//! Hypervisor error types

/// Errors that can occur during hypervisor operations
#[derive(Debug)]
pub enum HypervisorError {
    /// Hypervisor not supported on this architecture
    NotSupported,
    /// Invalid VM identifier
    InvalidVmId,
    /// Invalid vCPU identifier
    InvalidVcpuId,
    /// Maximum number of vCPUs reached for this VM
    MaxVcpusReached,
    /// Memory slot overlap with existing mapping
    MemorySlotOverlap,
    /// Memory slot not found
    MemorySlotNotFound,
    /// Invalid memory region parameters
    InvalidMemoryRegion,
    /// Architecture-specific error
    ArchError(&'static str),
}

impl core::fmt::Display for HypervisorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HypervisorError::NotSupported => write!(f, "Hypervisor not supported"),
            HypervisorError::InvalidVmId => write!(f, "Invalid VM ID"),
            HypervisorError::InvalidVcpuId => write!(f, "Invalid vCPU ID"),
            HypervisorError::MaxVcpusReached => write!(f, "Maximum vCPUs reached"),
            HypervisorError::MemorySlotOverlap => write!(f, "Memory slot overlap"),
            HypervisorError::MemorySlotNotFound => write!(f, "Memory slot not found"),
            HypervisorError::InvalidMemoryRegion => write!(f, "Invalid memory region"),
            HypervisorError::ArchError(msg) => write!(f, "Architecture error: {}", msg),
        }
    }
}
