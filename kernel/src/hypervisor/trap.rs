//! Arch-independent VM trap information

/// Type of guest trap
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapType {
    /// Page fault (second stage translation failure)
    PageFault,
    /// Firmware call (e.g., SBI on RISC-V, PSCI on AArch64)
    FirmwareCall,
    /// WFI or similar halt instruction
    Halt,
    /// Timer interrupt
    TimerInterrupt,
    /// External interrupt
    ExternalInterrupt,
    /// Unknown trap
    Unknown,
}

/// Memory access type for page faults
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    Read,
    Write,
    Execute,
}

/// Arch-independent trait for VM trap information
///
/// Each architecture implements this to capture and expose
/// hardware-specific trap details in a uniform way.
pub trait VmTrapInfo: Clone {
    /// Capture trap information from hardware CSRs/registers
    fn capture() -> Self;

    /// Get the type of this trap
    fn trap_type(&self) -> TrapType;

    /// Get the guest physical address (for page faults)
    fn gpa(&self) -> u64;

    /// Get access type (for page faults)
    fn access_type(&self) -> AccessType;

    /// Get access size in bytes (for MMIO)
    fn access_size(&self) -> u8;

    /// Get raw trap cause code (arch-specific)
    fn raw_cause(&self) -> u64;

    /// Check if this is an interrupt (vs exception)
    fn is_interrupt(&self) -> bool;
}
