//! ARM Generic Timer local interrupt controller (AArch64)
//!
//! This implements the CPU-local timer and (optionally) software interrupt control
//! using the architected system registers.
//!
//! We use the Virtual Timer (CNTV).
//!
//! When running under an EL2 hypervisor, guests typically use the virtual timer.
//! - Counter: CNTVCT_EL0
//! - Compare: CNTV_CVAL_EL0
//! - Control: CNTV_CTL_EL0
//! - Frequency: CNTFRQ_EL0

use core::arch::asm;

use crate::environment::MAX_NUM_CPUS;
use crate::interrupt::controllers::{LocalInterruptController, LocalInterruptType};
use crate::interrupt::{CpuId, InterruptError, InterruptResult};

/// CNTV_CTL_EL0 bit definitions
const CNTV_CTL_ENABLE: u64 = 1 << 0;
const CNTV_CTL_IMASK: u64 = 1 << 1;
const CNTV_CTL_ISTATUS: u64 = 1 << 2;

/// Timer PPI number.
///
/// EL1: Virtual Timer PPI 27.
/// EL2 VHE: EL2 Physical Timer (CNTHP) PPI 26, reserving the virtual timer for guests.
pub fn timer_ppi_irq() -> u32 {
    if crate::arch::aarch64::is_vhe_enabled() {
        26
    } else {
        27
    }
}

#[inline]
fn read_cntfrq_el0() -> u64 {
    let v: u64;
    unsafe {
        asm!("mrs {0}, cntfrq_el0", out(reg) v, options(nostack));
    }
    v
}

#[inline]
fn read_counter() -> u64 {
    let v: u64;
    unsafe {
        if crate::arch::aarch64::is_vhe_enabled() {
            asm!("mrs {0}, cntpct_el0", out(reg) v, options(nostack));
        } else {
            asm!("mrs {0}, cntvct_el0", out(reg) v, options(nostack));
        }
    }
    v
}

#[inline]
fn read_timer_ctl() -> u64 {
    let v: u64;
    unsafe {
        if crate::arch::aarch64::is_vhe_enabled() {
            asm!("mrs {0}, cntp_ctl_el0", out(reg) v, options(nostack));
        } else {
            asm!("mrs {0}, cntv_ctl_el0", out(reg) v, options(nostack));
        }
    }
    v
}

#[inline]
fn write_timer_ctl(v: u64) {
    unsafe {
        if crate::arch::aarch64::is_vhe_enabled() {
            asm!("msr cntp_ctl_el0, {0}", in(reg) v, options(nostack));
        } else {
            asm!("msr cntv_ctl_el0, {0}", in(reg) v, options(nostack));
        }
        asm!("isb", options(nostack));
    }
}

#[inline]
fn write_timer_cval(v: u64) {
    unsafe {
        if crate::arch::aarch64::is_vhe_enabled() {
            asm!("msr cntp_cval_el0, {0}", in(reg) v, options(nostack));
        } else {
            asm!("msr cntv_cval_el0, {0}", in(reg) v, options(nostack));
        }
        asm!("isb", options(nostack));
    }
}

/// AArch64 local controller backed by ARM Generic Timer.
///
/// The controller itself is stateless; all timer state lives in the architected registers.
pub struct ArmGenericTimer;

impl ArmGenericTimer {
    pub fn new() -> Self {
        ArmGenericTimer
    }

    pub fn is_timer_pending() -> bool {
        (read_timer_ctl() & CNTV_CTL_ISTATUS) != 0
    }

    fn enable_timer_interrupt() {
        // Enable timer and unmask interrupt.
        let mut ctl = read_timer_ctl();
        ctl |= CNTV_CTL_ENABLE;
        ctl &= !CNTV_CTL_IMASK;
        write_timer_ctl(ctl);
    }

    fn disable_timer_interrupt() {
        // Mask timer interrupt; keep ENABLE as-is.
        let mut ctl = read_timer_ctl();
        ctl |= CNTV_CTL_IMASK;
        write_timer_ctl(ctl);
    }
}

impl LocalInterruptController for ArmGenericTimer {
    fn init(&mut self, _cpu_id: CpuId) -> InterruptResult<()> {
        // Make sure the timer interrupt is masked until explicitly enabled.
        Self::disable_timer_interrupt();
        Ok(())
    }

    fn enable_interrupt(
        &self,
        _cpu_id: CpuId,
        interrupt_type: LocalInterruptType,
    ) -> InterruptResult<()> {
        match interrupt_type {
            LocalInterruptType::Timer => {
                Self::enable_timer_interrupt();
                Ok(())
            }
            _ => Err(InterruptError::NotSupported),
        }
    }

    fn disable_interrupt(
        &self,
        _cpu_id: CpuId,
        interrupt_type: LocalInterruptType,
    ) -> InterruptResult<()> {
        match interrupt_type {
            LocalInterruptType::Timer => {
                Self::disable_timer_interrupt();
                Ok(())
            }
            _ => Err(InterruptError::NotSupported),
        }
    }

    fn is_pending(&self, _cpu_id: CpuId, interrupt_type: LocalInterruptType) -> bool {
        match interrupt_type {
            LocalInterruptType::Timer => Self::is_timer_pending(),
            _ => false,
        }
    }

    fn clear_interrupt(
        &mut self,
        _cpu_id: CpuId,
        interrupt_type: LocalInterruptType,
    ) -> InterruptResult<()> {
        // Generic timer interrupt is level-sensitive on compare; clearing is done by
        // programming the next compare value.
        match interrupt_type {
            LocalInterruptType::Timer => Ok(()),
            _ => Err(InterruptError::NotSupported),
        }
    }

    fn send_software_interrupt(&self, _target_cpu: CpuId) -> InterruptResult<()> {
        Err(InterruptError::NotSupported)
    }

    fn clear_software_interrupt(&mut self, _cpu_id: CpuId) -> InterruptResult<()> {
        Err(InterruptError::NotSupported)
    }

    fn set_timer(&self, _cpu_id: CpuId, time: u64) -> InterruptResult<()> {
        // Program absolute compare value.
        write_timer_cval(time);
        Ok(())
    }

    fn get_time(&self) -> u64 {
        read_counter()
    }

    fn get_timer_frequency_hz(&self) -> u64 {
        read_cntfrq_el0()
    }
}

unsafe impl Send for ArmGenericTimer {}
unsafe impl Sync for ArmGenericTimer {}

fn register_local_timer_controller() {
    // Register for all CPUs that Scarlet is configured to support.
    let controller = alloc::boxed::Box::new(ArmGenericTimer::new());
    let _ = crate::interrupt::InterruptManager::global()
        .register_local_controller_for_range(controller, 0..(MAX_NUM_CPUS as CpuId));
}

crate::early_initcall!(register_local_timer_controller);
