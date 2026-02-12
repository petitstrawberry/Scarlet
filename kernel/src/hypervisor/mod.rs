//! Hypervisor subsystem
//!
//! This module provides a KVM-like in-kernel hypervisor facility.
//! It is consumed by ABI modules (Linux ABI for /dev/kvm emulation,
//! Scarlet Native via ControlOps) to provide hardware-assisted
//! virtualization to userspace VMMs.

extern crate alloc;

pub mod error;
pub mod exit;
pub mod memory;
pub mod vcpu;
pub mod vm;

pub use error::HypervisorError;
pub use exit::VmExit;
pub use memory::{MemorySlot, MemorySlotFlags, MemorySlotManager};
pub use vcpu::Vcpu;
pub use vm::Vm;

use alloc::sync::Arc;
use spin::Mutex;

/// Type alias for a shared reference to a VM
pub type VmRef = Arc<Mutex<Vm>>;

/// Type alias for a shared reference to a vCPU
pub type VcpuRef = Arc<Mutex<Vcpu>>;
