//! AArch64 virtual CPU support
//!
//! Virtual CPU functionality for AArch64 architecture.

use super::{Registers, Aarch64};

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
        // SP is X31 in AArch64
        self.regs.sp = sp;
    }

    pub fn get_mode(&self) -> Mode {
        self.mode
    }

    pub fn store(&mut self, aarch64: &Aarch64) {
        self.regs = aarch64.regs.clone();
        self.pc = aarch64.elr;
    }

    pub fn switch(&mut self, aarch64: &mut Aarch64) {
        aarch64.regs = self.regs.clone();
        aarch64.elr = self.pc;
    }
}

pub fn vcpu_init() {
    // TODO: Initialize AArch64 vCPU
}