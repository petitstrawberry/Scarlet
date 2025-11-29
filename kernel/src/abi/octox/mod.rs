//! Octox ABI Module
//!
//! This module provides binary compatibility with the octox OS (https://github.com/o8vm/octox).
//! Octox is a Unix-like operating system written in Rust with an xv6-compatible system call
//! interface and additional extensions.
//!
//! ## Architecture Support
//!
//! Currently supports:
//! - RISC-V 64-bit (`riscv64`)
//!

pub mod drivers;
pub mod riscv64;
