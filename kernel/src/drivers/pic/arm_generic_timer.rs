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
use crate::interrupt::{CpuId, InterruptError, InterruptManager, InterruptResult};

/// CNTV_CTL_EL0 bit definitions
const CNTV_CTL_ENABLE: u64 = 1 << 0;
const CNTV_CTL_IMASK: u64 = 1 << 1;
const CNTV_CTL_ISTATUS: u64 = 1 << 2;

/// QEMU virt / ARM Generic Timer: Virtual Timer PPI is 27.
///
/// We keep this as a constant so the IRQ handler can cheaply sanity-check timer pending state.
pub const CNTV_PPI_IRQ: u32 = 27;

#[inline]
fn read_cntfrq_el0() -> u64 {
    let v: u64;
    unsafe {
        asm!("mrs {0}, cntfrq_el0", out(reg) v, options(nostack));
    }
    v
}

#[inline]
fn read_cntvct_el0() -> u64 {
    let v: u64;
    unsafe {
        asm!("mrs {0}, cntvct_el0", out(reg) v, options(nostack));
    }
    v
}

#[inline]
fn read_cntv_ctl_el0() -> u64 {
    let v: u64;
    unsafe {
        asm!("mrs {0}, cntv_ctl_el0", out(reg) v, options(nostack));
    }
    v
}

#[inline]
fn write_cntv_ctl_el0(v: u64) {
    unsafe {
        asm!(
            "msr cntv_ctl_el0, {0}",
            "isb",
            in(reg) v,
            options(nostack)
        );
    }
}

#[inline]
fn write_cntv_cval_el0(v: u64) {
    unsafe {
        asm!(
            "msr cntv_cval_el0, {0}",
            "isb",
            in(reg) v,
            options(nostack)
        );
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
        (read_cntv_ctl_el0() & CNTV_CTL_ISTATUS) != 0
    }

    fn enable_timer_interrupt() {
        // Enable timer and unmask interrupt.
        let mut ctl = read_cntv_ctl_el0();
        ctl |= CNTV_CTL_ENABLE;
        ctl &= !CNTV_CTL_IMASK;
        write_cntv_ctl_el0(ctl);
    }

    fn disable_timer_interrupt() {
        // Mask timer interrupt; keep ENABLE as-is.
        let mut ctl = read_cntv_ctl_el0();
        ctl |= CNTV_CTL_IMASK;
        write_cntv_ctl_el0(ctl);
    }
}

impl LocalInterruptController for ArmGenericTimer {
    fn init(&mut self, _cpu_id: CpuId) -> InterruptResult<()> {
        // Make sure the timer interrupt is masked until explicitly enabled.
        Self::disable_timer_interrupt();
        Ok(())
    }

    fn enable_interrupt(
        &mut self,
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
        &mut self,
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

    fn send_software_interrupt(&mut self, _target_cpu: CpuId) -> InterruptResult<()> {
        Err(InterruptError::NotSupported)
    }

    fn clear_software_interrupt(&mut self, _cpu_id: CpuId) -> InterruptResult<()> {
        Err(InterruptError::NotSupported)
    }

    fn set_timer(&mut self, _cpu_id: CpuId, time: u64) -> InterruptResult<()> {
        // Program absolute compare value.
        write_cntv_cval_el0(time);
        Ok(())
    }

    fn get_time(&self) -> u64 {
        read_cntvct_el0()
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
    let _ = InterruptManager::with_manager(|mgr| {
        mgr.register_local_controller_for_range(controller, 0..(MAX_NUM_CPUS as CpuId))
    });
}

crate::early_initcall!(register_local_timer_controller);
