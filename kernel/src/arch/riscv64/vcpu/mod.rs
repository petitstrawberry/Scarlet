//! VCPU module for RISC-V 64-bit architecture.
//!
//! This module provides the virtual CPU (VCPU) abstraction for the RISC-V 64-bit
//! architecture. The VCPU is responsible for executing instructions and managing
//! the state of the CPU.

use crate::arch::Trapframe;

use super::IntRegisters;
use super::fpu::{FpuContext, VectorContext};

use alloc::boxed::Box;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    User,
    Kernel,
}

#[derive(Debug, Clone)]
pub struct Vcpu {
    pub iregs: IntRegisters,
    /// Floating-point register context (F/D extensions)
    pub fpu: FpuContext,
    /// Whether this task has ever used the FPU (F/D).
    pub fpu_used: bool,
    /// Vector register context (V extension)
    pub vector: Option<Box<VectorContext>>,
    /// Whether this task has ever used the Vector extension (V).
    pub vector_used: bool,
    pc: u64,
    asid: usize,
    mode: Mode,
}

impl Vcpu {
    pub fn new(mode: Mode) -> Self {
        Vcpu {
            iregs: IntRegisters::new(),
            fpu: FpuContext::new(),
            fpu_used: false,
            vector: None,
            vector_used: false,
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
        self.iregs.reg[2] = sp;
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

    /// Clone the entire VCPU state to another VCPU
    ///
    /// This copies all registers including general-purpose registers, FPU context,
    /// Vector context, and PC.
    pub fn clone_to(&self, other: &mut Vcpu) {
        other.iregs = self.iregs;
        other.fpu = self.fpu.clone();
        other.fpu_used = self.fpu_used;
        other.vector = self.vector.clone();
        other.vector_used = self.vector_used;
        other.pc = self.pc;
    }

    pub fn store(&mut self, trapframe: &Trapframe) {
        self.iregs = trapframe.regs;
        self.pc = trapframe.epc;
    }

    pub fn switch(&mut self, trapframe: &mut Trapframe) {
        trapframe.regs = self.iregs;
        trapframe.epc = self.pc;
    }
}
