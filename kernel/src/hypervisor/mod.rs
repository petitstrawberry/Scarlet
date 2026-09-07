//! Scarlet Hypervisor (SHV) - Type-2 Virtual Machine Manager
//!
//! This module provides the kernel-side implementation of Scarlet's built-in
//! hypervisor, enabling the execution of guest operating systems with hardware
//! virtualization support.
//!
//! # Architecture
//!
//! SHV follows a **Type-2 (hosted) hypervisor** architecture:
//! - **Kernel (SHV)**: Minimal privileged operations - VM-entry, VM-exit capture,
//!   Stage-2 MMU management, timer handling
//! - **Userspace (U-SHV)**: Device emulation, guest management, I/O handling
//!
//! # Supported Architectures
//!
//! - **RISC-V 64-bit**: H-extension support (experimental, many features missing)
//! - **AArch64**: Not implemented (stub code only)
//!
//! # Key Components
//!
//! - [`memory`]: Guest memory slot management
//! - [`vm`]: Virtual machine creation and management via [`VmObject`]
//! - [`vcpu`]: Virtual CPU management via [`VcpuObject`]
//! - [`syscall`]: Hypervisor system call interface
//! - [`types`]: Shared data structures for kernel-userspace communication
//!
//! # Usage
//!
//! ```rust,ignore
//! // Create a VM
//! let vm = GLOBAL_VM_MANAGER.create_vm()?;
//!
//! // Add memory region
//! vm.set_memory_region(0, 0x80000000, 128 * 1024 * 1024, host_addr, flags)?;
//!
//! // Create vCPU
//! let vcpu = vm.create_vcpu(0)?;
//!
//! // Run vCPU (typically done in userspace VMM loop)
//! let exit = vcpu.run()?;
//! ```
//!
//! # Feature Flag
//!
//! Enable the `hypervisor` feature in `Cargo.toml` to include this module.
//!
//! # References
//!
//! - [Type-2 Hypervisor Design](../../../docs/hypervisor/type2-design.md)
//! - RISC-V H-extension specification
//! - ARMv8-A Virtualization Extensions

extern crate alloc;

pub mod memory;
pub mod mmio;
pub mod syscall;
pub mod trap;
pub mod types;
pub mod vcpu;
pub mod vm;

pub use mmio::{VirtualMmioDevice, VirtualMmioDeviceRef};
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
