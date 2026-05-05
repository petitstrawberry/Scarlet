pub mod guest_vcpu;
pub mod mmu;
pub mod reg_index;
pub mod switch;
pub mod sysreg;
pub mod trap;
pub mod vm;

use alloc::sync::Arc;
pub use guest_vcpu::GuestVcpu;
pub use switch::{arch_guest_trap_exit, arch_run_guest_loop};
pub use trap::{arch_guest_trap_handler, clear_guest_mode, is_from_guest};
pub use vm::Vm;

use crate::hypervisor::vm::VmId;
use crate::vm::manager::VirtualMemoryManager;

pub fn create_vm(id: VmId, _owner_mm: VirtualMemoryManager) -> Result<Arc<Vm>, &'static str> {
    Ok(Arc::new(vm::Vm::new(id)))
}

pub fn arch_init_hv() {
    crate::println!("[shv] Initializing AArch64 hypervisor support (Not implemented yet)");
}

pub fn init_hv_per_cpu(cpu_id: usize) {
    // crate::println!("[shv] Initializing AArch64 CSRs for CPU {}", cpu_id);
}
