//! AArch64 timer implementation
//!
//! Timer functionality for AArch64 architecture.

use core::arch::asm;

use crate::{
    arch::get_cpu,
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

        crate::early_println!("[Timer] start() called: self={:p} next_event={:#x} initialized={}", 
            self as *const _, self.next_event, self.initialized);
        
        // Read current timer value before setting compare value
        let current_time = self.get_time();
        crate::early_println!("[Timer] Current timer count: {:#x}, compare value: {:#x}, diff: {}",
            current_time, self.next_event, self.next_event.wrapping_sub(current_time) as i64);
        
        // Program the next event before unmasking interrupts.
        self.set_timer(self.get_next_event());
        
        // Only perform GIC configuration on first start
        if !self.initialized {
            crate::early_println!("[Timer] First start - configuring GIC");
            
            // CRITICAL: Mask interrupts before configuring GIC to avoid deadlock
            // (interrupt firing during GIC config would try to re-lock InterruptManager)
            unsafe {
                asm!("msr daifset, #2", options(nostack));
            }
            crate::early_println!("[Timer] Interrupts masked during GIC configuration");

            // Enable timer local interrupt and ensure the corresponding PPI is enabled in the GIC.
            InterruptManager::with_manager(|mgr| {
                let cpu_id = get_cpu().get_cpuid() as u32;
                crate::early_println!("[Timer] Enabling local timer interrupt");
                mgr.enable_local_interrupt(cpu_id, LocalInterruptType::Timer)
                    .unwrap_or_else(|e| panic!("Failed to enable local timer interrupt: {e}"));

                // QEMU virt: CNTP PPI is 30.
                // PPIs are banked per-CPU but still need to be enabled in GIC distributor.
                crate::early_println!("[Timer] Enabling PPI 30 in GIC distributor");
                mgr.enable_external_interrupt(
                    crate::drivers::pic::arm_generic_timer::CNTP_PPI_IRQ,
                    cpu_id,
                )
                .unwrap_or_else(|e| panic!("Failed to enable timer PPI in GIC: {e}"));
            });
            
            crate::early_println!("[Timer] GIC configuration complete");

            // CRITICAL: Set initialized flag BEFORE unmasking interrupts
            // Otherwise, if an interrupt fires immediately after unmask, it will
            // see initialized=false and reconfigure GIC again
            self.initialized = true;

            // Ensure IRQ is unmasked at CPU level (first time only)
            unsafe {
                asm!("msr daifclr, #2", options(nostack));
            }
            
            crate::early_println!("[Timer] Timer initialization complete, self={:p} initialized={}", 
                self as *const _, self.initialized);
        } else {
            crate::early_println!("[Timer] Skipping GIC config (already initialized)");
        }
        // Note: Subsequent calls just update CVAL, no DAIF/GIC manipulation
        // This prevents nested interrupts during tick handling
    }

    pub fn stop(&mut self) {
        self.running = false;

        InterruptManager::with_manager(|mgr| {
            let cpu_id = get_cpu().get_cpuid() as u32;
            let _ = mgr.disable_local_interrupt(cpu_id, LocalInterruptType::Timer);
            let _ = mgr.disable_external_interrupt(
                crate::drivers::pic::arm_generic_timer::CNTP_PPI_IRQ,
                cpu_id,
            );
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
