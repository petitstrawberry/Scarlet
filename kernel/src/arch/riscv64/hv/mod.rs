//! RISC-V H-extension hypervisor support

pub mod csr;
pub mod guest_vcpu;
pub mod mmu;
pub mod reg_index;
pub mod switch;
pub mod trap;
pub mod vm;

use alloc::sync::Arc;
pub use guest_vcpu::GuestVcpu;
pub use switch::{arch_guest_trap_exit, arch_run_guest_loop};
pub use trap::{arch_guest_trap_handler, clear_guest_mode, is_from_guest};
pub use vm::{Riscv64VmObject, RiscvVmState, Vm};

use crate::hypervisor::vm::VmId;

pub fn create_vm(id: VmId) -> Result<Arc<Vm>, &'static str> {
    Ok(Arc::new(Riscv64VmObject::new(id)?))
}
