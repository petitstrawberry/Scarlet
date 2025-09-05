//! AArch64 register module.
//!
//! This module provides the register file for the AArch64 architecture.
//! The register file is responsible for storing the general-purpose registers
//! of the CPU.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Registers {
    /// General-purpose registers X0-X30
    pub reg: [usize; 31],
    /// Stack pointer (SP)
    pub sp: usize,
}

impl Registers {
    pub const fn new() -> Self {
        Registers { 
            reg: [0; 31],
            sp: 0,
        }
    }
}