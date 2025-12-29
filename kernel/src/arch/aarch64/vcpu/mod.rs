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
        self.iregs.reg[31] = sp;
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
        if self.mode == Mode::User && trapframe.epc == 0 {
            // During bring-up we sometimes observe ELR/epc saved as 0 (e.g. around early
            // scheduler/timer transitions). Never let that clobber the user PC, or we'll
            // ERET to VA 0 and immediately take an instruction abort.
            crate::early_println!(
                "[aarch64][vcpu] ignore NULL user pc; keeping pc={:#x}",
                self.pc
            );
        } else {
            self.pc = trapframe.epc;
        }
    }

    pub fn switch(&mut self, trapframe: &mut Trapframe) {
        trapframe.regs = self.iregs;
        let pc = if self.mode == Mode::User && self.pc == 0 {
            crate::early_println!("[aarch64][vcpu] fixup NULL user pc -> 0x10000");
            0x10000
        } else {
            self.pc
        };
        trapframe.epc = pc;
    }
}
