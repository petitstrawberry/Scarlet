//! AArch64 register module.
//!
//! This module provides the register file for the AArch64 architecture.
//! The register file is responsible for storing the general-purpose registers
//! of the CPU.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntRegisters {
    /// General-purpose registers X0-X30 + SP (32 total to match RISC-V structure)
    pub reg: [usize; 32],
}

impl IntRegisters {
    pub const fn new() -> Self {
        IntRegisters { 
            reg: [0; 32],
        }
    }
}