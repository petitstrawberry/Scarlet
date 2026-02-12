//! AArch64 hypervisor support (stub)
//!
//! AArch64 hardware-assisted virtualization is not yet implemented.
//! All operations return an error indicating lack of support.

use crate::hypervisor::exit::VmExit;
use crate::hypervisor::memory::MemorySlotFlags;

/// Architecture-specific VM state for AArch64 (stub)
pub struct ArchVm {}

impl ArchVm {
    pub fn new() -> Result<Self, &'static str> {
        Err("Hypervisor not supported on AArch64")
    }

    pub fn map_memory(
        &mut self,
        _guest_phys_addr: u64,
        _host_phys_addr: u64,
        _size: u64,
        _flags: MemorySlotFlags,
    ) -> Result<(), &'static str> {
        Err("Hypervisor not supported on AArch64")
    }

    pub fn unmap_memory(&mut self, _guest_phys_addr: u64, _size: u64) -> Result<(), &'static str> {
        Err("Hypervisor not supported on AArch64")
    }
}

/// Guest general-purpose registers
#[derive(Debug, Clone)]
pub struct GuestRegisters {
    pub regs: [u64; 32],
}

impl GuestRegisters {
    pub fn new() -> Self {
        Self { regs: [0; 32] }
    }

    /// Get a general-purpose register by index (0..31)
    pub fn get_gpr(&self, index: usize) -> u64 {
        self.regs[index]
    }

    /// Set a general-purpose register by index (0..31)
    pub fn set_gpr(&mut self, index: usize, value: u64) {
        self.regs[index] = value;
    }
}

impl Default for GuestRegisters {
    fn default() -> Self {
        Self::new()
    }
}

/// Architecture-specific vCPU state for AArch64 (stub)
pub struct ArchVcpu {
    guest_regs: GuestRegisters,
    guest_pc: u64,
}

impl ArchVcpu {
    pub fn new(_vm: &ArchVm) -> Result<Self, &'static str> {
        Err("Hypervisor not supported on AArch64")
    }

    pub fn run(&mut self) -> Result<VmExit, &'static str> {
        Err("Hypervisor not supported on AArch64")
    }

    pub fn get_regs(&self) -> GuestRegisters {
        self.guest_regs.clone()
    }

    pub fn set_regs(&mut self, regs: &GuestRegisters) {
        self.guest_regs = regs.clone();
    }

    pub fn get_pc(&self) -> u64 {
        self.guest_pc
    }

    pub fn set_pc(&mut self, pc: u64) {
        self.guest_pc = pc;
    }

    /// Get a general-purpose register by index (0..31)
    pub fn get_gpr(&self, index: usize) -> u64 {
        self.guest_regs.get_gpr(index)
    }

    /// Set a general-purpose register by index (0..31)
    pub fn set_gpr(&mut self, index: usize, value: u64) {
        self.guest_regs.set_gpr(index, value);
    }
}
