//! Architecture-specific code for Scarlet kernel
//!
//! This module contains architecture-specific implementations and definitions
//! for the Scarlet kernel. Each architecture has its own set of files that
//! implement the necessary functionality.
//!

/// Policy for whether user-mode should run with IRQs enabled immediately after
/// returning from the kernel (e.g. `sret`/`eret`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserReturnIrqPolicy {
    /// Do not change the interrupt state; honor the trapframe/arch default.
    Inherit,
    /// Ensure IRQs are enabled right after returning to user mode.
    Enable,
    /// Ensure IRQs are disabled right after returning to user mode.
    Disable,
}

/// Options applied right before returning to user mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserEntryOptions {
    pub irq_policy: UserReturnIrqPolicy,
}

impl Default for UserEntryOptions {
    fn default() -> Self {
        Self {
            irq_policy: UserReturnIrqPolicy::Inherit,
        }
    }
}

/// Configure architecture-specific state for the upcoming return to user mode.
///
/// This is intended to be called immediately before the final trampoline/exit
/// jump that performs `sret`/`eret`.
pub fn configure_user_entry(trapframe: &mut Trapframe, options: UserEntryOptions) {
    #[cfg(target_arch = "riscv64")]
    {
        riscv64::configure_user_entry(trapframe, options)
    }
    #[cfg(target_arch = "aarch64")]
    {
        aarch64::configure_user_entry(trapframe, options)
    }
}

#[cfg(target_arch = "riscv64")]
pub mod riscv64;
#[cfg(target_arch = "riscv64")]
pub use riscv64::*;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use aarch64::*;

// Re-export kernel context for architecture-independent use
#[cfg(target_arch = "riscv64")]
pub use riscv64::context::KernelContext;

#[cfg(target_arch = "aarch64")]
pub use aarch64::context::KernelContext;

// Re-export FPU context and functions for architecture-independent use
#[cfg(target_arch = "riscv64")]
pub use riscv64::fpu;

#[cfg(target_arch = "aarch64")]
pub use aarch64::fpu;
