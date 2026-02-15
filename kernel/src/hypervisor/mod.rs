//! Hypervisor subsystem (Type-2 architecture)

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

pub type VmRef = Arc<VmObject>;
pub type VcpuRef = Arc<VcpuObject>;
