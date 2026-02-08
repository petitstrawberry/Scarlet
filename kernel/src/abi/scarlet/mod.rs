//! Scarlet Native ABI (arch-specific)
//!
//! This module is split per-architecture and selected via `cfg(target_arch)`.

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(target_arch = "riscv64")]
pub mod riscv64;

#[cfg(target_arch = "aarch64")]
pub use aarch64::ScarletAbi;
#[cfg(target_arch = "riscv64")]
pub use riscv64::ScarletAbi;
