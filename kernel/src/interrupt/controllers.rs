//! Interrupt controller trait definitions
//!
//! This module defines the basic traits for local and external interrupt controllers.

use alloc::boxed::Box;

use crate::device::platform::resource::{PlatformDeviceResource, PlatformDeviceResourceType};
use crate::interrupt::InterruptError;

use super::{CpuId, Hwirq, InterruptId, InterruptResult, Priority, Virq};

pub use super::msi::{
    MsiAllocation, MsiMessage, MsiRequest, MsiRequestFlags, MsiRequester, MsiVector,
};

/// Virtual IRQ used for controller-provided reschedule IPIs without a normal SGI virq.
pub const RESCHEDULE_IPI_VIRQ: Virq = Virq::MAX;

/// Hardware IRQ sentinel used for software IPIs that have no external IRQ line.
pub const SOFTWARE_IPI_HWIRQ: Hwirq = Hwirq::MAX;

/// Interrupt handling flow used by the interrupt core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqFlow {
    /// Level-triggered line interrupt.
    Level,
    /// Edge-triggered line interrupt.
    Edge,
    /// Interrupt whose CPU-interface EOI is enough to finish handling.
    FastEoi,
    /// Message-signaled interrupt.
    Msi,
}

/// Mapping from a controller-local interrupt source to a kernel virtual IRQ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IrqMapping {
    /// Kernel-global virtual IRQ.
    pub virq: Virq,
    /// Controller-local hardware IRQ.
    pub hwirq: Hwirq,
    /// Handling flow for this interrupt.
    pub flow: IrqFlow,
}

impl IrqMapping {
    /// Create a legacy one-to-one IRQ mapping.
    ///
    /// # Arguments
    ///
    /// * `interrupt_id` - Interrupt number used as both virtual and hardware IRQ.
    /// * `flow` - Interrupt handling flow.
    ///
    /// # Returns
    ///
    /// A one-to-one interrupt mapping.
    pub const fn legacy(interrupt_id: InterruptId, flow: IrqFlow) -> Self {
        Self {
            virq: interrupt_id,
            hwirq: interrupt_id,
            flow,
        }
    }
}

/// Interrupt returned by the CPU-facing delivery path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingIrq {
    /// Interrupt mapping for this delivery.
    pub mapping: IrqMapping,
    /// CPU that observed the interrupt.
    pub cpu_id: CpuId,
}

/// Trait for per-CPU timer controllers.
///
/// Timer controllers manage the architectural or firmware-backed compare source
/// that raises timer interrupts on a CPU.
pub trait TimerController: Send + Sync {
    /// Initialize the timer controller for a specific CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose timer state should be initialized.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error when the CPU is invalid or
    /// the hardware cannot be initialized.
    fn init(&mut self, cpu_id: CpuId) -> InterruptResult<()>;

    /// Enable timer interrupts for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose timer interrupt source should be enabled.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn enable_timer(&self, cpu_id: CpuId) -> InterruptResult<()>;

    /// Disable timer interrupts for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose timer interrupt source should be disabled.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn disable_timer(&self, cpu_id: CpuId) -> InterruptResult<()>;

    /// Check whether a timer interrupt is pending for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose timer pending state should be checked.
    ///
    /// # Returns
    ///
    /// `true` when a timer interrupt is pending.
    fn is_timer_pending(&self, cpu_id: CpuId) -> bool;

    /// Clear or acknowledge a timer interrupt for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose timer interrupt should be cleared.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn clear_timer(&mut self, cpu_id: CpuId) -> InterruptResult<()>;

    /// Set the next timer compare value for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose timer compare should be programmed.
    /// * `time` - Absolute timer counter value for the next interrupt.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn set_timer(&self, cpu_id: CpuId, time: u64) -> InterruptResult<()>;

    /// Get the current timer counter value.
    ///
    /// # Returns
    ///
    /// The current timer counter value.
    fn get_time(&self) -> u64;

    /// Get the timer clock frequency.
    ///
    /// # Returns
    ///
    /// Timer frequency in Hz.
    fn get_timer_frequency_hz(&self) -> u64;
}

/// Trait for per-CPU software interrupt / IPI controllers.
///
/// Software interrupt controllers send and clear CPU-local reschedule-style
/// interrupts. On RISC-V this is typically SBI IPI; on AArch64 SGIs are usually
/// provided by the external interrupt controller.
pub trait SoftwareInterruptController: Send + Sync {
    /// Initialize software interrupt state for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose software interrupt state should be initialized.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn init(&mut self, cpu_id: CpuId) -> InterruptResult<()>;

    /// Enable software interrupts for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose software interrupt source should be enabled.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn enable_software_interrupt(&self, cpu_id: CpuId) -> InterruptResult<()>;

    /// Disable software interrupts for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose software interrupt source should be disabled.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn disable_software_interrupt(&self, cpu_id: CpuId) -> InterruptResult<()>;

    /// Check whether a software interrupt is pending for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose software interrupt pending state should be checked.
    ///
    /// # Returns
    ///
    /// `true` when a software interrupt is pending.
    fn is_software_interrupt_pending(&self, cpu_id: CpuId) -> bool;

    /// Clear a software interrupt for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose software interrupt should be cleared.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn clear_software_interrupt(&mut self, cpu_id: CpuId) -> InterruptResult<()>;

    /// Send a software interrupt to a CPU.
    ///
    /// # Arguments
    ///
    /// * `target_cpu` - CPU that should receive the software interrupt.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn send_software_interrupt(&self, target_cpu: CpuId) -> InterruptResult<()>;
}

/// Trait for external interrupt controllers (like PLIC)
///
/// External interrupt controllers manage interrupts from external devices
/// and can route them to different CPUs with priority support.
pub trait ExternalInterruptController: Send + Sync {
    /// Initialize the external interrupt controller.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error when initialization fails.
    fn init(&mut self) -> InterruptResult<()>;

    /// Enable a hardware interrupt for a CPU.
    ///
    /// # Arguments
    ///
    /// * `interrupt_id` - Controller-local hardware interrupt number.
    /// * `cpu_id` - CPU that should receive the interrupt.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn enable_interrupt(&self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()>;

    /// Disable a hardware interrupt for a CPU.
    ///
    /// # Arguments
    ///
    /// * `interrupt_id` - Controller-local hardware interrupt number.
    /// * `cpu_id` - CPU whose interrupt routing should be disabled.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn disable_interrupt(&self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()>;

    /// Set priority for a hardware interrupt.
    ///
    /// # Arguments
    ///
    /// * `interrupt_id` - Controller-local hardware interrupt number.
    /// * `priority` - Controller-specific priority value.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn set_priority(
        &mut self,
        interrupt_id: InterruptId,
        priority: Priority,
    ) -> InterruptResult<()>;

    /// Get priority for a hardware interrupt.
    ///
    /// # Arguments
    ///
    /// * `interrupt_id` - Controller-local hardware interrupt number.
    ///
    /// # Returns
    ///
    /// Controller-specific priority value.
    fn get_priority(&self, interrupt_id: InterruptId) -> InterruptResult<Priority>;

    /// Set priority threshold for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose threshold should be updated.
    /// * `threshold` - Controller-specific threshold value.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn set_threshold(&mut self, cpu_id: CpuId, threshold: Priority) -> InterruptResult<()>;

    /// Get priority threshold for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose threshold should be read.
    ///
    /// # Returns
    ///
    /// Controller-specific threshold value.
    fn get_threshold(&self, cpu_id: CpuId) -> InterruptResult<Priority>;

    /// Claim an interrupt and return its controller-local interrupt number.
    ///
    /// This compatibility hook is claim/complete centric. New dispatch paths
    /// should prefer `claim_pending_irq`, which returns virtual IRQ mapping and
    /// flow information.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU receiving the interrupt.
    ///
    /// # Returns
    ///
    /// Hardware interrupt number, or `None` if no interrupt is pending.
    fn claim_interrupt(&self, cpu_id: CpuId) -> InterruptResult<Option<InterruptId>>;

    /// Complete a controller-local hardware interrupt.
    ///
    /// This compatibility hook is used by the default `eoi_irq`
    /// implementation. New controllers can override `eoi_irq` directly when
    /// completion requires flow-specific state.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU that handled the interrupt.
    /// * `interrupt_id` - Controller-local hardware interrupt number.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn complete_interrupt(&self, cpu_id: CpuId, interrupt_id: InterruptId) -> InterruptResult<()>;

    /// Check if a hardware interrupt is pending.
    ///
    /// # Arguments
    ///
    /// * `interrupt_id` - Controller-local hardware interrupt number.
    ///
    /// # Returns
    ///
    /// `true` when the interrupt is pending.
    fn is_pending(&self, interrupt_id: InterruptId) -> bool;

    /// Get the maximum controller-local interrupt number supported.
    ///
    /// # Returns
    ///
    /// Maximum controller-local interrupt number.
    fn max_interrupts(&self) -> InterruptId;

    /// Get the number of CPUs supported by this controller.
    ///
    /// # Returns
    ///
    /// Number of CPUs that can be targeted.
    fn max_cpus(&self) -> CpuId;

    /// Translate a firmware-provided IRQ resource into this controller's interrupt ID.
    ///
    /// # Arguments
    ///
    /// * `resource` - Platform IRQ resource discovered from firmware.
    ///
    /// # Returns
    ///
    /// The interrupt ID used by this controller.
    fn translate_irq_resource(
        &self,
        resource: &PlatformDeviceResource,
    ) -> InterruptResult<InterruptId> {
        if resource.res_type != PlatformDeviceResourceType::IRQ {
            return Err(InterruptError::InvalidInterruptId);
        }

        Ok(resource
            .irq_metadata
            .map_or(resource.start as InterruptId, |metadata| {
                metadata.irq_number
            }))
    }

    /// Map a firmware-provided IRQ resource into a kernel IRQ descriptor.
    ///
    /// # Arguments
    ///
    /// * `resource` - Platform IRQ resource discovered from firmware.
    ///
    /// # Returns
    ///
    /// Interrupt mapping used by the interrupt core.
    fn map_irq_resource(&self, resource: &PlatformDeviceResource) -> InterruptResult<IrqMapping> {
        let hwirq = self.translate_irq_resource(resource)?;
        Ok(IrqMapping::legacy(hwirq, IrqFlow::Level))
    }

    /// Claim or fetch the next pending interrupt for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU receiving the interrupt.
    ///
    /// # Returns
    ///
    /// Pending interrupt mapping, or `None` if there is no pending interrupt.
    fn claim_pending_irq(&self, cpu_id: CpuId) -> InterruptResult<Option<PendingIrq>> {
        Ok(self
            .claim_interrupt(cpu_id)?
            .map(|interrupt_id| PendingIrq {
                mapping: IrqMapping::legacy(interrupt_id, IrqFlow::Level),
                cpu_id,
            }))
    }

    /// Acknowledge a pending interrupt before running handlers.
    ///
    /// # Arguments
    ///
    /// * `irq` - Pending interrupt to acknowledge.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success.
    fn ack_irq(&self, _irq: &PendingIrq) -> InterruptResult<()> {
        Ok(())
    }

    /// Finish interrupt handling at the controller.
    ///
    /// # Arguments
    ///
    /// * `irq` - Pending interrupt being finished.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success.
    fn eoi_irq(&self, irq: &PendingIrq) -> InterruptResult<()> {
        self.complete_interrupt(irq.cpu_id, irq.mapping.hwirq)
    }

    /// Send an inter-processor interrupt through this controller.
    ///
    /// # Arguments
    ///
    /// * `target_cpu_id` - CPU that should receive the IPI.
    /// * `ipi_type` - Local interrupt type to deliver.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or `NotSupported` when the controller cannot send
    /// IPIs.
    fn send_ipi(&self, target_cpu_id: CpuId, ipi_type: LocalInterruptType) -> InterruptResult<()> {
        let _ = (target_cpu_id, ipi_type);
        Err(InterruptError::NotSupported)
    }

    /// Initialize external interrupt state for a CPU.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU whose controller state should be initialized.
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an interrupt error on failure.
    fn init_for_cpu(&mut self, _cpu_id: CpuId) -> InterruptResult<()> {
        Ok(())
    }
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
    timer_controllers: alloc::vec::Vec<Box<dyn TimerController>>,
    software_interrupt_controllers: alloc::vec::Vec<Box<dyn SoftwareInterruptController>>,
    external_controller: Option<Box<dyn ExternalInterruptController>>,
    cpu_to_timer_controller: alloc::collections::BTreeMap<CpuId, usize>,
    cpu_to_software_interrupt_controller: alloc::collections::BTreeMap<CpuId, usize>,
}

unsafe impl Send for InterruptControllers {}
unsafe impl Sync for InterruptControllers {}

impl InterruptControllers {
    /// Create a new interrupt controller registry
    pub fn new() -> Self {
        Self {
            timer_controllers: alloc::vec::Vec::new(),
            software_interrupt_controllers: alloc::vec::Vec::new(),
            external_controller: None,
            cpu_to_timer_controller: alloc::collections::BTreeMap::new(),
            cpu_to_software_interrupt_controller: alloc::collections::BTreeMap::new(),
        }
    }

    /// Register a timer controller for specific CPUs
    /// Returns the controller index
    pub fn register_timer_controller(
        &mut self,
        controller: Box<dyn TimerController>,
        cpu_ids: &[CpuId],
    ) -> usize {
        let controller_index = self.timer_controllers.len();
        self.timer_controllers.push(controller);

        // Map CPUs to this controller
        for &cpu_id in cpu_ids {
            if !self.cpu_to_timer_controller.contains_key(&cpu_id) {
                self.cpu_to_timer_controller
                    .insert(cpu_id, controller_index);
            } else {
                crate::early_println!(
                    "[interrupt] Timer controller already registered for CPU {}, keeping existing mapping",
                    cpu_id
                );
            }
        }

        controller_index
    }

    /// Register a timer controller for a single CPU
    /// Returns the controller index
    pub fn register_timer_controller_for_cpu(
        &mut self,
        controller: Box<dyn TimerController>,
        cpu_id: CpuId,
    ) -> usize {
        self.register_timer_controller(controller, &[cpu_id])
    }

    /// Register a timer controller for a CPU range (convenience function)
    /// Returns the controller index
    pub fn register_timer_controller_for_range(
        &mut self,
        controller: Box<dyn TimerController>,
        cpu_range: core::ops::Range<CpuId>,
    ) -> usize {
        let cpu_ids: alloc::vec::Vec<CpuId> = cpu_range.collect();
        self.register_timer_controller(controller, &cpu_ids)
    }

    /// Register a software interrupt controller for specific CPUs
    /// Returns the controller index
    pub fn register_software_interrupt_controller(
        &mut self,
        controller: Box<dyn SoftwareInterruptController>,
        cpu_ids: &[CpuId],
    ) -> usize {
        let controller_index = self.software_interrupt_controllers.len();
        self.software_interrupt_controllers.push(controller);

        for &cpu_id in cpu_ids {
            if !self
                .cpu_to_software_interrupt_controller
                .contains_key(&cpu_id)
            {
                self.cpu_to_software_interrupt_controller
                    .insert(cpu_id, controller_index);
            } else {
                crate::early_println!(
                    "[interrupt] Software interrupt controller already registered for CPU {}, keeping existing mapping",
                    cpu_id
                );
            }
        }

        controller_index
    }

    /// Register a software interrupt controller for a CPU range
    /// Returns the controller index
    pub fn register_software_interrupt_controller_for_range(
        &mut self,
        controller: Box<dyn SoftwareInterruptController>,
        cpu_range: core::ops::Range<CpuId>,
    ) -> usize {
        let cpu_ids: alloc::vec::Vec<CpuId> = cpu_range.collect();
        self.register_software_interrupt_controller(controller, &cpu_ids)
    }

    /// Register an external interrupt controller
    pub fn register_external_controller(
        &mut self,
        controller: Box<dyn ExternalInterruptController>,
    ) {
        self.external_controller = Some(controller);
    }

    /// Get a reference to the timer controller for a specific CPU
    pub fn timer_controller_for_cpu(&self, cpu_id: CpuId) -> Option<&dyn TimerController> {
        let controller_index = self.cpu_to_timer_controller.get(&cpu_id)?;
        self.timer_controllers
            .get(*controller_index)
            .map(Box::as_ref)
    }

    /// Get a mutable reference to the timer controller for a specific CPU
    pub fn timer_controller_mut_for_cpu(
        &mut self,
        cpu_id: CpuId,
    ) -> Option<&mut Box<dyn TimerController>> {
        let controller_index = self.cpu_to_timer_controller.get(&cpu_id)?;
        self.timer_controllers.get_mut(*controller_index)
    }

    /// Get a reference to the software interrupt controller for a specific CPU
    pub fn software_interrupt_controller_for_cpu(
        &self,
        cpu_id: CpuId,
    ) -> Option<&dyn SoftwareInterruptController> {
        let controller_index = self.cpu_to_software_interrupt_controller.get(&cpu_id)?;
        self.software_interrupt_controllers
            .get(*controller_index)
            .map(Box::as_ref)
    }

    /// Get a mutable reference to the software interrupt controller for a specific CPU
    pub fn software_interrupt_controller_mut_for_cpu(
        &mut self,
        cpu_id: CpuId,
    ) -> Option<&mut Box<dyn SoftwareInterruptController>> {
        let controller_index = self.cpu_to_software_interrupt_controller.get(&cpu_id)?;
        self.software_interrupt_controllers
            .get_mut(*controller_index)
    }

    /// Get a mutable reference to the external interrupt controller
    pub fn external_controller_mut(&mut self) -> Option<&mut Box<dyn ExternalInterruptController>> {
        self.external_controller.as_mut()
    }

    /// Get a shared reference to the external interrupt controller
    pub fn external_controller(&self) -> Option<&dyn ExternalInterruptController> {
        self.external_controller.as_deref()
    }

    /// Initialize all local controllers for their respective CPUs
    pub fn init_local_controllers(&mut self) -> InterruptResult<()> {
        for (cpu_id, &controller_index) in &self.cpu_to_timer_controller {
            if let Some(controller) = self.timer_controllers.get_mut(controller_index) {
                controller.init(*cpu_id)?;
            } else {
                return Err(InterruptError::ControllerNotFound);
            }
        }
        for (cpu_id, &controller_index) in &self.cpu_to_software_interrupt_controller {
            if let Some(controller) = self
                .software_interrupt_controllers
                .get_mut(controller_index)
            {
                controller.init(*cpu_id)?;
            } else {
                return Err(InterruptError::ControllerNotFound);
            }
        }
        Ok(())
    }

    /// Initialize the external controller
    pub fn init_external_controller(&mut self) -> InterruptResult<()> {
        if let Some(controller) = self.external_controller.as_mut() {
            crate::early_println!(
                "[interrupt] init_external_controller: calling controller.init()"
            );
            controller.init()?;
            crate::early_println!(
                "[interrupt] init_external_controller: controller.init() returned"
            );
            Ok(())
        } else {
            crate::early_println!(
                "[interrupt] init_external_controller: no external controller registered"
            );
            Err(InterruptError::ControllerNotFound)
        }
    }

    /// Check if local controller is available for a specific CPU
    pub fn has_local_controller_for_cpu(&self, cpu_id: CpuId) -> bool {
        self.cpu_to_timer_controller.contains_key(&cpu_id)
            || self
                .cpu_to_software_interrupt_controller
                .contains_key(&cpu_id)
    }

    /// Check if any local controller is available
    pub fn has_local_controller(&self) -> bool {
        !self.timer_controllers.is_empty() || !self.software_interrupt_controllers.is_empty()
    }

    /// Check if external controller is available
    pub fn has_external_controller(&self) -> bool {
        self.external_controller.is_some()
    }

    /// Get the number of registered local controllers
    pub fn local_controller_count(&self) -> usize {
        self.timer_controllers.len() + self.software_interrupt_controllers.len()
    }

    /// Get CPU IDs managed by a specific timer controller
    pub fn cpus_for_timer_controller(&self, controller_index: usize) -> alloc::vec::Vec<CpuId> {
        self.cpu_to_timer_controller
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
