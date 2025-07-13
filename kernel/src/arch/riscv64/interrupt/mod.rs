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

/// Enable timer interrupts
/// 
/// Enables the timer interrupt by setting the STIE (Supervisor Timer Interrupt Enable) bit in the sie register.
pub fn enable_timer_interrupts() {
    unsafe {
        core::arch::asm!(
            "csrs sie, {0}",
            in(reg) 1 << 5, // Set STIE bit
            options(nostack)
        );
    }
}

/// Disable timer interrupts
/// 
/// Disables the timer interrupt by clearing the STIE (Supervisor Timer Interrupt Enable) bit in the sie register.
pub fn disable_timer_interrupts() {
    unsafe {
        core::arch::asm!(
            "csrc sie, {0}",
            in(reg) 1 << 5, // Clear STIE bit
            options(nostack)
        );
    }
}

/// Check if timer interrupts are enabled
/// 
/// Returns true if the STIE (Supervisor Timer Interrupt Enable) bit is set in the sie register.
pub fn are_timer_interrupts_enabled() -> bool {
    let sie: usize;
    unsafe {
        core::arch::asm!("csrr {}, sie", out(reg) sie);
    }
    (sie & (1 << 5)) != 0
}

/// Enable software interrupts
/// 
/// Enables the software interrupt by setting the SSIE (Supervisor Software Interrupt Enable) bit in the sie register.
pub fn enable_software_interrupts() {
    unsafe {
        core::arch::asm!(
            "csrs sie, {0}",
            in(reg) 1 << 1, // Set SSIE bit
            options(nostack)
        );
    }
}
/// Disable software interrupts
///
/// Disables the software interrupt by clearing the SSIE (Supervisor Software Interrupt Enable) bit in the sie register.
pub fn disable_software_interrupts() {
    unsafe {
        core::arch::asm!(
            "csrc sie, {0}",
            in(reg) 1 << 1, // Clear SSIE bit
            options(nostack)
        );
    }
}

/// Check if software interrupts are enabled
/// 
/// Returns true if the SSIE (Supervisor Software Interrupt Enable) bit is set in the sie register.
pub fn are_software_interrupts_enabled() -> bool {
    let sie: usize;
    unsafe {
        core::arch::asm!("csrr {}, sie", out(reg) sie);
    }
    (sie & (1 << 1)) != 0
}

/// Enable external interrupts
/// 
/// Enables the external interrupt by setting the SEIE (Supervisor External Interrupt Enable) bit in the sie register.
pub fn enable_external_interrupts() {
    unsafe {
        core::arch::asm!(
            "csrs sie, {0}",
            in(reg) 1 << 9, // Set SEIE bit
            options(nostack)
        );
    }
}

/// Disable external interrupts
///
/// Disables the external interrupt by clearing the SEIE (Supervisor External Interrupt Enable) bit in the sie register.
pub fn disable_external_interrupts() {
    unsafe {
        core::arch::asm!(
            "csrc sie, {0}",
            in(reg) 1 << 9, // Clear SEIE bit
            options(nostack)
        );
    }
}

/// Check if external interrupts are enabled
/// 
/// Returns true if the SEIE (Supervisor External Interrupt Enable) bit is set in the sie register.
pub fn are_external_interrupts_enabled() -> bool {
    let sie: usize;
    unsafe {
        core::arch::asm!("csrr {}, sie", out(reg) sie);
    }
    (sie & (1 << 9)) != 0
}
