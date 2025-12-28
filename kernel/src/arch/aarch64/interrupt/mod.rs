//! AArch64 interrupt handling
//!
//! Interrupt handling for AArch64 architecture.

use core::arch::asm;

pub fn interrupt_init() {
    // TODO: Initialize AArch64 interrupts
}

pub fn enable_interrupts() {
    // Unmask IRQ/FIQ/SError/Debug.
    unsafe {
        asm!("msr daifclr, #0xf", options(nostack));
    }
}

pub fn disable_interrupts() {
    unsafe {
        asm!("msr daifset, #0xf", options(nostack));
    }
}

pub fn enable_external_interrupts() {
    // External interrupts arrive as IRQ.
    unsafe {
        asm!("msr daifclr, #0x2", options(nostack));
    }
}

pub fn with_interrupts_disabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let saved: u64;
    unsafe {
        asm!("mrs {0}, daif", out(reg) saved, options(nostack));
        asm!("msr daifset, #0xf", options(nostack));
    }
    let ret = f();
    unsafe {
        asm!("msr daif, {0}", in(reg) saved, options(nostack));
    }
    ret
}

pub fn are_interrupts_enabled() -> bool {
    let daif: u64;
    unsafe { asm!("mrs {0}, daif", out(reg) daif, options(nostack)); }
    // DAIF.I (IRQ mask) bit is set when IRQs are disabled.
    (daif & (1 << 7)) == 0
}
