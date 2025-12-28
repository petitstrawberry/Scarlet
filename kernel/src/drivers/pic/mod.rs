//! Platform Interrupt Controller (PIC) implementations
//!
//! This module contains implementations of various interrupt controllers
//! used in different platforms and architectures.

// pub mod clint; // Currently not used
#[cfg(target_arch = "aarch64")]
pub mod arm_generic_timer;
#[cfg(target_arch = "aarch64")]
pub mod gic;
#[cfg(target_arch = "riscv64")]
pub mod plic;
#[cfg(target_arch = "riscv64")]
pub mod sbi_clint;

#[cfg(target_arch = "aarch64")]
pub use gic::Gic;
#[cfg(target_arch = "riscv64")]
pub use plic::Plic;
