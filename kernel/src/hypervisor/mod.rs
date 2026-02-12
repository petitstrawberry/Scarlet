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
pub use vcpu::VcpuObject;
pub use vm::VmObject;

use alloc::sync::Arc;

/// Type alias for a shared reference to a VM (internal mutability)
pub type VmRef = Arc<VmObject>;

/// Type alias for a shared reference to a vCPU (internal mutability)
pub type VcpuRef = Arc<VcpuObject>;
