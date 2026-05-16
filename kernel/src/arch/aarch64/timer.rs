//! AArch64 timer implementation
//!
//! Timer functionality for AArch64 architecture.

use core::arch::asm;

use crate::{arch::get_cpu, arch::interrupt};

pub fn timer_init() {
    // Local controller registration happens via early initcall.
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

pub struct ArchTimer {
    next_event: u64,
    running: bool,
    frequency: u64,
    initialized: bool,
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
            initialized: false,
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

        // Only perform interrupt controller configuration on first start
        if !self.initialized {
            interrupt::disable_external_interrupts();

            interrupt::enable_arch_timer_interrupt()
                .unwrap_or_else(|e| panic!("Failed to enable timer interrupt: {e}"));

            self.initialized = true;

            interrupt::enable_external_interrupts();
        }

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

    pub fn get_time_us(&self) -> u64 {
        let now = self.get_time();
        if self.frequency == 0 {
            return 0;
        }
        (now.saturating_mul(1_000_000)) / self.frequency
    }

    fn get_next_event(&self) -> u64 {
        self.next_event
    }

    fn set_next_event(&mut self, next_event: u64) {
        self.next_event = next_event;
    }
}
