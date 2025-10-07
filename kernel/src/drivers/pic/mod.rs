//! Platform Interrupt Controller (PIC) implementations
//!
//! This module contains implementations of various interrupt controllers
//! used in different platforms and architectures.

pub mod plic;
#[cfg(target_arch = "riscv64")]
pub mod clint;
#[cfg(target_arch = "aarch64")]
pub mod gic;

pub use plic::Plic;
#[cfg(target_arch = "riscv64")]
pub use clint::Clint;
#[cfg(target_arch = "aarch64")]
pub use gic::Gic;
