//! Scarlet Native ABI (arch-specific)
//!
//! This module is split per-architecture and selected via `cfg(target_arch)`.

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
pub mod riscv;
#[cfg(target_arch = "riscv64")]
pub use riscv as riscv64;

#[cfg(target_arch = "aarch64")]
pub use aarch64::ScarletAbi;
#[cfg(any(target_arch = "riscv32", target_arch = "riscv64"))]
pub use riscv::ScarletAbi;
