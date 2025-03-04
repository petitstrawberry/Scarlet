//! RISC-V 64-bit register module.
//!
//! This module provides the register file for the RISC-V 64-bit architecture.
//! The register file is responsible for storing the general-purpose registers
//! of the CPU.

#[derive(Debug, Clone, Copy)]
pub struct Registers {
    pub reg: [usize; 32],
}

impl Registers {
    pub fn new() -> Self {
        Registers { reg: [0; 32] }
    }
}