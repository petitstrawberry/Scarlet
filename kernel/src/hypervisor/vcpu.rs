//! Virtual CPU management

use crate::arch::hv::{ArchVcpu, GuestRegisters};
use crate::hypervisor::exit::VmExit;

use alloc::sync::Weak;
use spin::Mutex;

/// Virtual CPU identifier
pub type VcpuId = u32;

/// Represents a virtual CPU within a VM
pub struct Vcpu {
    /// vCPU identifier within its parent VM
    id: VcpuId,
    /// Architecture-specific vCPU state
    arch: ArchVcpu,
    /// Back-reference to the parent VM (weak to avoid cycles)
    _vm: Weak<Mutex<super::vm::Vm>>,
}

impl Vcpu {
    /// Create a new vCPU with the given ID
    pub(crate) fn new(id: VcpuId, vm: Weak<Mutex<super::vm::Vm>>) -> Result<Self, &'static str> {
        let arch = ArchVcpu::new()?;
        Ok(Self { id, arch, _vm: vm })
    }

    /// Get the vCPU ID
    pub fn id(&self) -> VcpuId {
        self.id
    }

    /// Run the vCPU, entering guest execution
    ///
    /// Returns the reason the guest exited
    pub fn run(&mut self) -> Result<VmExit, &'static str> {
        self.arch.run()
    }

    /// Get guest general-purpose registers
    pub fn get_regs(&self) -> GuestRegisters {
        self.arch.get_regs()
    }

    /// Set guest general-purpose registers
    pub fn set_regs(&mut self, regs: &GuestRegisters) {
        self.arch.set_regs(regs);
    }

    /// Get the guest program counter
    pub fn get_pc(&self) -> u64 {
        self.arch.get_pc()
    }

    /// Set the guest program counter
    pub fn set_pc(&mut self, pc: u64) {
        self.arch.set_pc(pc);
    }
}
