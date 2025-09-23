//! RISC-V 64-bit register module.
//!
//! This module provides the register file for the RISC-V 64-bit architecture.
//! The register file is responsible for storing the general-purpose registers
//! of the CPU.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntRegisters {
    pub reg: [usize; 32],
}

impl IntRegisters {
    pub const fn new() -> Self {
        IntRegisters { reg: [0; 32] }
    }
}