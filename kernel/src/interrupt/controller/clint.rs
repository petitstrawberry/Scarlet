//! Core Local Interruptor (CLINT) implementation.
//!
//! CLINT handles timer and software interrupts for RISC-V systems.
//! It provides per-core timer and software interrupt functionality.

use super::{InterruptController, InterruptPriority};
use crate::environment::NUM_OF_CPUS;

/// Core Local Interruptor
pub struct Clint {
    base_addr: usize,
}

impl Clint {
    /// Create a new CLINT controller
    pub fn new(base_addr: usize) -> Self {
        Self { base_addr }
    }

    /// Set the base address
    pub fn set_base_addr(&mut self, base_addr: usize) {
        self.base_addr = base_addr;
    }
}

impl InterruptController for Clint {
    fn init(&mut self) -> Result<(), &'static str> {
        if self.base_addr == 0 {
            return Err("CLINT base address not set");
        }

        crate::early_println!("[CLINT] Initializing CLINT at {:#x}", self.base_addr);
        Ok(())
    }

    fn enable_interrupt(&mut self, irq: usize, _priority: InterruptPriority, _cpu_id: usize) -> Result<(), &'static str> {
        match irq {
            1 => Ok(()), // Software interrupt
            5 => Ok(()), // Timer interrupt
            _ => Err("CLINT only handles software and timer interrupts"),
        }
    }

    fn disable_interrupt(&mut self, irq: usize) -> Result<(), &'static str> {
        match irq {
            1 => Ok(()), // Software interrupt
            5 => Ok(()), // Timer interrupt
            _ => Err("CLINT only handles software and timer interrupts"),
        }
    }

    fn set_priority(&mut self, irq: usize, _priority: InterruptPriority) -> Result<(), &'static str> {
        match irq {
            1 | 5 => Ok(()), // CLINT interrupts have fixed priority
            _ => Err("CLINT only handles software and timer interrupts"),
        }
    }

    fn get_priority(&self, irq: usize) -> Result<InterruptPriority, &'static str> {
        match irq {
            1 => Ok(1), // Software interrupt priority
            5 => Ok(5), // Timer interrupt priority
            _ => Err("CLINT only handles software and timer interrupts"),
        }
    }

    fn claim_interrupt(&mut self) -> Option<usize> {
        // CLINT interrupts are handled directly by the CPU
        // This is mainly for compatibility with the InterruptController trait
        None
    }

    fn complete_interrupt(&mut self, irq: usize) -> Result<(), &'static str> {
        match irq {
            1 | 5 => Ok(()), // No explicit completion needed for CLINT
            _ => Err("CLINT only handles software and timer interrupts"),
        }
    }

    fn is_pending(&self, _irq: usize) -> bool {
        // CLINT interrupt status is checked via CSRs, not memory-mapped registers
        false
    }

    fn name(&self) -> &'static str {
        "CLINT"
    }

    fn supports_cpu_routing(&self) -> bool {
        true // CLINT naturally supports per-CPU interrupts
    }

    fn route_to_cpu(&mut self, irq: usize, _cpu_id: usize) -> Result<(), &'static str> {
        match irq {
            1 | 5 => Ok(()), // CLINT interrupts are inherently per-CPU
            _ => Err("CLINT only handles software and timer interrupts"),
        }
    }
}
