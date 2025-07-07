//! Interrupt Controller implementations.
//!
//! This module contains implementations of various interrupt controllers
//! used in RISC-V systems, including PLIC, AIA, and CLINT.

use crate::arch::Trapframe;
use super::{InterruptController, InterruptPriority};

pub mod plic;
pub mod clint;

/// A simple software-based interrupt controller for testing and fallback
pub struct SoftwareInterruptController {
    name: &'static str,
    enabled_interrupts: [bool; super::MAX_INTERRUPTS],
    priorities: [InterruptPriority; super::MAX_INTERRUPTS],
    pending: [bool; super::MAX_INTERRUPTS],
}

impl SoftwareInterruptController {
    /// Create a new software interrupt controller
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            enabled_interrupts: [false; super::MAX_INTERRUPTS],
            priorities: [0; super::MAX_INTERRUPTS],
            pending: [false; super::MAX_INTERRUPTS],
        }
    }

    /// Trigger a software interrupt (for testing)
    pub fn trigger_interrupt(&mut self, irq: usize) {
        if irq < super::MAX_INTERRUPTS {
            self.pending[irq] = true;
        }
    }

    /// Clear a software interrupt
    pub fn clear_interrupt(&mut self, irq: usize) {
        if irq < super::MAX_INTERRUPTS {
            self.pending[irq] = false;
        }
    }
}

impl InterruptController for SoftwareInterruptController {
    fn init(&mut self) -> Result<(), &'static str> {
        // Software controller doesn't need hardware initialization
        crate::early_println!("[SoftwareIC] Initialized software interrupt controller: {}", self.name);
        Ok(())
    }

    fn enable_interrupt(&mut self, irq: usize, priority: InterruptPriority, _cpu_id: usize) -> Result<(), &'static str> {
        if irq >= super::MAX_INTERRUPTS {
            return Err("IRQ number out of range");
        }

        self.enabled_interrupts[irq] = true;
        self.priorities[irq] = priority;
        Ok(())
    }

    fn disable_interrupt(&mut self, irq: usize) -> Result<(), &'static str> {
        if irq >= super::MAX_INTERRUPTS {
            return Err("IRQ number out of range");
        }

        self.enabled_interrupts[irq] = false;
        Ok(())
    }

    fn set_priority(&mut self, irq: usize, priority: InterruptPriority) -> Result<(), &'static str> {
        if irq >= super::MAX_INTERRUPTS {
            return Err("IRQ number out of range");
        }

        self.priorities[irq] = priority;
        Ok(())
    }

    fn get_priority(&self, irq: usize) -> Result<InterruptPriority, &'static str> {
        if irq >= super::MAX_INTERRUPTS {
            return Err("IRQ number out of range");
        }

        Ok(self.priorities[irq])
    }

    fn claim_interrupt(&mut self) -> Option<usize> {
        // Find the highest priority pending interrupt
        let mut highest_priority = 0;
        let mut selected_irq = None;

        for irq in 0..super::MAX_INTERRUPTS {
            if self.pending[irq] && self.enabled_interrupts[irq] {
                if selected_irq.is_none() || self.priorities[irq] > highest_priority {
                    highest_priority = self.priorities[irq];
                    selected_irq = Some(irq);
                }
            }
        }

        selected_irq
    }

    fn complete_interrupt(&mut self, irq: usize) -> Result<(), &'static str> {
        if irq >= super::MAX_INTERRUPTS {
            return Err("IRQ number out of range");
        }

        self.pending[irq] = false;
        Ok(())
    }

    fn is_pending(&self, irq: usize) -> bool {
        if irq < super::MAX_INTERRUPTS {
            self.pending[irq] && self.enabled_interrupts[irq]
        } else {
            false
        }
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::*;

    #[test_case]
    fn test_software_interrupt_controller() {
        let mut controller = SoftwareInterruptController::new("test_sw_ic");
        
        // Test initialization
        assert!(controller.init().is_ok());
        assert_eq!(controller.name(), "test_sw_ic");

        // Test enabling interrupt
        assert!(controller.enable_interrupt(5, 10, 0).is_ok());
        assert_eq!(controller.get_priority(5).unwrap(), 10);

        // Test triggering interrupt
        controller.trigger_interrupt(5);
        assert!(controller.is_pending(5));

        // Test claiming interrupt
        let claimed = controller.claim_interrupt();
        assert_eq!(claimed, Some(5));

        // Test completing interrupt
        assert!(controller.complete_interrupt(5).is_ok());
        assert!(!controller.is_pending(5));

        // Test disabling interrupt
        controller.trigger_interrupt(5);
        assert!(controller.disable_interrupt(5).is_ok());
        assert!(!controller.is_pending(5));
    }

    #[test_case]
    fn test_priority_handling() {
        let mut controller = SoftwareInterruptController::new("priority_test");
        controller.init().unwrap();

        // Register multiple interrupts with different priorities
        controller.enable_interrupt(1, 5, 0).unwrap();
        controller.enable_interrupt(2, 10, 0).unwrap();
        controller.enable_interrupt(3, 1, 0).unwrap();

        // Trigger all interrupts
        controller.trigger_interrupt(1);
        controller.trigger_interrupt(2);
        controller.trigger_interrupt(3);

        // Should claim highest priority first (IRQ 2, priority 10)
        assert_eq!(controller.claim_interrupt(), Some(2));
        controller.complete_interrupt(2).unwrap();

        // Next should be IRQ 1 (priority 5)
        assert_eq!(controller.claim_interrupt(), Some(1));
        controller.complete_interrupt(1).unwrap();

        // Finally IRQ 3 (priority 1)
        assert_eq!(controller.claim_interrupt(), Some(3));
        controller.complete_interrupt(3).unwrap();

        // No more interrupts pending
        assert_eq!(controller.claim_interrupt(), None);
    }
}
