//! RISC-V specific interrupt control functions
//!
//! This module provides architecture-specific interrupt management functions
//! for the RISC-V architecture.

/// Enable interrupts globally on RISC-V
/// 
/// Sets the SIE (Supervisor Interrupt Enable) bit in the sstatus register.
pub fn enable_interrupts() {
    unsafe {
        core::arch::asm!("csrs sstatus, {}", in(reg) 1 << 1);
    }
}

/// Disable interrupts globally on RISC-V
/// 
/// Clears the SIE (Supervisor Interrupt Enable) bit in the sstatus register.
pub fn disable_interrupts() {
    unsafe {
        core::arch::asm!("csrc sstatus, {}", in(reg) 1 << 1);
    }
}

/// Check if interrupts are currently enabled on RISC-V
/// 
/// Returns true if the SIE bit is set in the sstatus register.
pub fn are_interrupts_enabled() -> bool {
    let sstatus: usize;
    unsafe {
        core::arch::asm!("csrr {}, sstatus", out(reg) sstatus);
    }
    (sstatus & (1 << 1)) != 0
}

/// Execute a closure with interrupts disabled
/// 
/// This is a convenience function that saves the current interrupt state,
/// disables interrupts, executes the closure, and restores the interrupt state.
pub fn with_interrupts_disabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let old_state = are_interrupts_enabled();
    disable_interrupts();
    let result = f();
    if old_state {
        enable_interrupts();
    }
    result
}
