//! Timer interrupt handler implementation.
//!
//! This module provides the timer interrupt handler that integrates with
//! the new interrupt management system.

use alloc::boxed::Box;
use crate::arch::Trapframe;
use crate::interrupt::{InterruptHandler, InterruptPriority};
use crate::sched::scheduler::get_scheduler;

/// Timer interrupt handler
pub struct TimerInterruptHandler {
    name: &'static str,
}

impl TimerInterruptHandler {
    /// Create a new timer interrupt handler
    pub fn new() -> Self {
        Self {
            name: "timer",
        }
    }

    /// Create a boxed timer interrupt handler for registration
    pub fn boxed() -> Box<dyn InterruptHandler> {
        Box::new(Self::new())
    }
}

impl InterruptHandler for TimerInterruptHandler {
    fn handle(&mut self, irq: usize, trapframe: &mut Trapframe) -> Result<(), &'static str> {
        // Verify this is the timer interrupt
        if irq != crate::interrupt::irq::TIMER_INTERRUPT {
            return Err("Not a timer interrupt");
        }

        // Schedule the next task
        let scheduler = get_scheduler();
        scheduler.schedule(trapframe);
        
        Ok(())
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn is_interruptible(&self) -> bool {
        false // Timer interrupts should not be interrupted
    }
}

/// Initialize timer interrupt handling
pub fn init_timer_interrupt() -> Result<(), &'static str> {
    use crate::interrupt::manager::InterruptManager;
    use crate::interrupt::irq;

    // Register the timer interrupt handler
    let handler = TimerInterruptHandler::boxed();
    let priority: InterruptPriority = 5; // Medium priority
    let cpu_id = 0; // Start with CPU 0

    InterruptManager::register_global_handler(
        irq::TIMER_INTERRUPT,
        handler,
        priority,
        cpu_id,
    )?;

    crate::early_println!("[Timer] Timer interrupt handler registered");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::*;

    #[test_case]
    fn test_timer_interrupt_handler() {
        let mut handler = TimerInterruptHandler::new();
        assert_eq!(handler.name(), "timer");
        assert!(!handler.is_interruptible());

        // Create a dummy trapframe
        let mut dummy_trapframe = unsafe { core::mem::zeroed::<Trapframe>() };
        
        // Test with wrong IRQ
        let result = handler.handle(1, &mut dummy_trapframe);
        assert!(result.is_err());

        // Test with correct IRQ (would fail without scheduler initialization)
        // This test is mainly for checking the interface
    }
}
