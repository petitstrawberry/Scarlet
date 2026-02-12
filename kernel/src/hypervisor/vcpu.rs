//! Virtual CPU management

use crate::arch::hv::{ArchVcpu, ArchVm, GuestRegisters};
use crate::hypervisor::exit::VmExit;

use alloc::sync::Weak;
use spin::Mutex;

pub type VcpuId = u32;

struct VcpuState {
    arch: ArchVcpu,
}

/// Virtual CPU with internal mutability.
///
/// Mutable arch state is behind a `Mutex<VcpuState>`.
pub struct VcpuObject {
    id: VcpuId,
    state: Mutex<VcpuState>,
    _vm: Weak<super::vm::VmObject>,
}

impl VcpuObject {
    pub(crate) fn new(
        id: VcpuId,
        vm: Weak<super::vm::VmObject>,
        arch_vm: &ArchVm,
    ) -> Result<Self, &'static str> {
        let arch = ArchVcpu::new(arch_vm)?;
        Ok(Self {
            id,
            state: Mutex::new(VcpuState { arch }),
            _vm: vm,
        })
    }

    pub fn id(&self) -> VcpuId {
        self.id
    }

    pub fn run(&self) -> Result<VmExit, &'static str> {
        self.state.lock().arch.run()
    }

    pub fn get_regs(&self) -> GuestRegisters {
        self.state.lock().arch.get_regs()
    }

    pub fn set_regs(&self, regs: &GuestRegisters) {
        self.state.lock().arch.set_regs(regs);
    }

    pub fn get_pc(&self) -> u64 {
        self.state.lock().arch.get_pc()
    }

    pub fn set_pc(&self, pc: u64) {
        self.state.lock().arch.set_pc(pc);
    }
}
