//! RISC-V H-extension hypervisor support
//!
//! This module provides hardware-assisted virtualization using the
//! RISC-V Hypervisor extension. The kernel runs in HS-mode
//! and manages guests in VS/VU-mode.

use crate::hypervisor::exit::VmExit;
use crate::hypervisor::memory::MemorySlotFlags;

/// Architecture-specific VM state for RISC-V
pub struct ArchVm {
    // G-stage page table root (Sv48x4) — to be implemented
}

impl ArchVm {
    /// Create a new architecture-specific VM context
    pub fn new() -> Result<Self, &'static str> {
        Ok(Self {})
    }

    /// Map a guest physical address region in the G-stage page table
    pub fn map_memory(
        &mut self,
        _guest_phys_addr: u64,
        _host_phys_addr: u64,
        _size: u64,
        _flags: MemorySlotFlags,
    ) -> Result<(), &'static str> {
        Ok(())
    }

    /// Unmap a guest physical address region from the G-stage page table
    pub fn unmap_memory(&mut self, _guest_phys_addr: u64, _size: u64) -> Result<(), &'static str> {
        Ok(())
    }
}

/// Guest general-purpose registers
#[derive(Debug, Clone)]
pub struct GuestRegisters {
    /// x0-x31
    pub regs: [u64; 32],
}

impl GuestRegisters {
    pub fn new() -> Self {
        Self { regs: [0; 32] }
    }
}

impl Default for GuestRegisters {
    fn default() -> Self {
        Self::new()
    }
}

/// Architecture-specific vCPU state for RISC-V
pub struct ArchVcpu {
    guest_regs: GuestRegisters,
    guest_pc: u64,
}

impl ArchVcpu {
    /// Create a new architecture-specific vCPU context
    pub fn new() -> Result<Self, &'static str> {
        Ok(Self {
            guest_regs: GuestRegisters::new(),
            guest_pc: 0,
        })
    }

    /// Enter guest execution and return on VM exit
    pub fn run(&mut self) -> Result<VmExit, &'static str> {
        Err("Guest execution not yet implemented")
    }

    /// Get guest general-purpose registers
    pub fn get_regs(&self) -> GuestRegisters {
        self.guest_regs.clone()
    }

    /// Set guest general-purpose registers
    pub fn set_regs(&mut self, regs: &GuestRegisters) {
        self.guest_regs = regs.clone();
    }

    /// Get the guest program counter
    pub fn get_pc(&self) -> u64 {
        self.guest_pc
    }

    /// Set the guest program counter
    pub fn set_pc(&mut self, pc: u64) {
        self.guest_pc = pc;
    }
}
