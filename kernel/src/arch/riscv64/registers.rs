//! RISC-V 64-bit register module.
//!
//! This module provides the register file for the RISC-V 64-bit architecture.
//! The register file is responsible for storing the general-purpose registers
//! of the CPU.

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntRegisters {
    pub reg: [usize; 32],
}

impl IntRegisters {
    pub const fn new() -> Self {
        IntRegisters { reg: [0; 32] }
    }

    pub fn get_return_value(&self) -> usize {
        // RISC-V syscall return value: a0 (x10)
        self.reg[10]
    }

    pub fn set_return_value(&mut self, value: usize) {
        // RISC-V syscall return value: a0 (x10)
        self.reg[10] = value;
    }

    pub fn set_arg(&mut self, index: usize, value: usize) {
        // RISC-V syscall arguments: a0-a7 (x10-x17)
        if index < 8 {
            self.reg[index + 10] = value;
        }
    }

    pub fn get_arg(&self, index: usize) -> usize {
        // RISC-V syscall arguments: a0-a7 (x10-x17)
        if index < 8 { self.reg[index + 10] } else { 0 }
    }

    /// Get thread pointer (tp/x4) for TLS
    pub fn get_tp(&self) -> usize {
        self.reg[4]
    }

    /// Set thread pointer (tp/x4) for TLS
    pub fn set_tp(&mut self, value: usize) {
        self.reg[4] = value;
    }
}
