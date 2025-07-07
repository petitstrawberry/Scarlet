//! Platform-Level Interrupt Controller (PLIC) implementation.
//!
//! The PLIC is the standard interrupt controller for RISC-V systems.
//! It manages external interrupts and routes them to different CPU cores
//! based on priority and configuration.
//!
//! PLIC Memory Map:
//! - 0x0000_0000: Reserved
//! - 0x0000_0004: Interrupt source 1 priority
//! - 0x0000_0008: Interrupt source 2 priority
//! - ...
//! - 0x0000_1000: Interrupt pending bits
//! - 0x0000_2000: Interrupt enable bits for context 0
//! - 0x0000_2080: Interrupt enable bits for context 1
//! - ...
//! - 0x0020_0000: Priority threshold for context 0
//! - 0x0020_0004: Claim/complete for context 0
//! - 0x0020_1000: Priority threshold for context 1
//! - 0x0020_1004: Claim/complete for context 1

use core::ptr::{read_volatile, write_volatile};
use crate::environment::NUM_OF_CPUS;
use super::{InterruptController, InterruptPriority};

/// PLIC register offsets
const PLIC_PRIORITY_OFFSET: usize = 0x0000_0000;
const PLIC_PENDING_OFFSET: usize = 0x0000_1000;
const PLIC_ENABLE_OFFSET: usize = 0x0000_2000;
const PLIC_ENABLE_PER_CONTEXT: usize = 0x80;
const PLIC_CONTEXT_OFFSET: usize = 0x0020_0000;
const PLIC_CONTEXT_PER_CONTEXT: usize = 0x1000;
const PLIC_THRESHOLD_OFFSET: usize = 0x0000;
const PLIC_CLAIM_OFFSET: usize = 0x0004;

/// Maximum number of interrupt sources supported by PLIC
const PLIC_MAX_SOURCES: usize = 1024;

/// Maximum priority level
const PLIC_MAX_PRIORITY: u32 = 7;

/// PLIC context represents a CPU core in a specific privilege mode
#[derive(Debug, Clone, Copy)]
pub struct PlicContext {
    pub cpu_id: usize,
    pub is_machine_mode: bool,
}

impl PlicContext {
    pub fn supervisor(cpu_id: usize) -> Self {
        Self {
            cpu_id,
            is_machine_mode: false,
        }
    }

    pub fn machine(cpu_id: usize) -> Self {
        Self {
            cpu_id,
            is_machine_mode: true,
        }
    }

    /// Get the context ID for register calculations
    fn context_id(&self) -> usize {
        // Assuming interleaved contexts: S-mode context 0, M-mode context 1, S-mode context 2, etc.
        self.cpu_id * 2 + if self.is_machine_mode { 1 } else { 0 }
    }
}

/// Platform-Level Interrupt Controller
pub struct Plic {
    base_addr: usize,
    max_sources: usize,
    contexts: [PlicContext; NUM_OF_CPUS],
}

impl Plic {
    /// Create a new PLIC controller
    ///
    /// # Arguments
    /// * `base_addr` - Base address of PLIC registers in memory
    /// * `max_sources` - Maximum number of interrupt sources (default: 53 for QEMU virt)
    pub fn new(base_addr: usize, max_sources: Option<usize>) -> Self {
        let max_sources = max_sources.unwrap_or(53); // QEMU virt default
        
        // Initialize contexts for supervisor mode on each CPU
        let mut contexts = [PlicContext::supervisor(0); NUM_OF_CPUS];
        for i in 0..NUM_OF_CPUS {
            contexts[i] = PlicContext::supervisor(i);
        }

        Self {
            base_addr,
            max_sources: core::cmp::min(max_sources, PLIC_MAX_SOURCES),
            contexts,
        }
    }

    /// Set the base address (useful for device tree initialization)
    pub fn set_base_addr(&mut self, base_addr: usize) {
        self.base_addr = base_addr;
    }

    /// Get priority register address for a source
    fn priority_addr(&self, source: usize) -> usize {
        self.base_addr + PLIC_PRIORITY_OFFSET + source * 4
    }

    /// Get pending register address
    fn pending_addr(&self, word: usize) -> usize {
        self.base_addr + PLIC_PENDING_OFFSET + word * 4
    }

    /// Get enable register address for a context
    fn enable_addr(&self, context: PlicContext, word: usize) -> usize {
        self.base_addr + PLIC_ENABLE_OFFSET + 
        context.context_id() * PLIC_ENABLE_PER_CONTEXT + word * 4
    }

    /// Get threshold register address for a context
    fn threshold_addr(&self, context: PlicContext) -> usize {
        self.base_addr + PLIC_CONTEXT_OFFSET + 
        context.context_id() * PLIC_CONTEXT_PER_CONTEXT + PLIC_THRESHOLD_OFFSET
    }

    /// Get claim/complete register address for a context
    fn claim_addr(&self, context: PlicContext) -> usize {
        self.base_addr + PLIC_CONTEXT_OFFSET + 
        context.context_id() * PLIC_CONTEXT_PER_CONTEXT + PLIC_CLAIM_OFFSET
    }

    /// Set interrupt priority for a specific source
    pub fn set_source_priority(&mut self, source: usize, priority: u32) -> Result<(), &'static str> {
        if source == 0 || source > self.max_sources {
            return Err("Invalid interrupt source");
        }
        if priority > PLIC_MAX_PRIORITY {
            return Err("Priority too high");
        }

        let addr = self.priority_addr(source);
        unsafe {
            write_volatile(addr as *mut u32, priority);
        }
        Ok(())
    }

    /// Get interrupt priority for a specific source
    pub fn get_source_priority(&self, source: usize) -> Result<u32, &'static str> {
        if source == 0 || source > self.max_sources {
            return Err("Invalid interrupt source");
        }

        let addr = self.priority_addr(source);
        let priority = unsafe { read_volatile(addr as *const u32) };
        Ok(priority)
    }

    /// Enable interrupt source for a specific context
    pub fn enable_source(&mut self, source: usize, context: PlicContext) -> Result<(), &'static str> {
        if source == 0 || source > self.max_sources {
            return Err("Invalid interrupt source");
        }

        let word = source / 32;
        let bit = source % 32;
        let addr = self.enable_addr(context, word);

        unsafe {
            let current = read_volatile(addr as *const u32);
            write_volatile(addr as *mut u32, current | (1 << bit));
        }
        Ok(())
    }

    /// Disable interrupt source for a specific context
    pub fn disable_source(&mut self, source: usize, context: PlicContext) -> Result<(), &'static str> {
        if source == 0 || source > self.max_sources {
            return Err("Invalid interrupt source");
        }

        let word = source / 32;
        let bit = source % 32;
        let addr = self.enable_addr(context, word);

        unsafe {
            let current = read_volatile(addr as *const u32);
            write_volatile(addr as *mut u32, current & !(1 << bit));
        }
        Ok(())
    }

    /// Set priority threshold for a context
    pub fn set_threshold(&mut self, context: PlicContext, threshold: u32) -> Result<(), &'static str> {
        if threshold > PLIC_MAX_PRIORITY {
            return Err("Threshold too high");
        }

        let addr = self.threshold_addr(context);
        unsafe {
            write_volatile(addr as *mut u32, threshold);
        }
        Ok(())
    }

    /// Claim the next pending interrupt for a context
    pub fn claim(&mut self, context: PlicContext) -> Option<usize> {
        let addr = self.claim_addr(context);
        let source = unsafe { read_volatile(addr as *const u32) };
        
        if source == 0 {
            None
        } else {
            Some(source as usize)
        }
    }

    /// Complete interrupt processing for a source and context
    pub fn complete(&mut self, source: usize, context: PlicContext) -> Result<(), &'static str> {
        if source == 0 || source > self.max_sources {
            return Err("Invalid interrupt source");
        }

        let addr = self.claim_addr(context);
        unsafe {
            write_volatile(addr as *mut u32, source as u32);
        }
        Ok(())
    }

    /// Check if a source is pending
    pub fn is_source_pending(&self, source: usize) -> bool {
        if source == 0 || source > self.max_sources {
            return false;
        }

        let word = source / 32;
        let bit = source % 32;
        let addr = self.pending_addr(word);

        let pending = unsafe { read_volatile(addr as *const u32) };
        (pending & (1 << bit)) != 0
    }

    /// Get the context for a specific CPU (default to supervisor mode)
    fn get_cpu_context(&self, cpu_id: usize) -> PlicContext {
        if cpu_id < NUM_OF_CPUS {
            self.contexts[cpu_id]
        } else {
            PlicContext::supervisor(0) // Fallback to CPU 0
        }
    }
}

impl InterruptController for Plic {
    fn init(&mut self) -> Result<(), &'static str> {
        if self.base_addr == 0 {
            return Err("PLIC base address not set");
        }

        crate::early_println!("[PLIC] Initializing PLIC at {:#x}", self.base_addr);
        crate::early_println!("[PLIC] Supporting {} interrupt sources", self.max_sources);

        // Set threshold to 0 for all contexts to receive all interrupts
        for cpu_id in 0..NUM_OF_CPUS {
            let context = self.get_cpu_context(cpu_id);
            self.set_threshold(context, 0)?;
        }

        Ok(())
    }

    fn enable_interrupt(&mut self, irq: usize, priority: InterruptPriority, cpu_id: usize) -> Result<(), &'static str> {
        if irq == 0 || irq > self.max_sources {
            return Err("Invalid IRQ number for PLIC");
        }

        let plic_priority = core::cmp::min(priority, PLIC_MAX_PRIORITY);
        let context = self.get_cpu_context(cpu_id);

        self.set_source_priority(irq, plic_priority)?;
        self.enable_source(irq, context)?;

        Ok(())
    }

    fn disable_interrupt(&mut self, irq: usize) -> Result<(), &'static str> {
        if irq == 0 || irq > self.max_sources {
            return Err("Invalid IRQ number for PLIC");
        }

        // Disable for all contexts
        for cpu_id in 0..NUM_OF_CPUS {
            let context = self.get_cpu_context(cpu_id);
            self.disable_source(irq, context)?;
        }

        Ok(())
    }

    fn set_priority(&mut self, irq: usize, priority: InterruptPriority) -> Result<(), &'static str> {
        if irq == 0 || irq > self.max_sources {
            return Err("Invalid IRQ number for PLIC");
        }

        let plic_priority = core::cmp::min(priority, PLIC_MAX_PRIORITY);
        self.set_source_priority(irq, plic_priority)
    }

    fn get_priority(&self, irq: usize) -> Result<InterruptPriority, &'static str> {
        if irq == 0 || irq > self.max_sources {
            return Err("Invalid IRQ number for PLIC");
        }

        self.get_source_priority(irq)
    }

    fn claim_interrupt(&mut self) -> Option<usize> {
        // Try to claim from CPU 0 context for now
        // In a multi-CPU system, this should check the current CPU
        let context = self.get_cpu_context(0);
        self.claim(context)
    }

    fn complete_interrupt(&mut self, irq: usize) -> Result<(), &'static str> {
        if irq == 0 || irq > self.max_sources {
            return Err("Invalid IRQ number for PLIC");
        }

        // Complete for CPU 0 context for now
        let context = self.get_cpu_context(0);
        self.complete(irq, context)
    }

    fn is_pending(&self, irq: usize) -> bool {
        self.is_source_pending(irq)
    }

    fn name(&self) -> &'static str {
        "PLIC"
    }

    fn supports_cpu_routing(&self) -> bool {
        true
    }

    fn route_to_cpu(&mut self, irq: usize, cpu_id: usize) -> Result<(), &'static str> {
        if irq == 0 || irq > self.max_sources {
            return Err("Invalid IRQ number for PLIC");
        }

        if cpu_id >= NUM_OF_CPUS {
            return Err("Invalid CPU ID");
        }

        // Disable for all other CPUs and enable for target CPU
        for i in 0..NUM_OF_CPUS {
            let context = self.get_cpu_context(i);
            if i == cpu_id {
                self.enable_source(irq, context)?;
            } else {
                self.disable_source(irq, context)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::*;

    #[test_case]
    fn test_plic_context() {
        let ctx_s = PlicContext::supervisor(0);
        assert_eq!(ctx_s.cpu_id, 0);
        assert!(!ctx_s.is_machine_mode);
        assert_eq!(ctx_s.context_id(), 0);

        let ctx_m = PlicContext::machine(0);
        assert_eq!(ctx_m.cpu_id, 0);
        assert!(ctx_m.is_machine_mode);
        assert_eq!(ctx_m.context_id(), 1);

        let ctx_s1 = PlicContext::supervisor(1);
        assert_eq!(ctx_s1.context_id(), 2);
    }

    #[test_case]
    fn test_plic_creation() {
        let plic = Plic::new(0x0c000000, Some(53));
        assert_eq!(plic.base_addr, 0x0c000000);
        assert_eq!(plic.max_sources, 53);
        assert_eq!(plic.name(), "PLIC");
        assert!(plic.supports_cpu_routing());
    }

    #[test_case]
    fn test_plic_address_calculation() {
        let plic = Plic::new(0x0c000000, Some(53));
        
        // Test priority address
        assert_eq!(plic.priority_addr(1), 0x0c000004);
        assert_eq!(plic.priority_addr(2), 0x0c000008);

        // Test context addresses
        let ctx = PlicContext::supervisor(0);
        assert_eq!(plic.threshold_addr(ctx), 0x0c200000);
        assert_eq!(plic.claim_addr(ctx), 0x0c200004);
    }
}
