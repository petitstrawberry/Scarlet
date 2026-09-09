//! Architecture-specific code for Scarlet kernel
//!
//! This module contains architecture-specific implementations and definitions
//! for the Scarlet kernel. Each architecture has its own set of files that
//! implement the necessary functionality.
//!

#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
pub mod riscv;
#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
pub use riscv::*;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64::*;
