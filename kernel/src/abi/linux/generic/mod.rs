//! Generic Linux ABI implementation (architecture-independent components)
//!
//! This module contains syscall implementations that are common across
//! all architectures (RISC-V64, AArch64, etc.). Architecture-specific
//! modules (linux::riscv64, linux::aarch64) re-export these and add
//! their own syscall tables with architecture-specific configurations.

pub mod errno;
pub mod fs;
pub mod futex;
pub mod mm;
pub mod pipe;
pub mod proc;
pub mod signal;
pub mod socket;
pub mod time;
