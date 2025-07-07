//! Interrupt controller trait definitions
//! 
//! This module defines the basic traits for local and external interrupt controllers.

use super::{InterruptId, CpuId, Priority, InterruptResult};
use alloc::boxed::Box;

/// Trait for local interrupt controllers (like CLINT)
/// 
/// Local interrupt controllers manage CPU-local interrupts such as timer interrupts
/// and software interrupts.
pub trait LocalInterruptController: Send + Sync {
    /// Initialize the local interrupt controller for a specific CPU
    fn init(&mut self, cpu_id: CpuId) -> InterruptResult<()>;

    /// Enable a specific local interrupt type for a CPU
    fn enable_interrupt(&mut self, cpu_id: CpuId, interrupt_type: LocalInterruptType) -> InterruptResult<()>;

    /// Disable a specific local interrupt type for a CPU
    fn disable_interrupt(&mut self, cpu_id: CpuId, interrupt_type: LocalInterruptType) -> InterruptResult<()>;

    /// Check if a specific local interrupt type is pending for a CPU
    fn is_pending(&self, cpu_id: CpuId, interrupt_type: LocalInterruptType) -> bool;

    /// Clear a pending local interrupt for a CPU
    fn clear_interrupt(&mut self, cpu_id: CpuId, interrupt_type: LocalInterruptType) -> InterruptResult<()>;

    /// Send a software interrupt to a specific CPU
    fn send_software_interrupt(&mut self, target_cpu: CpuId) -> InterruptResult<()>;

    /// Clear a software interrupt for a specific CPU
    fn clear_software_interrupt(&mut self, cpu_id: CpuId) -> InterruptResult<()>;

    /// Set timer interrupt for a specific CPU
    fn set_timer(&mut self, cpu_id: CpuId, time: u64) -> InterruptResult<()>;

    /// Get current timer value
    fn get_time(&self) -> u64;
}

/// Trait for external interrupt controllers (like PLIC)
/// 
/// External interrupt controllers manage interrupts from external devices
/// and can route them to different CPUs with priority support.
pub trait ExternalInterruptController: Send + Sync {
    /// Initialize the external interrupt controller
    fn init(&mut self) -> InterruptResult<()>;

    /// Enable a specific interrupt for a CPU
    fn enable_interrupt(&mut self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()>;

    /// Disable a specific interrupt for a CPU
    fn disable_interrupt(&mut self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()>;

    /// Set priority for a specific interrupt
    fn set_priority(&mut self, interrupt_id: InterruptId, priority: Priority) -> InterruptResult<()>;

    /// Get priority for a specific interrupt
    fn get_priority(&self, interrupt_id: InterruptId) -> InterruptResult<Priority>;

    /// Set priority threshold for a CPU
    fn set_threshold(&mut self, cpu_id: CpuId, threshold: Priority) -> InterruptResult<()>;

    /// Get priority threshold for a CPU
    fn get_threshold(&self, cpu_id: CpuId) -> InterruptResult<Priority>;

    /// Claim an interrupt (acknowledge and get the interrupt ID)
    fn claim_interrupt(&mut self, cpu_id: CpuId) -> InterruptResult<Option<InterruptId>>;

    /// Complete an interrupt (signal that handling is finished)
    fn complete_interrupt(&mut self, cpu_id: CpuId, interrupt_id: InterruptId) -> InterruptResult<()>;

    /// Check if a specific interrupt is pending
    fn is_pending(&self, interrupt_id: InterruptId) -> bool;

    /// Get the maximum number of interrupts supported
    fn max_interrupts(&self) -> InterruptId;

    /// Get the number of CPUs supported
    fn max_cpus(&self) -> CpuId;
}

/// Types of local interrupts
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalInterruptType {
    /// Timer interrupt
    Timer,
    /// Software interrupt
    Software,
    /// External interrupt (from PLIC)
    External,
}

/// Interrupt controller registry
/// 
/// This struct maintains references to the active interrupt controllers
/// and provides a unified interface for interrupt management.
/// Supports multiple local interrupt controllers for different CPU groups.
pub struct InterruptControllers {
    local_controllers: alloc::vec::Vec<Box<dyn LocalInterruptController>>,
    external_controller: Option<Box<dyn ExternalInterruptController>>,
    cpu_to_local_controller: alloc::collections::BTreeMap<CpuId, usize>, // CPU ID -> controller index
}

unsafe impl Send for InterruptControllers {}
unsafe impl Sync for InterruptControllers {}

impl InterruptControllers {
    /// Create a new interrupt controller registry
    pub fn new() -> Self {
        Self {
            local_controllers: alloc::vec::Vec::new(),
            external_controller: None,
            cpu_to_local_controller: alloc::collections::BTreeMap::new(),
        }
    }

    /// Register a local interrupt controller for specific CPUs
    /// Returns the controller index
    pub fn register_local_controller(&mut self, controller: Box<dyn LocalInterruptController>, cpu_ids: &[CpuId]) -> usize {
        let controller_index = self.local_controllers.len();
        self.local_controllers.push(controller);
        
        // Map CPUs to this controller
        for &cpu_id in cpu_ids {
            self.cpu_to_local_controller.insert(cpu_id, controller_index);
        }
        
        controller_index
    }

    /// Register a local interrupt controller for a single CPU
    /// Returns the controller index
    pub fn register_local_controller_for_cpu(&mut self, controller: Box<dyn LocalInterruptController>, cpu_id: CpuId) -> usize {
        self.register_local_controller(controller, &[cpu_id])
    }

    /// Register a local interrupt controller for a CPU range (convenience function)
    /// Returns the controller index
    pub fn register_local_controller_for_range(&mut self, controller: Box<dyn LocalInterruptController>, cpu_range: core::ops::Range<CpuId>) -> usize {
        let cpu_ids: alloc::vec::Vec<CpuId> = cpu_range.collect();
        self.register_local_controller(controller, &cpu_ids)
    }

    /// Register an external interrupt controller
    pub fn register_external_controller(&mut self, controller: Box<dyn ExternalInterruptController>) {
        self.external_controller = Some(controller);
    }

    /// Get a mutable reference to the local interrupt controller for a specific CPU
    pub fn local_controller_mut_for_cpu(&mut self, cpu_id: CpuId) -> Option<&mut Box<dyn LocalInterruptController>> {
        let controller_index = self.cpu_to_local_controller.get(&cpu_id)?;
        self.local_controllers.get_mut(*controller_index)
    }

    /// Get a mutable reference to a specific local interrupt controller by index
    pub fn local_controller_mut(&mut self, index: usize) -> Option<&mut Box<dyn LocalInterruptController>> {
        self.local_controllers.get_mut(index)
    }

    /// Get a mutable reference to the external interrupt controller
    pub fn external_controller_mut(&mut self) -> Option<&mut Box<dyn ExternalInterruptController>> {
        self.external_controller.as_mut()
    }

    /// Check if local controller is available for a specific CPU
    pub fn has_local_controller_for_cpu(&self, cpu_id: CpuId) -> bool {
        self.cpu_to_local_controller.contains_key(&cpu_id)
    }

    /// Check if any local controller is available
    pub fn has_local_controller(&self) -> bool {
        !self.local_controllers.is_empty()
    }

    /// Check if external controller is available
    pub fn has_external_controller(&self) -> bool {
        self.external_controller.is_some()
    }

    /// Get the number of registered local controllers
    pub fn local_controller_count(&self) -> usize {
        self.local_controllers.len()
    }

    /// Get CPU IDs managed by a specific local controller
    pub fn cpus_for_controller(&self, controller_index: usize) -> alloc::vec::Vec<CpuId> {
        self.cpu_to_local_controller
            .iter()
            .filter_map(|(cpu_id, &index)| {
                if index == controller_index {
                    Some(*cpu_id)
                } else {
                    None
                }
            })
            .collect()
    }
}
