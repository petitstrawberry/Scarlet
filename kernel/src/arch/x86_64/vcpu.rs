//! x86_64 VCPU (Virtual CPU) state management

use super::Trapframe;
use super::fpu::FpuState;
use super::registers::IntRegisters;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    User,
    Kernel,
}

pub type Vcpu = VCpuState;

#[derive(Debug, Clone)]
pub struct VCpuState {
    pub iregs: IntRegisters,
    pub fpu_state: FpuState,
    pub fpu_used: bool,
    pub mode: Mode,
    pc: u64,
}

impl VCpuState {
    pub fn new(mode: Mode) -> Self {
        VCpuState {
            iregs: IntRegisters::new(),
            fpu_state: FpuState::new(),
            fpu_used: false,
            mode,
            pc: 0,
        }
    }

    pub fn switch(&mut self, trapframe: &mut Trapframe) {
        self.iregs = trapframe.regs.clone();
        self.pc = trapframe.rip;
    }

    pub fn save(&mut self, trapframe: &Trapframe) {
        self.iregs = trapframe.regs.clone();
        self.pc = trapframe.rip;
    }

    pub fn store(&mut self, trapframe: &Trapframe) {
        self.save(trapframe);
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    pub fn get_mode(&self) -> Mode {
        self.mode
    }

    pub fn mark_fpu_used(&mut self) {
        self.fpu_used = true;
    }

    pub fn reset_fpu_used(&mut self) {
        self.fpu_used = false;
    }

    pub fn set_sp(&mut self, sp: usize) {
        self.iregs.rsp = sp;
    }

    pub fn set_pc(&mut self, pc: u64) {
        self.pc = pc;
    }

    pub fn get_pc(&self) -> u64 {
        self.pc
    }

    pub fn set_tls_pointer(&mut self, ptr: usize) {
        self.iregs.fsbase = ptr;
    }

    pub fn clone_to(&self, other: &mut VCpuState) {
        other.iregs = self.iregs.clone();
        other.fpu_state = self.fpu_state.clone();
        other.fpu_used = self.fpu_used;
        other.mode = self.mode;
        other.pc = self.pc;
    }

    pub fn reset_iregs(&mut self) {
        self.iregs = IntRegisters::new();
    }

    pub fn copy_iregs_to(&self, iregs: &mut IntRegisters) {
        *iregs = self.iregs.clone();
    }

    pub fn copy_iregs_from(&mut self, iregs: &IntRegisters) {
        self.iregs = iregs.clone();
    }
}

impl Default for VCpuState {
    fn default() -> Self {
        Self::new(Mode::Kernel)
    }
}

/// Initialize a new VCPU for a task
///
/// # Arguments
/// * `entry` - Entry point address
/// * `stack_top` - Top of the user stack
/// * `arg` - Argument to pass to the task
pub fn init_vcpu(entry: usize, stack_top: usize, arg: usize) -> Trapframe {
    let mut trapframe = Trapframe::new();

    // Set up registers for task entry
    trapframe.rip = entry as u64;
    trapframe.rsp = stack_top as u64;
    trapframe.rflags = 0x202; // Enable interrupts

    // Pass argument in RDI
    trapframe.regs.rdi = arg;

    trapframe
}
