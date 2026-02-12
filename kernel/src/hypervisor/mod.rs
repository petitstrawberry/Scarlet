//! Hypervisor subsystem
//!
//! This module provides a KVM-like in-kernel hypervisor facility.
//! It is consumed by ABI modules (Linux ABI for /dev/kvm emulation,
//! Scarlet Native via ControlOps) to provide hardware-assisted
//! virtualization to userspace VMMs.

#[cfg(feature = "hypervisor")]
extern crate alloc;

#[cfg(feature = "hypervisor")]
pub mod error;
#[cfg(feature = "hypervisor")]
pub mod exit;
#[cfg(feature = "hypervisor")]
pub mod memory;
pub mod syscall;
#[cfg(feature = "hypervisor")]
pub mod vcpu;
#[cfg(feature = "hypervisor")]
pub mod vm;

#[cfg(feature = "hypervisor")]
pub use error::HypervisorError;
#[cfg(feature = "hypervisor")]
pub use exit::VmExit;
#[cfg(feature = "hypervisor")]
pub use memory::{MemorySlot, MemorySlotFlags, MemorySlotManager};
#[cfg(feature = "hypervisor")]
pub use vcpu::VcpuObject;
#[cfg(feature = "hypervisor")]
pub use vm::VmObject;

#[cfg(feature = "hypervisor")]
use alloc::sync::Arc;

/// Type alias for a shared reference to a VM (internal mutability)
#[cfg(feature = "hypervisor")]
pub type VmRef = Arc<VmObject>;

/// Type alias for a shared reference to a vCPU (internal mutability)
#[cfg(feature = "hypervisor")]
pub type VcpuRef = Arc<VcpuObject>;
