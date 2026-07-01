//! Interrupt management system
//!
//! This module provides a comprehensive interrupt management system for the Scarlet kernel.
//! It supports both local interrupts (via CLINT) and external interrupts (via PLIC) on RISC-V architecture.

use alloc::{sync::Arc, vec::Vec};
use core::fmt;
use hashbrown::HashMap;

use crate::arch::{self, interrupt::enable_external_interrupts};
use crate::device::manager::DeviceManager;
use crate::device::platform::resource::PlatformDeviceResource;

pub mod controllers;
pub mod msi;

static INTERRUPT_MANAGER: spin::Once<InterruptManager> = spin::Once::new();

/// Kernel-global virtual IRQ number.
pub type Virq = u32;

/// Controller-local hardware IRQ number.
pub type Hwirq = u32;

/// Interrupt ID type.
///
/// This is kept as a compatibility alias for drivers. New interrupt-core code
/// should treat it as a `Virq`, not as a controller-local hardware line.
pub type InterruptId = Virq;

/// CPU ID type
pub type CpuId = u32;

/// Priority level for interrupts
pub type Priority = u32;

/// Resolve a platform IRQ resource to the interrupt ID used by the active controller.
///
/// Device Tree and other firmware formats describe interrupts in controller-specific
/// domains. Device drivers should not know those encodings; they should pass their
/// `PlatformDeviceResource` here and use the returned `InterruptId` for enabling and
/// registering handlers.
///
/// # Arguments
///
/// * `resource` - Platform IRQ resource discovered from firmware.
///
/// # Returns
///
/// The controller interrupt ID that should be enabled and registered.
pub fn resolve_platform_irq(resource: &PlatformDeviceResource) -> InterruptResult<InterruptId> {
    InterruptManager::global().resolve_platform_irq(resource)
}

/// Register an interrupt-capable device from a platform IRQ resource and enable
/// the corresponding controller interrupt line.
///
/// # Arguments
///
/// * `resource` - Platform IRQ resource discovered from firmware.
/// * `device` - Device that should receive interrupt callbacks.
/// * `cpu_id` - CPU that should receive the interrupt.
///
/// # Returns
///
/// The resolved controller interrupt ID.
pub fn register_and_enable_platform_irq_device(
    resource: &PlatformDeviceResource,
    device: Arc<dyn crate::device::events::InterruptCapableDevice>,
    cpu_id: CpuId,
) -> InterruptResult<InterruptId> {
    InterruptManager::global().register_and_enable_platform_irq_device(resource, device, cpu_id)
}

/// Register and enable a maskable interrupt source in lifecycle-safe order.
///
/// # Arguments
///
/// * `source` - Maskable interrupt source to register.
/// * `cpu_id` - CPU that should receive the interrupt.
///
/// # Returns
///
/// The virtual IRQ assigned to the source.
pub fn register_and_enable_interrupt_source(
    source: Arc<dyn MaskableInterruptSource>,
    cpu_id: CpuId,
) -> InterruptResult<InterruptId> {
    InterruptManager::global().register_and_enable_interrupt_source(source, cpu_id)
}

/// Allocate MSI/MSI-X vectors from the active external interrupt controller.
///
/// # Arguments
///
/// * `count` - Number of vectors requested.
/// * `cpu_id` - Preferred target CPU for the vectors.
///
/// # Returns
///
/// Allocated vectors with MSI doorbell programming data.
pub fn allocate_msi_vectors(
    request: controllers::MsiRequest,
) -> InterruptResult<controllers::MsiAllocation> {
    InterruptManager::global().allocate_msi_vectors(request)
}

/// Handler function type for external interrupts.
pub type ExternalInterruptHandler = fn(&mut InterruptHandle) -> InterruptResult<InterruptClaim>;

/// Handler function type for local interrupts (timer, software)
pub type LocalInterruptHandler =
    fn(cpu_id: CpuId, interrupt_type: controllers::LocalInterruptType) -> InterruptResult<()>;

/// Result of asking an interrupt source to claim a shared interrupt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptClaim {
    /// The source owned and cleared this interrupt.
    Handled,
    /// The source did not assert this shared interrupt line.
    NotMine,
}

impl InterruptClaim {
    /// Check whether this claim handled the interrupt.
    ///
    /// # Returns
    ///
    /// `true` when the interrupt source claimed and cleared the interrupt.
    pub const fn is_handled(self) -> bool {
        matches!(self, Self::Handled)
    }
}

/// Interrupt source that can participate in shared IRQ dispatch.
pub trait InterruptSource: Send + Sync {
    /// Return the interrupt line this source is attached to.
    ///
    /// # Returns
    ///
    /// The virtual IRQ number for this source, or `None` when it has not been
    /// attached yet.
    fn interrupt_id(&self) -> Option<InterruptId>;

    /// Try to claim and clear this source's interrupt cause.
    ///
    /// # Returns
    ///
    /// `Handled` when this source owned the interrupt and cleared its device-side
    /// cause, or `NotMine` when another source on the shared line asserted it.
    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim>;
}

/// Interrupt source whose device-side assertion can be masked independently.
pub trait MaskableInterruptSource: InterruptSource {
    /// Mask this source before registering or reconfiguring it.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the source can no longer assert its interrupt line.
    fn mask_source(&self) -> InterruptResult<()>;

    /// Unmask this source after its handler and controller line are ready.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the source may assert its interrupt line again.
    fn unmask_source(&self) -> InterruptResult<()>;

    /// Clear stale pending device-side interrupt state while the source is masked.
    ///
    /// # Returns
    ///
    /// `Ok(())` when any pending cause that can be cleared has been drained.
    fn clear_pending_source(&self) -> InterruptResult<()>;
}

struct InterruptDeviceSource {
    device: Arc<dyn crate::device::events::InterruptCapableDevice>,
}

impl InterruptDeviceSource {
    fn new(device: Arc<dyn crate::device::events::InterruptCapableDevice>) -> Self {
        Self { device }
    }
}

impl InterruptSource for InterruptDeviceSource {
    fn interrupt_id(&self) -> Option<InterruptId> {
        self.device.interrupt_id()
    }

    fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
        self.device.claim_interrupt()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IrqDesc {
    mapping: controllers::IrqMapping,
}

pub struct InterruptManager {
    controllers: spin::Once<spin::Mutex<controllers::InterruptControllers>>,
    irq_descs: spin::Lazy<spin::Mutex<HashMap<Virq, IrqDesc>>>,
    external_handlers: spin::Lazy<spin::Mutex<HashMap<InterruptId, ExternalInterruptHandler>>>,
    enabled_external_interrupts: spin::Lazy<spin::Mutex<HashMap<InterruptId, CpuId>>>,
    interrupt_sources:
        spin::Lazy<spin::Mutex<HashMap<InterruptId, alloc::vec::Vec<Arc<dyn InterruptSource>>>>>,
    unhandled_external_interrupts: spin::Lazy<spin::Mutex<HashMap<InterruptId, usize>>>,
}

impl InterruptManager {
    pub fn new() -> Self {
        Self {
            controllers: spin::Once::new(),
            irq_descs: spin::Lazy::new(|| spin::Mutex::new(HashMap::new())),
            external_handlers: spin::Lazy::new(|| spin::Mutex::new(HashMap::new())),
            enabled_external_interrupts: spin::Lazy::new(|| spin::Mutex::new(HashMap::new())),
            interrupt_sources: spin::Lazy::new(|| spin::Mutex::new(HashMap::new())),
            unhandled_external_interrupts: spin::Lazy::new(|| spin::Mutex::new(HashMap::new())),
        }
    }

    pub fn global() -> &'static InterruptManager {
        INTERRUPT_MANAGER.call_once(Self::new)
    }

    fn controllers(&self) -> &spin::Mutex<controllers::InterruptControllers> {
        self.controllers
            .call_once(|| spin::Mutex::new(controllers::InterruptControllers::new()))
    }

    pub fn init_controllers(&self) {
        crate::early_println!("[interrupt] init: external controller...");

        let enabled_external_interrupts: Vec<_> = self
            .enabled_external_interrupts
            .lock()
            .iter()
            .map(|(interrupt_id, cpu_id)| (self.irq_desc_or_legacy(*interrupt_id), *cpu_id))
            .collect();

        let mut controllers = self.controllers().lock();
        match controllers.init_external_controller() {
            Ok(()) => {}
            Err(e) => {
                crate::early_println!("Failed to initialize external controller: {}", e);
            }
        }
        if let Some(controller) = controllers.external_controller() {
            let reenable_count = enabled_external_interrupts.len();
            for (desc, cpu_id) in enabled_external_interrupts {
                if let Err(e) = controller.enable_interrupt(desc.mapping.hwirq, cpu_id) {
                    crate::early_println!(
                        "[interrupt] failed to re-enable IRQ {} for CPU {} after controller init: {}",
                        desc.mapping.virq,
                        cpu_id,
                        e
                    );
                }
            }
            crate::early_println!(
                "[interrupt] re-enabled {} external IRQs after controller init",
                reenable_count
            );
        }

        crate::early_println!("[interrupt] init: external controller done");
    }

    pub fn init_controllers_for_cpu(&self, cpu_id: CpuId) {
        disable_interrupts();

        let mut controllers = self.controllers().lock();

        if let Some(controller) = controllers.timer_controller_mut_for_cpu(cpu_id) {
            if let Err(e) = controller.init(cpu_id) {
                crate::early_println!(
                    "[interrupt] AP {}: failed to init timer controller: {}",
                    cpu_id,
                    e
                );
            }
        }

        if let Some(controller) = controllers.software_interrupt_controller_mut_for_cpu(cpu_id) {
            if let Err(e) = controller.init(cpu_id) {
                crate::early_println!(
                    "[interrupt] AP {}: failed to init software interrupt controller: {}",
                    cpu_id,
                    e
                );
            }
        }

        if let Some(controller) = controllers.external_controller_mut() {
            if let Err(e) = controller.init_for_cpu(cpu_id) {
                crate::early_println!(
                    "[interrupt] AP {}: failed to init external controller: {}",
                    cpu_id,
                    e
                );
            }
        }
    }

    pub fn resolve_platform_irq(
        &self,
        resource: &PlatformDeviceResource,
    ) -> InterruptResult<InterruptId> {
        let mapping = {
            let controllers = self.controllers().lock();
            if let Some(controller) = controllers.external_controller() {
                controller.map_irq_resource(resource)?
            } else {
                return Err(InterruptError::ControllerNotFound);
            }
        };

        self.register_irq_mapping(mapping);
        Ok(mapping.virq)
    }

    fn register_irq_mapping(&self, mapping: controllers::IrqMapping) {
        let mut descs = self.irq_descs.lock();
        descs.entry(mapping.virq).or_insert(IrqDesc { mapping });
    }

    fn irq_desc_or_legacy(&self, interrupt_id: InterruptId) -> IrqDesc {
        {
            let descs = self.irq_descs.lock();
            if let Some(desc) = descs.get(&interrupt_id) {
                return *desc;
            }
        }

        let mapping = controllers::IrqMapping::legacy(interrupt_id, controllers::IrqFlow::Level);
        let desc = IrqDesc { mapping };
        self.irq_descs.lock().entry(interrupt_id).or_insert(desc);
        desc
    }

    fn finish_pending_irq(&self, irq: &controllers::PendingIrq) -> InterruptResult<()> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.external_controller() {
            controller.eoi_irq(irq)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    fn record_unhandled_external_interrupt(&self, interrupt_id: InterruptId) -> usize {
        let mut counts = self.unhandled_external_interrupts.lock();
        let count = counts.entry(interrupt_id).or_insert(0);
        *count += 1;
        *count
    }

    fn clear_unhandled_external_interrupt_count(&self, interrupt_id: InterruptId) {
        self.unhandled_external_interrupts
            .lock()
            .remove(&interrupt_id);
    }

    fn unhandled_external_interrupt_count(&self, interrupt_id: InterruptId) -> usize {
        self.unhandled_external_interrupts
            .lock()
            .get(&interrupt_id)
            .copied()
            .unwrap_or(0)
    }

    fn handle_pending_irq(&self, pending: controllers::PendingIrq) -> InterruptResult<()> {
        self.register_irq_mapping(pending.mapping);
        {
            let controllers = self.controllers().lock();
            if let Some(controller) = controllers.external_controller() {
                controller.ack_irq(&pending)?;
            } else {
                return Err(InterruptError::ControllerNotFound);
            }
        }

        let interrupt_id = pending.mapping.virq;
        let sources = {
            let sources = self.interrupt_sources.lock();
            sources.get(&interrupt_id).cloned()
        };

        if let Some(sources) = sources {
            let mut handled = false;
            let mut first_error = None;
            for source in sources {
                match source.claim_interrupt() {
                    Ok(claim) => handled |= claim.is_handled(),
                    Err(error) => {
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }

            if handled {
                self.clear_unhandled_external_interrupt_count(interrupt_id);
            } else {
                let count = self.record_unhandled_external_interrupt(interrupt_id);
                if count == 1 || count.is_power_of_two() {
                    crate::early_println!(
                        "[interrupt] IRQ {} was not claimed by any registered source (count={})",
                        interrupt_id,
                        count
                    );
                }
            }

            let finish_result = self.finish_pending_irq(&pending);
            if let Some(error) = first_error {
                finish_result?;
                Err(error)
            } else {
                finish_result
            }
        } else {
            let handler = {
                let handlers = self.external_handlers.lock();
                handlers.get(&interrupt_id).copied()
            };

            if let Some(handler_fn) = handler {
                let mut handle = InterruptHandle::new_pending(pending);
                let claim = handler_fn(&mut handle)?;
                if claim.is_handled() {
                    self.clear_unhandled_external_interrupt_count(interrupt_id);
                } else {
                    let count = self.record_unhandled_external_interrupt(interrupt_id);
                    if count == 1 || count.is_power_of_two() {
                        crate::early_println!(
                            "[interrupt] IRQ {} was not claimed by its registered handler (count={})",
                            interrupt_id,
                            count
                        );
                    }
                }
                Ok(())
            } else {
                self.finish_pending_irq(&pending)
            }
        }
    }

    pub fn handle_external_interrupt(
        &self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
        let desc = self.irq_desc_or_legacy(interrupt_id);
        self.handle_pending_irq(controllers::PendingIrq {
            mapping: desc.mapping,
            cpu_id,
        })
    }

    /// Claim and handle one pending external interrupt, preserving its mapping.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU receiving the interrupt.
    ///
    /// # Returns
    ///
    /// The handled pending IRQ, or `None` when no external interrupt is pending.
    pub fn claim_and_handle_pending_external_interrupt(
        &self,
        cpu_id: CpuId,
    ) -> InterruptResult<Option<controllers::PendingIrq>> {
        let pending = {
            let controllers = self.controllers().lock();
            if let Some(controller) = controllers.external_controller() {
                controller.claim_pending_irq(cpu_id)?
            } else {
                return Err(InterruptError::ControllerNotFound);
            }
        };

        if let Some(pending) = pending {
            self.handle_pending_irq(pending)?;
            Ok(Some(pending))
        } else {
            Ok(None)
        }
    }

    /// Claim and handle one pending external interrupt.
    ///
    /// # Arguments
    ///
    /// * `cpu_id` - CPU receiving the interrupt.
    ///
    /// # Returns
    ///
    /// The handled virtual IRQ number, or `None` when no external interrupt is pending.
    pub fn claim_and_handle_external_interrupt(
        &self,
        cpu_id: CpuId,
    ) -> InterruptResult<Option<InterruptId>> {
        self.claim_and_handle_pending_external_interrupt(cpu_id)
            .map(|pending| pending.map(|pending| pending.mapping.virq))
    }

    pub fn enable_local_interrupt(
        &self,
        cpu_id: CpuId,
        interrupt_type: controllers::LocalInterruptType,
    ) -> InterruptResult<()> {
        let controllers = self.controllers().lock();
        match interrupt_type {
            controllers::LocalInterruptType::Timer => controllers
                .timer_controller_for_cpu(cpu_id)
                .ok_or(InterruptError::ControllerNotFound)?
                .enable_timer(cpu_id),
            controllers::LocalInterruptType::Software => controllers
                .software_interrupt_controller_for_cpu(cpu_id)
                .ok_or(InterruptError::ControllerNotFound)?
                .enable_software_interrupt(cpu_id),
            controllers::LocalInterruptType::External => Err(InterruptError::NotSupported),
        }
    }

    pub fn disable_local_interrupt(
        &self,
        cpu_id: CpuId,
        interrupt_type: controllers::LocalInterruptType,
    ) -> InterruptResult<()> {
        let controllers = self.controllers().lock();
        match interrupt_type {
            controllers::LocalInterruptType::Timer => controllers
                .timer_controller_for_cpu(cpu_id)
                .ok_or(InterruptError::ControllerNotFound)?
                .disable_timer(cpu_id),
            controllers::LocalInterruptType::Software => controllers
                .software_interrupt_controller_for_cpu(cpu_id)
                .ok_or(InterruptError::ControllerNotFound)?
                .disable_software_interrupt(cpu_id),
            controllers::LocalInterruptType::External => Err(InterruptError::NotSupported),
        }
    }

    pub fn send_software_interrupt(&self, target_cpu: CpuId) -> InterruptResult<()> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.software_interrupt_controller_for_cpu(target_cpu) {
            controller.send_software_interrupt(target_cpu)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn set_timer(&self, cpu_id: CpuId, time: u64) -> InterruptResult<()> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.timer_controller_for_cpu(cpu_id) {
            controller.set_timer(cpu_id, time)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn get_time(&self, cpu_id: CpuId) -> InterruptResult<u64> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.timer_controller_for_cpu(cpu_id) {
            Ok(controller.get_time())
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn get_timer_frequency_hz(&self, cpu_id: CpuId) -> InterruptResult<u64> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.timer_controller_for_cpu(cpu_id) {
            Ok(controller.get_timer_frequency_hz())
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn is_local_interrupt_pending(
        &self,
        cpu_id: CpuId,
        interrupt_type: controllers::LocalInterruptType,
    ) -> bool {
        let controllers = self.controllers().lock();
        match interrupt_type {
            controllers::LocalInterruptType::Timer => controllers
                .timer_controller_for_cpu(cpu_id)
                .map(|controller| controller.is_timer_pending(cpu_id))
                .unwrap_or(false),
            controllers::LocalInterruptType::Software => controllers
                .software_interrupt_controller_for_cpu(cpu_id)
                .map(|controller| controller.is_software_interrupt_pending(cpu_id))
                .unwrap_or(false),
            controllers::LocalInterruptType::External => false,
        }
    }

    pub fn register_timer_controller(
        &self,
        controller: alloc::boxed::Box<dyn controllers::TimerController>,
        cpu_ids: &[CpuId],
    ) -> InterruptResult<usize> {
        let mut controllers = self.controllers().lock();
        Ok(controllers.register_timer_controller(controller, cpu_ids))
    }

    pub fn register_timer_controller_for_range(
        &self,
        controller: alloc::boxed::Box<dyn controllers::TimerController>,
        cpu_range: core::ops::Range<CpuId>,
    ) -> InterruptResult<usize> {
        let mut controllers = self.controllers().lock();
        Ok(controllers.register_timer_controller_for_range(controller, cpu_range))
    }

    pub fn register_timer_controller_for_cpu(
        &self,
        controller: alloc::boxed::Box<dyn controllers::TimerController>,
        cpu_id: CpuId,
    ) -> InterruptResult<usize> {
        let mut controllers = self.controllers().lock();
        Ok(controllers.register_timer_controller_for_cpu(controller, cpu_id))
    }

    pub fn register_software_interrupt_controller_for_range(
        &self,
        controller: alloc::boxed::Box<dyn controllers::SoftwareInterruptController>,
        cpu_range: core::ops::Range<CpuId>,
    ) -> InterruptResult<usize> {
        let mut controllers = self.controllers().lock();
        Ok(controllers.register_software_interrupt_controller_for_range(controller, cpu_range))
    }

    pub fn register_external_controller(
        &self,
        controller: alloc::boxed::Box<dyn controllers::ExternalInterruptController>,
    ) -> InterruptResult<()> {
        let mut controllers = self.controllers().lock();
        if controllers.has_external_controller() {
            return Err(InterruptError::HardwareError);
        }
        controllers.register_external_controller(controller);
        Ok(())
    }

    pub fn register_external_handler(
        &self,
        interrupt_id: InterruptId,
        handler: ExternalInterruptHandler,
    ) -> InterruptResult<()> {
        let mut handlers = self.external_handlers.lock();
        if handlers.contains_key(&interrupt_id) {
            return Err(InterruptError::HandlerAlreadyRegistered);
        }
        drop(handlers);
        self.irq_desc_or_legacy(interrupt_id);
        let mut handlers = self.external_handlers.lock();
        handlers.insert(interrupt_id, handler);
        Ok(())
    }

    pub fn register_interrupt_device(
        &self,
        interrupt_id: InterruptId,
        device: Arc<dyn crate::device::events::InterruptCapableDevice>,
    ) -> InterruptResult<()> {
        self.register_interrupt_source(interrupt_id, Arc::new(InterruptDeviceSource::new(device)))
    }

    /// Register an interrupt source on a virtual IRQ.
    ///
    /// # Arguments
    ///
    /// * `interrupt_id` - Virtual IRQ to dispatch to this source.
    /// * `source` - Source to call when the IRQ is delivered.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the source is registered.
    pub fn register_interrupt_source(
        &self,
        interrupt_id: InterruptId,
        source: Arc<dyn InterruptSource>,
    ) -> InterruptResult<()> {
        if let Some(source_interrupt_id) = source.interrupt_id()
            && source_interrupt_id != interrupt_id
        {
            return Err(InterruptError::InvalidInterruptId);
        }

        self.irq_desc_or_legacy(interrupt_id);
        let mut sources = self.interrupt_sources.lock();
        sources.entry(interrupt_id).or_default().push(source);
        Ok(())
    }

    /// Register and enable a maskable interrupt source in lifecycle-safe order.
    ///
    /// The guaranteed order is:
    ///
    /// 1. mask the source;
    /// 2. clear stale pending source state;
    /// 3. register the source with the interrupt manager;
    /// 4. enable the interrupt controller line;
    /// 5. unmask the source.
    ///
    /// # Arguments
    ///
    /// * `source` - Maskable interrupt source to register.
    /// * `cpu_id` - CPU that should receive the interrupt.
    ///
    /// # Returns
    ///
    /// The virtual IRQ assigned to the source.
    pub fn register_and_enable_interrupt_source(
        &self,
        source: Arc<dyn MaskableInterruptSource>,
        cpu_id: CpuId,
    ) -> InterruptResult<InterruptId> {
        source.mask_source()?;
        source.clear_pending_source()?;
        let interrupt_id = source
            .interrupt_id()
            .ok_or(InterruptError::InvalidInterruptId)?;
        self.register_interrupt_source(interrupt_id, source.clone())?;
        self.enable_external_interrupt(interrupt_id, cpu_id)?;

        if let Err(error) = source.unmask_source() {
            let _ = self.disable_external_interrupt(interrupt_id, cpu_id);
            return Err(error);
        }

        Ok(interrupt_id)
    }

    pub fn register_platform_interrupt_device(
        &self,
        resource: &PlatformDeviceResource,
        device: Arc<dyn crate::device::events::InterruptCapableDevice>,
    ) -> InterruptResult<InterruptId> {
        let interrupt_id = self.resolve_platform_irq(resource)?;
        self.register_interrupt_device(interrupt_id, device)?;
        Ok(interrupt_id)
    }

    pub fn register_and_enable_platform_irq_device(
        &self,
        resource: &PlatformDeviceResource,
        device: Arc<dyn crate::device::events::InterruptCapableDevice>,
        cpu_id: CpuId,
    ) -> InterruptResult<InterruptId> {
        let interrupt_id = self.register_platform_interrupt_device(resource, device)?;
        self.enable_external_interrupt(interrupt_id, cpu_id)?;
        Ok(interrupt_id)
    }

    pub fn complete_external_interrupt(
        &self,
        cpu_id: CpuId,
        interrupt_id: InterruptId,
    ) -> InterruptResult<()> {
        let desc = self.irq_desc_or_legacy(interrupt_id);
        self.finish_pending_irq(&controllers::PendingIrq {
            mapping: desc.mapping,
            cpu_id,
        })
    }

    pub fn enable_external_interrupt(
        &self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
        let desc = self.irq_desc_or_legacy(interrupt_id);
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.external_controller() {
            controller.enable_interrupt(desc.mapping.hwirq, cpu_id)?;
            self.enabled_external_interrupts
                .lock()
                .insert(interrupt_id, cpu_id);
            Ok(())
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn enable_platform_interrupt(
        &self,
        resource: &PlatformDeviceResource,
        cpu_id: CpuId,
    ) -> InterruptResult<InterruptId> {
        let interrupt_id = self.resolve_platform_irq(resource)?;
        self.enable_external_interrupt(interrupt_id, cpu_id)?;
        Ok(interrupt_id)
    }

    pub fn allocate_msi_vectors(
        &self,
        request: controllers::MsiRequest,
    ) -> InterruptResult<controllers::MsiAllocation> {
        self.allocate_msi_vectors_from_device_manager(DeviceManager::get_manager(), request)
    }

    fn allocate_msi_vectors_from_device_manager(
        &self,
        device_manager: &DeviceManager,
        request: controllers::MsiRequest,
    ) -> InterruptResult<controllers::MsiAllocation> {
        let mut result = Err(InterruptError::NotSupported);
        let mut controller_seen = false;

        device_manager.for_each_msi_controller(|controller| {
            controller_seen = true;
            match controller.allocate_vectors(request) {
                Ok(allocation) => {
                    result = Ok(allocation);
                    false
                }
                Err(error) => {
                    result = Err(Self::msi_error_to_interrupt_error(error));
                    true
                }
            }
        });

        let allocation = match result {
            Ok(allocation) => allocation,
            Err(_) if !controller_seen => return Err(InterruptError::NotSupported),
            Err(error) => return Err(error),
        };

        for vector in &allocation.vectors {
            self.register_irq_mapping(controllers::IrqMapping {
                virq: vector.virq,
                hwirq: vector.hwirq,
                flow: controllers::IrqFlow::Msi,
            });
        }

        Ok(allocation)
    }

    fn msi_error_to_interrupt_error(error: msi::MsiError) -> InterruptError {
        match error {
            msi::MsiError::ControllerNotFound => InterruptError::ControllerNotFound,
            msi::MsiError::NoVectors => InterruptError::NotSupported,
            msi::MsiError::InvalidRequest => InterruptError::InvalidOperation,
            msi::MsiError::NotSupported => InterruptError::NotSupported,
            msi::MsiError::HardwareError => InterruptError::HardwareError,
            msi::MsiError::Busy => InterruptError::InvalidOperation,
        }
    }

    pub fn disable_external_interrupt(
        &self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
        let desc = self.irq_desc_or_legacy(interrupt_id);
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.external_controller() {
            controller.disable_interrupt(desc.mapping.hwirq, cpu_id)?;
            self.enabled_external_interrupts
                .lock()
                .remove(&interrupt_id);
            Ok(())
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn has_local_controller(&self) -> bool {
        self.controllers().lock().has_local_controller()
    }

    pub fn has_external_controller(&self) -> bool {
        self.controllers().lock().has_external_controller()
    }

    pub fn send_ipi(
        &self,
        target_cpu_id: CpuId,
        ipi_type: controllers::LocalInterruptType,
    ) -> InterruptResult<()> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.external_controller() {
            controller.send_ipi(target_cpu_id, ipi_type)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }
}

/// Handle for managing interrupt processing
///
/// This provides a safe interface for interrupt handlers to interact with
/// the interrupt controller without direct access.
pub struct InterruptHandle {
    pending: controllers::PendingIrq,
    completed: bool,
}

impl InterruptHandle {
    /// Create a new interrupt handle
    pub fn new(interrupt_id: InterruptId, cpu_id: CpuId) -> Self {
        let desc = InterruptManager::global().irq_desc_or_legacy(interrupt_id);
        Self::new_pending(controllers::PendingIrq {
            mapping: desc.mapping,
            cpu_id,
        })
    }

    fn new_pending(pending: controllers::PendingIrq) -> Self {
        Self {
            pending,
            completed: false,
        }
    }

    /// Get the interrupt ID
    pub fn interrupt_id(&self) -> InterruptId {
        self.pending.mapping.virq
    }

    /// Get the controller-local hardware interrupt ID.
    pub fn hwirq(&self) -> Hwirq {
        self.pending.mapping.hwirq
    }

    /// Get the interrupt flow.
    pub fn flow(&self) -> controllers::IrqFlow {
        self.pending.mapping.flow
    }

    /// Get the CPU ID
    pub fn cpu_id(&self) -> CpuId {
        self.pending.cpu_id
    }

    /// Mark the interrupt as completed
    ///
    /// This should be called when the handler has finished processing the interrupt.
    pub fn complete(&mut self) -> InterruptResult<()> {
        if self.completed {
            return Err(InterruptError::InvalidOperation);
        }

        InterruptManager::global().finish_pending_irq(&self.pending)?;
        self.completed = true;
        Ok(())
    }

    /// Check if the interrupt has been completed
    pub fn is_completed(&self) -> bool {
        self.completed
    }

    /// Enable another interrupt
    pub fn enable_interrupt(&self, target_interrupt: InterruptId) -> InterruptResult<()> {
        InterruptManager::global().enable_external_interrupt(target_interrupt, self.cpu_id())
    }

    /// Disable another interrupt
    pub fn disable_interrupt(&self, target_interrupt: InterruptId) -> InterruptResult<()> {
        InterruptManager::global().disable_external_interrupt(target_interrupt, self.cpu_id())
    }
}

impl Drop for InterruptHandle {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.complete();
        }
    }
}

/// Result type for interrupt operations
pub type InterruptResult<T = ()> = Result<T, InterruptError>;

/// Errors that can occur during interrupt management
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptError {
    /// Invalid interrupt ID
    InvalidInterruptId,
    /// Invalid CPU ID
    InvalidCpuId,
    /// Controller not found
    ControllerNotFound,
    /// Handler already registered
    HandlerAlreadyRegistered,
    /// Handler not found
    HandlerNotFound,
    /// Invalid priority
    InvalidPriority,
    /// Operation not supported
    NotSupported,
    /// Hardware error
    HardwareError,
    /// Invalid operation
    InvalidOperation,
}

impl fmt::Display for InterruptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InterruptError::InvalidInterruptId => write!(f, "Invalid interrupt ID"),
            InterruptError::InvalidCpuId => write!(f, "Invalid CPU ID"),
            InterruptError::ControllerNotFound => write!(f, "Controller not found"),
            InterruptError::HandlerAlreadyRegistered => write!(f, "Handler already registered"),
            InterruptError::HandlerNotFound => write!(f, "Handler not found"),
            InterruptError::InvalidPriority => write!(f, "Invalid priority"),
            InterruptError::NotSupported => write!(f, "Operation not supported"),
            InterruptError::HardwareError => write!(f, "Hardware error"),
            InterruptError::InvalidOperation => write!(f, "Invalid operation"),
        }
    }
}

/// Enable interrupts globally
pub fn enable_interrupts() {
    arch::interrupt::enable_interrupts();
}

/// Disable interrupts globally
pub fn disable_interrupts() {
    arch::interrupt::disable_interrupts();
}

/// Execute a closure with interrupts disabled
pub fn with_interrupts_disabled<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    arch::interrupt::with_interrupts_disabled(f)
}

/// Check if interrupts are currently enabled
pub fn are_interrupts_enabled() -> bool {
    arch::interrupt::are_interrupts_enabled()
}

/// Enable software interrupt reception at CPU level.
///
/// RISC-V uses this for SSIE (reschedule IPIs). AArch64 SGIs are GIC-backed and
/// do not require a separate architectural CPU bit beyond the IRQ mask.
pub fn enable_software_interrupts() {
    arch::interrupt::enable_software_interrupts();
}

/// Enable CPU interrupt reception (stage 2).
///
/// This should run after device drivers have registered their handlers and
/// enabled the desired interrupt lines in the controller.
pub fn enable_cpu_interrupts() {
    enable_external_interrupts();
    enable_software_interrupts();
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use alloc::vec;
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::interrupt::msi::{
        MsiAllocation, MsiController, MsiError, MsiMessage, MsiRequest, MsiRequestFlags, MsiVector,
    };

    struct FakeMsiController {
        result: Result<controllers::MsiAllocation, msi::MsiError>,
        calls: AtomicUsize,
    }

    struct FakeExternalController {
        events: Arc<spin::Mutex<Vec<&'static str>>>,
    }

    impl FakeExternalController {
        fn new(events: Arc<spin::Mutex<Vec<&'static str>>>) -> Self {
            Self { events }
        }
    }

    impl controllers::ExternalInterruptController for FakeExternalController {
        fn init(&mut self) -> InterruptResult<()> {
            Ok(())
        }

        fn enable_interrupt(
            &self,
            _interrupt_id: InterruptId,
            _cpu_id: CpuId,
        ) -> InterruptResult<()> {
            self.events.lock().push("controller-enable");
            Ok(())
        }

        fn disable_interrupt(
            &self,
            _interrupt_id: InterruptId,
            _cpu_id: CpuId,
        ) -> InterruptResult<()> {
            self.events.lock().push("controller-disable");
            Ok(())
        }

        fn set_priority(
            &mut self,
            _interrupt_id: InterruptId,
            _priority: Priority,
        ) -> InterruptResult<()> {
            Ok(())
        }

        fn get_priority(&self, _interrupt_id: InterruptId) -> InterruptResult<Priority> {
            Ok(0)
        }

        fn set_threshold(&mut self, _cpu_id: CpuId, _threshold: Priority) -> InterruptResult<()> {
            Ok(())
        }

        fn get_threshold(&self, _cpu_id: CpuId) -> InterruptResult<Priority> {
            Ok(0)
        }

        fn claim_interrupt(&self, _cpu_id: CpuId) -> InterruptResult<Option<InterruptId>> {
            Ok(None)
        }

        fn complete_interrupt(
            &self,
            _cpu_id: CpuId,
            _interrupt_id: InterruptId,
        ) -> InterruptResult<()> {
            self.events.lock().push("controller-eoi");
            Ok(())
        }

        fn is_pending(&self, _interrupt_id: InterruptId) -> bool {
            false
        }

        fn max_interrupts(&self) -> InterruptId {
            256
        }

        fn max_cpus(&self) -> CpuId {
            4
        }

        fn ack_irq(&self, _irq: &controllers::PendingIrq) -> InterruptResult<()> {
            self.events.lock().push("controller-ack");
            Ok(())
        }
    }

    struct FakeInterruptSource {
        interrupt_id: InterruptId,
        claim: InterruptClaim,
        event_name: &'static str,
        events: Arc<spin::Mutex<Vec<&'static str>>>,
    }

    impl FakeInterruptSource {
        fn new(
            interrupt_id: InterruptId,
            claim: InterruptClaim,
            event_name: &'static str,
            events: Arc<spin::Mutex<Vec<&'static str>>>,
        ) -> Self {
            Self {
                interrupt_id,
                claim,
                event_name,
                events,
            }
        }
    }

    impl InterruptSource for FakeInterruptSource {
        fn interrupt_id(&self) -> Option<InterruptId> {
            Some(self.interrupt_id)
        }

        fn claim_interrupt(&self) -> InterruptResult<InterruptClaim> {
            self.events.lock().push(self.event_name);
            Ok(self.claim)
        }
    }

    impl MaskableInterruptSource for FakeInterruptSource {
        fn mask_source(&self) -> InterruptResult<()> {
            self.events.lock().push("source-mask");
            Ok(())
        }

        fn unmask_source(&self) -> InterruptResult<()> {
            self.events.lock().push("source-unmask");
            Ok(())
        }

        fn clear_pending_source(&self) -> InterruptResult<()> {
            self.events.lock().push("source-clear");
            Ok(())
        }
    }

    impl FakeMsiController {
        fn new(result: Result<controllers::MsiAllocation, msi::MsiError>) -> Self {
            Self {
                result,
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl MsiController for FakeMsiController {
        fn name(&self) -> &'static str {
            "fake-msi"
        }

        fn allocate_vectors(&self, _request: MsiRequest) -> Result<MsiAllocation, MsiError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.clone()
        }

        fn free_vectors(&self, _allocation: &MsiAllocation) {}

        fn mask_vector(&self, _vector: &MsiVector) -> Result<(), MsiError> {
            Ok(())
        }

        fn unmask_vector(&self, _vector: &MsiVector) -> Result<(), MsiError> {
            Ok(())
        }
    }

    fn test_request() -> controllers::MsiRequest {
        controllers::MsiRequest {
            count: 1,
            target_cpu: 0,
            requester: None,
            flags: MsiRequestFlags::NONE,
        }
    }

    fn test_allocation(virq: Virq) -> controllers::MsiAllocation {
        controllers::MsiAllocation {
            vectors: vec![controllers::MsiVector {
                virq,
                hwirq: virq + 32,
                message: MsiMessage {
                    address: 0xfee0_0000,
                    data: virq,
                },
            }],
        }
    }

    #[test_case]
    fn test_allocate_msi_vectors_returns_not_supported_when_no_controller_registered() {
        let devices = DeviceManager::new_for_test();
        let manager = InterruptManager::new();

        let error = manager
            .allocate_msi_vectors_from_device_manager(&devices, test_request())
            .unwrap_err();

        assert_eq!(error, InterruptError::NotSupported);
    }

    #[test_case]
    fn test_allocate_msi_vectors_returns_allocation_when_controller_succeeds() {
        let devices = DeviceManager::new_for_test();
        let controller = Arc::new(FakeMsiController::new(Ok(test_allocation(64))));
        devices.register_msi_controller(1, controller.clone());
        let manager = InterruptManager::new();

        let allocation = manager
            .allocate_msi_vectors_from_device_manager(&devices, test_request())
            .expect("expected MSI allocation");

        assert_eq!(controller.calls(), 1);
        assert_eq!(allocation.vectors.len(), 1);
        assert_eq!(allocation.vectors[0].virq, 64);
        assert_eq!(allocation.vectors[0].hwirq, 96);
    }

    #[test_case]
    fn test_allocate_msi_vectors_iterates_until_success() {
        let devices = DeviceManager::new_for_test();
        let first = Arc::new(FakeMsiController::new(Err(MsiError::NoVectors)));
        let second = Arc::new(FakeMsiController::new(Ok(test_allocation(80))));
        devices.register_msi_controller(1, first.clone());
        devices.register_msi_controller(2, second.clone());
        let manager = InterruptManager::new();

        let allocation = manager
            .allocate_msi_vectors_from_device_manager(&devices, test_request())
            .expect("expected second controller to allocate");

        assert_eq!(first.calls(), 1);
        assert_eq!(second.calls(), 1);
        assert_eq!(allocation.vectors[0].virq, 80);
    }

    #[test_case]
    fn test_shared_interrupt_sources_all_claim_before_controller_eoi() {
        let events = Arc::new(spin::Mutex::new(Vec::new()));
        let manager = InterruptManager::new();
        manager
            .register_external_controller(Box::new(FakeExternalController::new(events.clone())))
            .expect("fake external controller should register");
        manager
            .register_interrupt_source(
                42,
                Arc::new(FakeInterruptSource::new(
                    42,
                    InterruptClaim::NotMine,
                    "source-a",
                    events.clone(),
                )),
            )
            .expect("first source should register");
        manager
            .register_interrupt_source(
                42,
                Arc::new(FakeInterruptSource::new(
                    42,
                    InterruptClaim::Handled,
                    "source-b",
                    events.clone(),
                )),
            )
            .expect("second source should register");

        manager
            .handle_external_interrupt(42, 0)
            .expect("shared IRQ should dispatch");

        assert_eq!(
            events.lock().as_slice(),
            ["controller-ack", "source-a", "source-b", "controller-eoi"]
        );
        assert_eq!(manager.unhandled_external_interrupt_count(42), 0);
    }

    #[test_case]
    fn test_unclaimed_shared_interrupt_is_counted_and_eoi_still_runs() {
        let events = Arc::new(spin::Mutex::new(Vec::new()));
        let manager = InterruptManager::new();
        manager
            .register_external_controller(Box::new(FakeExternalController::new(events.clone())))
            .expect("fake external controller should register");
        manager
            .register_interrupt_source(
                43,
                Arc::new(FakeInterruptSource::new(
                    43,
                    InterruptClaim::NotMine,
                    "source-a",
                    events.clone(),
                )),
            )
            .expect("source should register");

        manager
            .handle_external_interrupt(43, 0)
            .expect("unclaimed IRQ should still finish at controller");

        assert_eq!(
            events.lock().as_slice(),
            ["controller-ack", "source-a", "controller-eoi"]
        );
        assert_eq!(manager.unhandled_external_interrupt_count(43), 1);
    }

    #[test_case]
    fn test_register_and_enable_interrupt_source_orders_mask_clear_enable_unmask() {
        let events = Arc::new(spin::Mutex::new(Vec::new()));
        let manager = InterruptManager::new();
        manager
            .register_external_controller(Box::new(FakeExternalController::new(events.clone())))
            .expect("fake external controller should register");

        let source = Arc::new(FakeInterruptSource::new(
            44,
            InterruptClaim::Handled,
            "source-claim",
            events.clone(),
        ));
        let interrupt_id = manager
            .register_and_enable_interrupt_source(source, 0)
            .expect("source lifecycle should complete");

        assert_eq!(interrupt_id, 44);
        assert_eq!(
            events.lock().as_slice(),
            [
                "source-mask",
                "source-clear",
                "controller-enable",
                "source-unmask"
            ]
        );
    }
}
