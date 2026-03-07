//! AArch64 register module.
//!
//! This module provides the register file for the AArch64 architecture.
//! The register file is responsible for storing the general-purpose registers
//! of the CPU.

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntRegisters {
    /// General-purpose registers X0-X30
    pub reg: [usize; 31],
}

impl IntRegisters {
    pub const fn new() -> Self {
        IntRegisters { reg: [0; 31] }
    }

    pub fn get_return_value(&self) -> usize {
        // AArch64 syscall return value: X0
        self.reg[0]
    }

    pub fn set_return_value(&mut self, value: usize) {
        // AArch64 syscall return value: X0
        self.reg[0] = value;
    }

    pub fn set_arg(&mut self, index: usize, value: usize) {
        // AArch64 syscall arguments: X0-X7
        if index < 8 {
            self.reg[index] = value;
        }
    }

    pub fn get_arg(&self, index: usize) -> usize {
        // AArch64 syscall arguments: X0-X7
        if index < 8 { self.reg[index] } else { 0 }
    }
}
