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
use core::sync::atomic::{AtomicU32, Ordering};

use crate::device::manager::{DeviceManager, DriverPriority, probe_defer};
use crate::device::platform::{
    PlatformDeviceDriver, PlatformDeviceInfo, resource::PlatformDeviceResourceType,
};
use crate::environment::MAX_NUM_CPUS;
use crate::interrupt::controllers::TimerController;
use crate::interrupt::{CpuId, InterruptError, InterruptResult};

/// CNTV_CTL_EL0 bit definitions
const CNTV_CTL_ENABLE: u64 = 1 << 0;
const CNTV_CTL_IMASK: u64 = 1 << 1;
const CNTV_CTL_ISTATUS: u64 = 1 << 2;
const CNTV_CTL_STATE_MASK: u64 = CNTV_CTL_ENABLE | CNTV_CTL_IMASK | CNTV_CTL_ISTATUS;
const CNTV_CTL_FIRING: u64 = CNTV_CTL_ENABLE | CNTV_CTL_ISTATUS;

#[inline]
const fn timer_control_is_firing(control: u64) -> bool {
    control & CNTV_CTL_STATE_MASK == CNTV_CTL_FIRING
}

const TIMER_PPI_UNCONFIGURED: u32 = u32::MAX;
static TIMER_PPI_IRQ: AtomicU32 = AtomicU32::new(TIMER_PPI_UNCONFIGURED);

/// Timer PPI number selected from firmware, with architectural defaults for
/// platforms that do not describe the generic timer in a device tree.
pub fn timer_ppi_irq() -> u32 {
    let configured = TIMER_PPI_IRQ.load(Ordering::Acquire);
    if configured != TIMER_PPI_UNCONFIGURED {
        return configured;
    }

    // Standard GIC wiring: virtual timer PPI 27, EL2 physical timer PPI 26.
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
        timer_control_is_firing(read_timer_ctl())
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

impl TimerController for ArmGenericTimer {
    fn init(
        &mut self,
        _cpu_id: CpuId,
        _mode: crate::interrupt::controllers::InterruptControllerInitMode,
    ) -> InterruptResult<()> {
        // Make sure the timer interrupt is masked until explicitly enabled.
        Self::disable_timer_interrupt();
        Ok(())
    }

    fn enable_timer(&self, _cpu_id: CpuId) -> InterruptResult<()> {
        Self::enable_timer_interrupt();
        Ok(())
    }

    fn disable_timer(&self, _cpu_id: CpuId) -> InterruptResult<()> {
        Self::disable_timer_interrupt();
        Ok(())
    }

    fn is_timer_pending(&self, _cpu_id: CpuId) -> bool {
        Self::is_timer_pending()
    }

    fn clear_timer(&mut self, _cpu_id: CpuId) -> InterruptResult<()> {
        // Generic timer interrupt is level-sensitive on compare; clearing is done by
        // programming the next compare value.
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn timer_control_is_firing_only_when_enabled_and_unmasked() {
        assert!(timer_control_is_firing(CNTV_CTL_ENABLE | CNTV_CTL_ISTATUS));
        assert!(!timer_control_is_firing(CNTV_CTL_ISTATUS));
        assert!(!timer_control_is_firing(
            CNTV_CTL_ENABLE | CNTV_CTL_IMASK | CNTV_CTL_ISTATUS
        ));
        assert!(!timer_control_is_firing(CNTV_CTL_ENABLE));
    }
}

fn register_arm_generic_timer() {
    // Register for all CPUs that Scarlet is configured to support.
    let controller = alloc::boxed::Box::new(ArmGenericTimer::new());
    let _ = crate::interrupt::InterruptManager::global()
        .register_timer_controller_for_range(controller, 0..(MAX_NUM_CPUS as CpuId));

    DeviceManager::get_manager().register_driver(
        alloc::boxed::Box::new(PlatformDeviceDriver::new(
            "arm-generic-timer",
            platform_timer_probe,
            platform_timer_remove,
            alloc::vec!["arm,armv8-timer", "arm,armv7-timer"],
        )),
        DriverPriority::Critical,
    );
}

crate::early_initcall!(register_arm_generic_timer);

fn platform_timer_probe(device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    let irq_resources: alloc::vec::Vec<_> = device
        .get_resources()
        .iter()
        .filter(|resource| resource.res_type == PlatformDeviceResourceType::IRQ)
        .collect();

    let irq_name = if crate::arch::aarch64::is_vhe_enabled() {
        "hyp-phys"
    } else {
        "virt"
    };
    let fallback_index = if crate::arch::aarch64::is_vhe_enabled() {
        3
    } else {
        2
    };
    let irq_index = device
        .property("interrupt-names")
        .and_then(|property| property.as_string_list())
        .and_then(|names| names.iter().position(|name| *name == irq_name))
        .unwrap_or(fallback_index);
    let irq_resource = irq_resources
        .get(irq_index)
        .ok_or("ARM generic timer: selected IRQ is missing")?;

    let interrupt_id = match crate::interrupt::resolve_platform_irq(irq_resource) {
        Ok(interrupt_id) => interrupt_id,
        Err(InterruptError::ControllerNotFound) => return probe_defer(),
        Err(_) => return Err("ARM generic timer: failed to resolve timer IRQ"),
    };

    TIMER_PPI_IRQ.store(interrupt_id, Ordering::Release);
    crate::arch::interrupt::configure_timer_interrupt_route(
        crate::arch::interrupt::TimerInterruptRoute::ExternalControllerIrq,
        Some(interrupt_id),
    );
    crate::early_println!(
        "[interrupt] ARM generic timer: using {} PPI {} from firmware",
        irq_name,
        interrupt_id
    );
    Ok(())
}

fn platform_timer_remove(_device: &PlatformDeviceInfo) -> Result<(), &'static str> {
    Ok(())
}
