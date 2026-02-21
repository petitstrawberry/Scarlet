extern crate alloc;

pub mod memory;
pub mod syscall;
pub mod trap;
pub mod types;
pub mod vcpu;
pub mod vm;

pub use trap::{AccessType, TrapType, VmTrapInfo};
pub use types::{MmioInfo, VcpuExit, VcpuExitReason, VmExit};
pub use vcpu::VcpuObject;
pub use vm::VmObject;

use alloc::sync::Arc;

pub type VmRef = Arc<crate::arch::hv::Vm>;
pub type VcpuRef = Arc<dyn VcpuObject>;

pub fn init_hv() {
    crate::arch::hv::arch_init_hv();
}

pub fn init_hv_per_cpu(cpu_id: usize) {
    crate::arch::hv::init_hv_per_cpu(cpu_id);
}
