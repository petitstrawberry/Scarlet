//! AArch64 interrupt handling
//!
//! Interrupt handling for AArch64 architecture.

use core::arch::asm;
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use crate::arch::get_cpu;
use crate::interrupt::{
    InterruptError,
    controllers::{LocalInterruptType, PendingIrq, RESCHEDULE_IPI_VIRQ},
};

/// GIC SGI used by Scarlet for scheduler reschedule IPIs.
pub const RESCHEDULE_SGI: u32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerInterruptRoute {
    Unknown,
    ExternalControllerIrq,
    FastInterrupt,
}

static TIMER_INTERRUPT_ROUTE: AtomicU8 = AtomicU8::new(TimerInterruptRoute::Unknown as u8);
static TIMER_EXTERNAL_INTERRUPT_ID: AtomicU32 = AtomicU32::new(u32::MAX);

pub fn configure_timer_interrupt_route(
    route: TimerInterruptRoute,
    external_interrupt_id: Option<u32>,
) {
    TIMER_INTERRUPT_ROUTE.store(route as u8, Ordering::Relaxed);
    TIMER_EXTERNAL_INTERRUPT_ID.store(external_interrupt_id.unwrap_or(u32::MAX), Ordering::Relaxed);
}

pub fn timer_interrupt_route() -> TimerInterruptRoute {
    match TIMER_INTERRUPT_ROUTE.load(Ordering::Relaxed) {
        x if x == TimerInterruptRoute::ExternalControllerIrq as u8 => {
            TimerInterruptRoute::ExternalControllerIrq
        }
        x if x == TimerInterruptRoute::FastInterrupt as u8 => TimerInterruptRoute::FastInterrupt,
        _ => TimerInterruptRoute::Unknown,
    }
}

pub fn timer_external_interrupt_id() -> Option<u32> {
    let interrupt_id = TIMER_EXTERNAL_INTERRUPT_ID.load(Ordering::Relaxed);
    (interrupt_id != u32::MAX).then_some(interrupt_id)
}

pub fn is_arch_timer_external_interrupt(interrupt_id: u32) -> bool {
    timer_external_interrupt_id() == Some(interrupt_id)
}

/// Check whether a handled interrupt is a scheduler reschedule IPI.
///
/// # Arguments
///
/// * `pending` - Pending IRQ mapping returned by the interrupt controller.
///
/// # Returns
///
/// `true` when the interrupt should run scheduler reschedule handling.
pub fn is_reschedule_interrupt(pending: &PendingIrq) -> bool {
    pending.mapping.virq == RESCHEDULE_SGI || pending.mapping.virq == RESCHEDULE_IPI_VIRQ
}

pub fn interrupt_init() {
    // TODO: Initialize AArch64 interrupts
}

/// Enable interrupts globally (IRQ/FIQ/SError/Debug) at CPU level.
///
/// Note: This is gated during early scheduler bring-up; see
/// `crate::arch::aarch64::mark_interrupts_allowed`.
pub fn enable_interrupts() {
    // Keep interrupts masked until the timer has started at least once.
    // See `crate::arch::aarch64::mark_interrupts_allowed`.
    // if !crate::arch::aarch64::interrupts_allowed() {
    //     unsafe {
    //         asm!("msr daifset, #0xf", options(nostack));
    //     }
    //     return;
    // }
    unsafe {
        asm!("msr daifclr, #0xf", options(nostack));
    }
}

/// Disable interrupts globally (IRQ/FIQ/SError/Debug) at CPU level.
pub fn disable_interrupts() {
    unsafe {
        asm!("msr daifset, #0xf", options(nostack));
    }
}

/// Save the current DAIF state and mask every interrupt class.
///
/// # Returns
///
/// The complete DAIF value to pass to [`restore_interrupts`].
pub fn save_and_disable_interrupts() -> usize {
    let saved: usize;
    unsafe {
        asm!("mrs {0}, daif", out(reg) saved, options(nostack));
        asm!("msr daifset, #0xf", options(nostack));
    }
    saved
}

/// Restore a DAIF state returned by [`save_and_disable_interrupts`].
///
/// # Arguments
///
/// * `saved` - Complete DAIF value to restore.
pub fn restore_interrupts(saved: usize) {
    unsafe {
        asm!("msr daif, {0}", in(reg) saved, options(nostack));
    }
}

/// Enable external interrupt reception sources without changing DAIF.
///
/// Source and controller enables, such as GIC interrupt-line configuration,
/// are performed separately. Kernel bootstrap must remain non-preemptible, so
/// unmasking DAIF.I is restricted to the controlled idle and user-return paths.
pub fn enable_external_interrupts() {}

/// Enable software interrupt reception at CPU level.
///
/// AArch64 scheduler IPIs are delivered as GIC SGIs, which share the IRQ CPU
/// mask with other external interrupts. The per-SGI source enable is handled by
/// the GIC driver, so there is no separate architectural CPU bit to set here.
pub fn enable_software_interrupts() {}

/// Disable external interrupt reception sources without changing DAIF.
///
/// Source and controller disables are performed separately. This intentionally
/// leaves DAIF.I alone so it remains owned by the controlled idle and
/// user-return paths, and by save/restore users such as [`crate::sync::IrqGuard`].
pub fn disable_external_interrupts() {}

/// Enable a core-local interrupt source via the InterruptManager.
///
/// This corresponds to "core-local" enables such as the architectural timer.
pub fn enable_core_local_interrupt(source: LocalInterruptType) -> Result<(), &'static str> {
    let cpu_id = get_cpu().get_cpuid() as u32;
    crate::interrupt::InterruptManager::global()
        .enable_local_interrupt(cpu_id, source)
        .map_err(|_| "failed to enable core-local interrupt")
}

/// Disable a core-local interrupt source via the InterruptManager.
pub fn disable_core_local_interrupt(source: LocalInterruptType) -> Result<(), &'static str> {
    let cpu_id = get_cpu().get_cpuid() as u32;
    crate::interrupt::InterruptManager::global()
        .disable_local_interrupt(cpu_id, source)
        .map_err(|_| "failed to disable core-local interrupt")
}

/// Enable an external interrupt line (GIC-backed) for the current CPU.
pub fn enable_external_interrupt_line(interrupt_id: u32) -> Result<(), &'static str> {
    let cpu_id = get_cpu().get_cpuid() as u32;
    crate::interrupt::InterruptManager::global()
        .enable_external_interrupt(interrupt_id, cpu_id)
        .map_err(|_| "failed to enable external interrupt line")
}

/// Disable an external interrupt line (GIC-backed) for the current CPU.
pub fn disable_external_interrupt_line(interrupt_id: u32) -> Result<(), &'static str> {
    let cpu_id = get_cpu().get_cpuid() as u32;
    crate::interrupt::InterruptManager::global()
        .disable_external_interrupt(interrupt_id, cpu_id)
        .map_err(|_| "failed to disable external interrupt line")
}

/// Unmask the architectural timer interrupt at the timer source.
///
/// This is the closest equivalent to RISC-V's per-source enable bit like STIE.
/// Uses CNTP_CTL_EL0 (physical) in VHE mode, CNTV_CTL_EL0 (virtual) otherwise.
pub fn enable_timer_source_interrupt() {
    let mut ctl: u64;
    unsafe {
        if crate::arch::aarch64::is_vhe_enabled() {
            asm!("mrs {0}, cntp_ctl_el0", out(reg) ctl, options(nostack));
        } else {
            asm!("mrs {0}, cntv_ctl_el0", out(reg) ctl, options(nostack));
        }
        ctl &= !(1 << 1); // IMASK=0: unmask
        ctl |= 1; // ENABLE=1
        if crate::arch::aarch64::is_vhe_enabled() {
            asm!("msr cntp_ctl_el0, {0}", in(reg) ctl, options(nostack));
        } else {
            asm!("msr cntv_ctl_el0, {0}", in(reg) ctl, options(nostack));
        }
        asm!("isb", options(nostack));
    }
}

/// Mask the architectural timer interrupt at the timer source.
pub fn disable_timer_source_interrupt() {
    let mut ctl: u64;
    unsafe {
        if crate::arch::aarch64::is_vhe_enabled() {
            asm!("mrs {0}, cntp_ctl_el0", out(reg) ctl, options(nostack));
        } else {
            asm!("mrs {0}, cntv_ctl_el0", out(reg) ctl, options(nostack));
        }
        ctl |= 1 << 1; // IMASK=1: mask
        if crate::arch::aarch64::is_vhe_enabled() {
            asm!("msr cntp_ctl_el0, {0}", in(reg) ctl, options(nostack));
        } else {
            asm!("msr cntv_ctl_el0, {0}", in(reg) ctl, options(nostack));
        }
        asm!("isb", options(nostack));
    }
}

/// Enable the architectural timer interrupt for the current CPU.
///
/// This is a platform-agnostic helper that:
/// 1. Enables the timer at the local controller level (CNTV_CTL_EL0)
/// 2. Attempts to enable the timer PPI at the external controller level
///
/// On GIC-based systems, the timer uses PPI 27 (EL1 virtual) or PPI 26 (EL2 physical/VHE).
/// On Apple Silicon (AIC), the timer bypasses the AIC entirely
/// (it's wired to FIQ), so the external enable gracefully fails and is ignored.
pub fn enable_arch_timer_interrupt() -> Result<(), &'static str> {
    let cpu_id = get_cpu().get_cpuid() as u32;

    crate::interrupt::InterruptManager::global()
        .enable_local_interrupt(cpu_id, LocalInterruptType::Timer)
        .map_err(|_| "failed to enable local timer interrupt")?;

    match timer_interrupt_route() {
        TimerInterruptRoute::FastInterrupt => {}
        TimerInterruptRoute::ExternalControllerIrq => {
            let interrupt_id = timer_external_interrupt_id()
                .ok_or("external timer interrupt route is not configured")?;
            crate::interrupt::InterruptManager::global()
                .enable_external_interrupt(interrupt_id, cpu_id)
                .or_else(|e| {
                    if matches!(e, InterruptError::InvalidInterruptId) {
                        Ok(())
                    } else {
                        Err("failed to enable timer PPI in external controller")
                    }
                })?;
        }
        TimerInterruptRoute::Unknown => return Err("timer interrupt route is not configured"),
    }

    Ok(())
}

/// Disable the architectural timer interrupt for the current CPU.
pub fn disable_arch_timer_interrupt() -> Result<(), &'static str> {
    let cpu_id = get_cpu().get_cpuid() as u32;

    crate::interrupt::InterruptManager::global()
        .disable_local_interrupt(cpu_id, LocalInterruptType::Timer)
        .map_err(|_| "failed to disable local timer interrupt")?;

    match timer_interrupt_route() {
        TimerInterruptRoute::FastInterrupt => {}
        TimerInterruptRoute::ExternalControllerIrq => {
            let interrupt_id = timer_external_interrupt_id()
                .ok_or("external timer interrupt route is not configured")?;
            crate::interrupt::InterruptManager::global()
                .disable_external_interrupt(interrupt_id, cpu_id)
                .or_else(|e| {
                    if matches!(e, InterruptError::InvalidInterruptId) {
                        Ok(())
                    } else {
                        Err("failed to disable timer PPI in external controller")
                    }
                })?;
        }
        TimerInterruptRoute::Unknown => return Err("timer interrupt route is not configured"),
    }

    Ok(())
}

pub fn is_arch_timer_pending() -> bool {
    let cpu_id = get_cpu().get_cpuid() as u32;
    crate::interrupt::InterruptManager::global()
        .is_local_interrupt_pending(cpu_id, LocalInterruptType::Timer)
}

pub fn with_interrupts_disabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let saved = save_and_disable_interrupts();
    let ret = f();
    restore_interrupts(saved);
    ret
}

pub fn are_interrupts_enabled() -> bool {
    let daif: u64;
    unsafe {
        asm!("mrs {0}, daif", out(reg) daif, options(nostack));
    }
    // DAIF.I (IRQ mask) bit is set when IRQs are disabled.
    (daif & (1 << 7)) == 0
}
