//! Platform Interrupt Controller (PIC) implementations
//!
//! This module contains implementations of various interrupt controllers
//! used in different platforms and architectures.

// pub mod clint; // Currently not used
pub mod plic;
#[cfg(target_arch = "riscv64")]
pub mod sbi_clint;
