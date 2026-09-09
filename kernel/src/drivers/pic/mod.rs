//! Platform Interrupt Controller (PIC) implementations
//!
//! This module contains implementations of various interrupt controllers
//! used in different platforms and architectures.

// pub mod clint; // Currently not used
#[cfg(target_arch = "aarch64")]
pub mod arm_generic_timer;
#[cfg(target_arch = "aarch64")]
pub mod gic;
#[cfg(target_arch = "aarch64")]
pub mod gicv3;
#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
pub mod plic;
#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
pub mod sbi_ipi;
#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
pub mod sbi_timer;
#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
pub mod sstc;

#[cfg(target_arch = "aarch64")]
pub use gic::Gic;
#[cfg(target_arch = "aarch64")]
pub use gicv3::GicV3;
#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
pub use plic::Plic;
