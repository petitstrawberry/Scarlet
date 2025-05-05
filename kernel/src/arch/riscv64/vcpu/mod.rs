//! VCPU module for RISC-V 64-bit architecture.
//! 
//! This module provides the virtual CPU (VCPU) abstraction for the RISC-V 64-bit
//! architecture. The VCPU is responsible for executing instructions and managing
//! the state of the CPU.

use super::{Registers, Riscv64};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    User,
    Kernel,
}

#[derive(Debug, Clone)]
pub struct Vcpu {
    pub regs: Registers,
    pc: u64,
    asid: usize,
    mode: Mode,
}

impl Vcpu {
    pub fn new(mode: Mode) -> Self {
        Vcpu {
            regs: Registers::new(),
            pc: 0,
            asid: 0,
            mode,
        }
    }

    pub fn set_asid(&mut self, asid: usize) {
        self.asid = asid;
    }

    pub fn set_pc(&mut self, pc: u64) {
        self.pc = pc;
    }

    pub fn get_pc(&self) -> u64 {
        self.pc
    }

    pub fn set_sp(&mut self, sp: usize) {
        self.regs.reg[2] = sp;
    }

    pub fn get_mode(&self) -> Mode {
        self.mode
    }

    pub fn store(&mut self, riscv64: &Riscv64) {
        self.regs = riscv64.regs;
        self.pc = riscv64.epc;
    }

    pub fn switch(&mut self, riscv64: &mut Riscv64) {
        riscv64.regs = self.regs;
        riscv64.epc = self.pc;
    }
}
