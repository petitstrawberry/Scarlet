//! AArch64 virtual CPU support
//!
//! Virtual CPU functionality for AArch64 architecture.

use crate::arch::Trapframe;

use super::fpu::FpuContext;
use super::IntRegisters;
use crate::arch::Mode;

#[derive(Debug, Clone)]
pub struct Vcpu {
    pub iregs: IntRegisters,
    /// Floating-point and SIMD register context (NEON)
    pub fpu: FpuContext,
    /// Whether this task has ever used FP/SIMD (NEON).
    pub fpu_used: bool,
    pub sp: u64,
    pc: u64,
    spsr: u64,
    tpidr_el0: u64,
    tpidrro_el0: u64,
    asid: usize,
    mode: Mode,
}

impl Vcpu {
    pub fn new(mode: Mode) -> Self {
        let initial_pc = match mode {
            Mode::User | Mode::GuestUser => 0x10000,
            Mode::Kernel | Mode::GuestKernel => 0,
        };
        Vcpu {
            iregs: IntRegisters::new(),
            fpu: FpuContext::new(),
            fpu_used: false,
            sp: 0,
            pc: initial_pc,
            spsr: 0,
            tpidr_el0: 0,
            tpidrro_el0: 0,
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

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
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
    /// This copies all registers, including general-purpose registers,
    /// FPU/SIMD context, SP, PC, SPSR, and thread-local storage registers.
    pub fn clone_to(&self, other: &mut Vcpu) {
        other.iregs = self.iregs;
        other.fpu = self.fpu.clone();
        other.fpu_used = self.fpu_used;
        other.sp = self.sp;
        other.pc = self.pc;
        other.spsr = self.spsr;
        other.tpidr_el0 = self.tpidr_el0;
        other.tpidrro_el0 = self.tpidrro_el0;
    }

    pub fn get_sp(&self) -> usize {
        self.sp as usize
    }

    pub fn get_spsr(&self) -> u64 {
        self.spsr
    }

    pub fn set_spsr(&mut self, spsr: u64) {
        self.spsr = spsr;
    }

    pub fn get_tpidr_el0(&self) -> u64 {
        self.tpidr_el0
    }

    pub fn set_tpidr_el0(&mut self, tpidr_el0: u64) {
        self.tpidr_el0 = tpidr_el0;
    }

    pub fn get_tpidrro_el0(&self) -> u64 {
        self.tpidrro_el0
    }

    pub fn set_tpidrro_el0(&mut self, tpidrro_el0: u64) {
        self.tpidrro_el0 = tpidrro_el0;
    }

    pub fn store(&mut self, trapframe: &Trapframe) {
        self.iregs = trapframe.regs;
        self.sp = trapframe.sp;
        self.pc = trapframe.elr;
        self.spsr = trapframe.spsr;
        self.tpidr_el0 = trapframe.tpidr_el0;
        self.tpidrro_el0 = trapframe.tpidrro_el0;
    }

    pub fn switch(&mut self, trapframe: &mut Trapframe) {
        trapframe.regs = self.iregs;
        trapframe.sp = self.sp;
        trapframe.elr = self.pc;
        trapframe.spsr = self.spsr;
        trapframe.tpidr_el0 = self.tpidr_el0;
        trapframe.tpidrro_el0 = self.tpidrro_el0;
    }

    /// Get the TLS (Thread Local Storage) pointer for this task
    ///
    /// On AArch64, TLS is stored in the TPIDR_EL0 system register.
    #[inline]
    pub fn get_tls_pointer(&self) -> usize {
        self.tpidr_el0 as usize
    }

    /// Set the TLS (Thread Local Storage) pointer for this task
    ///
    /// On AArch64, TLS is stored in the TPIDR_EL0 system register.
    #[inline]
    pub fn set_tls_pointer(&mut self, ptr: usize) {
        self.tpidr_el0 = ptr as u64;
    }
}
