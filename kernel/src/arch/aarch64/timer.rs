//! AArch64 timer implementation
//!
//! Timer functionality for AArch64 architecture.

use core::arch::asm;

use crate::{arch::get_cpu, arch::interrupt};

pub fn timer_init() {
    // Local controller registration happens via early initcall.
}

/// Allow EL0 code to read architectural counter registers.
///
/// Linux AArch64 userspace commonly reads `CNTVCT_EL0` directly for fast time
/// sampling. Keep timer control registers trapped, but expose virtual and
/// physical counter reads so libc/runtime code does not fault on `mrs`.
pub fn enable_el0_counter_access() {
    const CNTKCTL_EL1_EL0PCTEN: u64 = 1 << 0;
    const CNTKCTL_EL1_EL0VCTEN: u64 = 1 << 1;

    let mut cntkctl: u64;
    unsafe {
        asm!("mrs {0}, cntkctl_el1", out(reg) cntkctl, options(nostack));
        cntkctl |= CNTKCTL_EL1_EL0PCTEN | CNTKCTL_EL1_EL0VCTEN;
        asm!(
            "msr cntkctl_el1, {0}",
            "isb",
            in(reg) cntkctl,
            options(nostack)
        );
    }
}

pub fn get_time() -> u64 {
    let cpu_id = get_cpu().get_cpuid() as u32;
    crate::interrupt::InterruptManager::global()
        .get_time(cpu_id)
        .unwrap_or(0)
}

pub fn set_timer(_time: u64) {
    let cpu_id = get_cpu().get_cpuid() as u32;
    let _ = crate::interrupt::InterruptManager::global().set_timer(cpu_id, _time);
}

#[inline]
fn saturating_u128_to_u64(value: u128) -> u64 {
    value.min(u64::MAX as u128) as u64
}

pub struct ArchTimer {
    next_event: u64,
    running: bool,
    frequency: u64,
}

impl ArchTimer {
    pub fn new() -> Self {
        let cpu_id = get_cpu().get_cpuid() as u32;
        let freq = crate::interrupt::InterruptManager::global()
            .get_timer_frequency_hz(cpu_id)
            .unwrap_or(0);

        ArchTimer {
            next_event: 0,
            running: false,
            frequency: freq,
        }
    }

    pub fn init(&self) {
        // Nothing to do; the controller initializes itself.
    }

    pub fn get_time(&self) -> u64 {
        let cpu_id = get_cpu().get_cpuid() as u32;
        crate::interrupt::InterruptManager::global()
            .get_time(cpu_id)
            .unwrap_or(0)
    }

    pub fn set_timer(&self, time: u64) {
        let cpu_id = get_cpu().get_cpuid() as u32;
        let _ = crate::interrupt::InterruptManager::global().set_timer(cpu_id, time);
    }

    pub fn start(&mut self) {
        self.running = true;

        // The common scheduler code enables interrupts *before* calling timer.start().
        // Open the gate here so subsequent enable calls can actually unmask.
        crate::arch::mark_interrupts_allowed();

        // Avoid programming a compare value in the past. This can create an immediate
        // interrupt storm (especially when the requested interval is 0).
        let current_time = self.get_time();
        let mut next = self.get_next_event();
        if next <= current_time {
            next = current_time.wrapping_add(1);
            self.set_next_event(next);
        }

        // Program the next event before unmasking interrupts.
        self.set_timer(next);

        // stop() disables both the local timer interrupt and its external PPI
        // route, so every start must restore the controller state. This is
        // intentionally idempotent for the normal per-tick rearm path.
        interrupt::enable_arch_timer_interrupt()
            .unwrap_or_else(|e| panic!("Failed to enable timer interrupt: {e}"));

        interrupt::enable_timer_source_interrupt();
    }

    pub fn stop(&mut self) {
        self.running = false;

        let _ = interrupt::disable_arch_timer_interrupt();

        let cpu_id = get_cpu().get_cpuid() as u32;
        let _ = crate::interrupt::InterruptManager::global().set_timer(cpu_id, u64::MAX);
    }

    pub fn set_interval_us(&mut self, interval_us: u64) {
        let current = self.get_time();

        // If frequency isn't available yet (controller missing), keep behavior safe.
        if self.frequency == 0 {
            self.next_event = current;
            return;
        }

        let delta = (interval_us.saturating_mul(self.frequency)) / 1_000_000;
        self.set_next_event(current.saturating_add(delta));
    }

    /// Program the next timer event at an absolute hardware-counter deadline.
    ///
    /// # Arguments
    ///
    /// * `deadline` - Absolute hardware-counter value to fire at.
    pub fn set_deadline(&mut self, deadline: u64) {
        self.set_next_event(deadline);
    }

    /// Program an absolute monotonic nanosecond deadline.
    ///
    /// Expired or very-near deadlines are moved at least one microsecond into
    /// the future so a stale queue head cannot create an interrupt storm.
    pub fn set_deadline_ns(&mut self, deadline_ns: u64) {
        let now = self.get_time();
        if self.frequency == 0 {
            self.set_next_event(now.saturating_add(1));
            return;
        }

        let deadline = saturating_u128_to_u64(
            (deadline_ns as u128).saturating_mul(self.frequency as u128) / 1_000_000_000,
        );
        let minimum_delta = self.frequency.div_ceil(1_000_000).max(1);
        self.set_next_event(deadline.max(now.saturating_add(minimum_delta)));
    }

    pub fn get_time_us(&self) -> u64 {
        let now = self.get_time();
        if self.frequency == 0 {
            return 0;
        }
        (now.saturating_mul(1_000_000)) / self.frequency
    }

    pub fn get_time_ns(&self) -> u64 {
        if self.frequency == 0 {
            return 0;
        }
        saturating_u128_to_u64(
            (self.get_time() as u128).saturating_mul(1_000_000_000) / self.frequency as u128,
        )
    }

    fn get_next_event(&self) -> u64 {
        self.next_event
    }

    fn set_next_event(&mut self, next_event: u64) {
        self.next_event = next_event;
    }
}

#[cfg(test)]
mod tests {
    use super::saturating_u128_to_u64;

    #[test_case]
    fn counter_conversion_saturates_instead_of_truncating() {
        assert_eq!(saturating_u128_to_u64(u64::MAX as u128), u64::MAX);
        assert_eq!(saturating_u128_to_u64((u64::MAX as u128) + 1), u64::MAX);
    }
}
