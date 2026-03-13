//! AArch64 timer implementation
//!
//! Timer functionality for AArch64 architecture.

use core::arch::asm;

use crate::{
    arch::get_cpu,
    arch::interrupt,
    interrupt::{InterruptManager, controllers::LocalInterruptType},
};

pub fn timer_init() {
    // Local controller registration happens via early initcall.
}

pub fn get_time() -> u64 {
    InterruptManager::with_manager(|mgr| {
        let cpu_id = get_cpu().get_cpuid() as u32;
        mgr.get_time(cpu_id).unwrap_or(0)
    })
}

pub fn set_timer(_time: u64) {
    InterruptManager::with_manager(|mgr| {
        let cpu_id = get_cpu().get_cpuid() as u32;
        let _ = mgr.set_timer(cpu_id, _time);
    });
}

pub struct ArchTimer {
    next_event: u64,
    running: bool,
    frequency: u64,
    initialized: bool,
}

impl ArchTimer {
    pub fn new() -> Self {
        let freq = InterruptManager::with_manager(|mgr| {
            let cpu_id = get_cpu().get_cpuid() as u32;
            mgr.get_timer_frequency_hz(cpu_id).unwrap_or(0)
        });

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
        InterruptManager::with_manager(|mgr| {
            let cpu_id = get_cpu().get_cpuid() as u32;
            mgr.get_time(cpu_id).unwrap_or(0)
        })
    }

    pub fn set_timer(&self, time: u64) {
        InterruptManager::with_manager(|mgr| {
            let cpu_id = get_cpu().get_cpuid() as u32;
            let _ = mgr.set_timer(cpu_id, time);
        });
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

        // Only perform controller-independent interrupt configuration on first start
        if !self.initialized {
            // CRITICAL: Mask IRQs before configuring the interrupt controller to avoid deadlock
            // (an interrupt firing during InterruptManager access could try to re-lock it).
            interrupt::disable_external_interrupts();

            // Enable the timer at the core-local interrupt source.
            //
            // Architectures/controllers that need an additional routing step are
            // expected to have prepared it during controller initialization.
            interrupt::enable_core_local_interrupt(LocalInterruptType::Timer)
                .unwrap_or_else(|e| panic!("Failed to enable local timer interrupt: {e}"));

            // CRITICAL: Set initialized flag BEFORE unmasking interrupts
            // Otherwise, if an interrupt fires immediately after unmask, it will
            // see initialized=false and reconfigure GIC again
            self.initialized = true;

            // Ensure IRQ is unmasked at CPU level (first time only)
            interrupt::enable_external_interrupts();
        }

        // Finally, unmask the timer interrupt at the timer source itself.
        // (This is analogous to a per-source enable bit like RISC-V STIE.)
        interrupt::enable_timer_source_interrupt();
        // Note: Subsequent calls just update CVAL, no DAIF/GIC manipulation
        // This prevents nested interrupts during tick handling
    }

    pub fn stop(&mut self) {
        self.running = false;

        InterruptManager::with_manager(|mgr| {
            let cpu_id = get_cpu().get_cpuid() as u32;
            let _ = mgr.disable_local_interrupt(cpu_id, LocalInterruptType::Timer);
            let _ = mgr.set_timer(cpu_id, u64::MAX);
        });
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
