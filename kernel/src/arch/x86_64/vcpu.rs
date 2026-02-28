//! x86_64 VCPU (Virtual CPU) state management
//!
//! Manages the per-CPU virtualized state including general-purpose registers,
//! FPU state, and system registers.

use super::Trapframe;
use super::fpu::FpuState;

/// Execution mode (kernel or user)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    User,
    Kernel,
}

/// VCPU state for x86_64
///
/// Contains all the state needed to save/restore a task's execution context.
#[derive(Debug, Clone)]
pub struct VCpuState {
    /// General-purpose registers and trapframe
    pub trapframe: Trapframe,
    /// FPU/SIMD state
    pub fpu_state: FpuState,
    /// Whether FPU has been used (for lazy FPU switching)
    pub fpu_used: bool,
    /// Current execution mode
    pub mode: Mode,
}

impl VCpuState {
    /// Create a new VCPU state
    pub fn new() -> Self {
        VCpuState {
            trapframe: Trapframe::new(),
            fpu_state: FpuState::new(),
            fpu_used: false,
            mode: Mode::Kernel,
        }
    }

    /// Switch to this VCPU state
    ///
    /// Copies the saved state into the provided trapframe.
    ///
    /// # Arguments
    /// * `trapframe` - The target trapframe to load state into
    pub fn switch(&mut self, trapframe: &mut Trapframe) {
        // Copy the trapframe
        *trapframe = self.trapframe.clone();
    }

    /// Save VCPU state from trapframe
    ///
    /// # Arguments
    /// * `trapframe` - The source trapframe to save state from
    pub fn save(&mut self, trapframe: &Trapframe) {
        self.trapframe = trapframe.clone();
    }

    /// Set the execution mode
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// Get the execution mode
    pub fn get_mode(&self) -> Mode {
        self.mode
    }

    /// Mark FPU as used
    pub fn mark_fpu_used(&mut self) {
        self.fpu_used = true;
    }

    /// Reset FPU used flag
    pub fn reset_fpu_used(&mut self) {
        self.fpu_used = false;
    }
}

impl Default for VCpuState {
    fn default() -> Self {
        Self::new()
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
