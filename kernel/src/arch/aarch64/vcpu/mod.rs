//! AArch64 virtual CPU support
//!
//! Virtual CPU functionality for AArch64 architecture.

use crate::arch::Trapframe;

use super::IntRegisters;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    User,
    Kernel,
}

#[derive(Debug, Clone)]
pub struct Vcpu {
    pub iregs: IntRegisters,
    pub sp: u64,
    pc: u64,
    asid: usize,
    mode: Mode,
}

impl Vcpu {
    pub fn new(mode: Mode) -> Self {
        let initial_pc = match mode {
            Mode::User => 0x10000,
            Mode::Kernel => 0,
        };
        Vcpu {
            iregs: IntRegisters::new(),
            sp: 0,
            pc: initial_pc,
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
        // SP is register 31 in AArch64 (index 31 in our array)
        self.sp = sp as u64;
    }

    pub fn get_mode(&self) -> Mode {
        self.mode
    }

    pub fn reset_iregs(&mut self) {
        self.iregs = IntRegisters::new();
    }

    pub fn copy_iregs_to(&self, iregs: &mut IntRegisters) {
        *iregs = self.iregs;
    }

    pub fn copy_iregs_from(&mut self, iregs: &IntRegisters) {
        self.iregs = *iregs;
    }

    pub fn store(&mut self, trapframe: &Trapframe) {
        self.iregs = trapframe.regs;
        self.sp = trapframe.sp;
        self.pc = trapframe.epc;
    }

    pub fn switch(&mut self, trapframe: &mut Trapframe) {
        trapframe.regs = self.iregs;
        trapframe.sp = self.sp;
        trapframe.epc = self.pc;
    }
}
