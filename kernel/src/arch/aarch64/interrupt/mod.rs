//! AArch64 interrupt handling
//!
//! Interrupt handling for AArch64 architecture.

use core::arch::asm;
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use crate::arch::get_cpu;
use crate::interrupt::{InterruptError, InterruptManager, controllers::LocalInterruptType};

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

/// Enable external interrupts (IRQ) at CPU level.
///
/// This only unmasks the IRQ bit (DAIF.I). Source-level enables (e.g. GIC
/// enable of a specific interrupt ID) are handled separately.
pub fn enable_external_interrupts() {
    // // External interrupts arrive as IRQ.
    // if !crate::arch::aarch64::interrupts_allowed() {
    //     unsafe {
    //         asm!("msr daifset, #0xf", options(nostack));
    //     }
    //     return;
    // }
    // unsafe {
    //     asm!("msr daifclr, #0x2", options(nostack));
    // }
}

/// Disable external interrupts (IRQ) at CPU level.
pub fn disable_external_interrupts() {
    // unsafe {
    //     asm!("msr daifset, #0x2", options(nostack));
    // }
}

/// Enable a core-local interrupt source via the InterruptManager.
///
/// This corresponds to "core-local" enables such as the architectural timer.
pub fn enable_core_local_interrupt(source: LocalInterruptType) -> Result<(), &'static str> {
    InterruptManager::with_manager(|mgr| {
        let cpu_id = get_cpu().get_cpuid() as u32;
        mgr.enable_local_interrupt(cpu_id, source)
    })
    .map_err(|_| "failed to enable core-local interrupt")
}

/// Disable a core-local interrupt source via the InterruptManager.
pub fn disable_core_local_interrupt(source: LocalInterruptType) -> Result<(), &'static str> {
    InterruptManager::with_manager(|mgr| {
        let cpu_id = get_cpu().get_cpuid() as u32;
        mgr.disable_local_interrupt(cpu_id, source)
    })
    .map_err(|_| "failed to disable core-local interrupt")
}

/// Enable an external interrupt line (GIC-backed) for the current CPU.
pub fn enable_external_interrupt_line(interrupt_id: u32) -> Result<(), &'static str> {
    InterruptManager::with_manager(|mgr| {
        let cpu_id = get_cpu().get_cpuid() as u32;
        mgr.enable_external_interrupt(interrupt_id, cpu_id)
    })
    .map_err(|_| "failed to enable external interrupt line")
}

/// Disable an external interrupt line (GIC-backed) for the current CPU.
pub fn disable_external_interrupt_line(interrupt_id: u32) -> Result<(), &'static str> {
    InterruptManager::with_manager(|mgr| {
        let cpu_id = get_cpu().get_cpuid() as u32;
        mgr.disable_external_interrupt(interrupt_id, cpu_id)
    })
    .map_err(|_| "failed to disable external interrupt line")
}

/// Unmask the architectural timer interrupt at the timer source.
///
/// This is the closest equivalent to RISC-V's per-source enable bit like STIE,
/// but on AArch64 we use CNTV_CTL_EL0 (virtual timer).
pub fn enable_timer_source_interrupt() {
    unsafe {
        let mut ctl: u64;
        asm!("mrs {0}, cntv_ctl_el0", out(reg) ctl, options(nostack));
        // IMASK bit (1): 0 = unmask timer interrupt.
        ctl &= !(1 << 1);
        // ENABLE bit (0): 1 = enable timer.
        ctl |= 1;
        asm!("msr cntv_ctl_el0, {0}", in(reg) ctl, options(nostack));
        asm!("isb", options(nostack));
    }
}

/// Mask the architectural timer interrupt at the timer source.
pub fn disable_timer_source_interrupt() {
    unsafe {
        let mut ctl: u64;
        asm!("mrs {0}, cntv_ctl_el0", out(reg) ctl, options(nostack));
        // IMASK bit (1): 1 = mask timer interrupt.
        ctl |= 1 << 1;
        asm!("msr cntv_ctl_el0, {0}", in(reg) ctl, options(nostack));
        asm!("isb", options(nostack));
    }
}

/// Enable the architectural timer interrupt for the current CPU.
///
/// This is a platform-agnostic helper that:
/// 1. Enables the timer at the local controller level (CNTV_CTL_EL0)
/// 2. Attempts to enable the timer PPI at the external controller level
///
/// On GIC-based systems, the timer uses PPI 27 (EL1) or PPI 28 (EL2 VHE).
/// On Apple Silicon (AIC), the timer bypasses the AIC entirely
/// (it's wired to FIQ), so the external enable gracefully fails and is ignored.
pub fn enable_arch_timer_interrupt() -> Result<(), &'static str> {
    let cpu_id = get_cpu().get_cpuid() as u32;

    InterruptManager::with_manager(|mgr| {
        mgr.enable_local_interrupt(cpu_id, LocalInterruptType::Timer)
    })
    .map_err(|_| "failed to enable local timer interrupt")?;

    match timer_interrupt_route() {
        TimerInterruptRoute::FastInterrupt => {}
        TimerInterruptRoute::ExternalControllerIrq => {
            let interrupt_id = timer_external_interrupt_id()
                .ok_or("external timer interrupt route is not configured")?;
            InterruptManager::with_manager(|mgr| {
                mgr.enable_external_interrupt(interrupt_id, cpu_id)
            })
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

    InterruptManager::with_manager(|mgr| {
        mgr.disable_local_interrupt(cpu_id, LocalInterruptType::Timer)
    })
    .map_err(|_| "failed to disable local timer interrupt")?;

    match timer_interrupt_route() {
        TimerInterruptRoute::FastInterrupt => {}
        TimerInterruptRoute::ExternalControllerIrq => {
            let interrupt_id = timer_external_interrupt_id()
                .ok_or("external timer interrupt route is not configured")?;
            InterruptManager::with_manager(|mgr| {
                mgr.disable_external_interrupt(interrupt_id, cpu_id)
            })
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
    InterruptManager::with_manager(|mgr| {
        let cpu_id = get_cpu().get_cpuid() as u32;
        mgr.is_local_interrupt_pending(cpu_id, LocalInterruptType::Timer)
    })
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
    unsafe {
        asm!("mrs {0}, daif", out(reg) daif, options(nostack));
    }
    // DAIF.I (IRQ mask) bit is set when IRQs are disabled.
    (daif & (1 << 7)) == 0
}
