//! Architecture-specific code for Scarlet kernel
//!
//! This module contains architecture-specific implementations and definitions
//! for the Scarlet kernel. Each architecture has its own set of files that
//! implement the necessary functionality.
//!

#[cfg(target_arch = "riscv64")]
pub mod riscv64;
#[cfg(target_arch = "riscv64")]
pub use riscv64::*;

// Re-export kernel context for architecture-independent use
#[cfg(target_arch = "riscv64")]
pub use riscv64::context::KernelContext;
