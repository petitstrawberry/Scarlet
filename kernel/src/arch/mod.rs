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

// アーキテクチャ固有の割り込み制御機能を公開
#[cfg(target_arch = "riscv64")]
pub mod interrupt {
    pub use super::riscv64::interrupt::*;
}

