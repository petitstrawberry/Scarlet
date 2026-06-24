//! Interrupt management system
//!
//! This module provides a comprehensive interrupt management system for the Scarlet kernel.
//! It supports both local interrupts (via CLINT) and external interrupts (via PLIC) on RISC-V architecture.

use alloc::sync::Arc;
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

/// Handler function type for external interrupts
pub type ExternalInterruptHandler = fn(&mut InterruptHandle) -> InterruptResult<()>;

/// Handler function type for local interrupts (timer, software)
pub type LocalInterruptHandler =
    fn(cpu_id: CpuId, interrupt_type: controllers::LocalInterruptType) -> InterruptResult<()>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IrqDesc {
    mapping: controllers::IrqMapping,
}

pub struct InterruptManager {
    controllers: spin::Once<spin::Mutex<controllers::InterruptControllers>>,
    irq_descs: spin::Lazy<spin::Mutex<HashMap<Virq, IrqDesc>>>,
    external_handlers: spin::Lazy<spin::Mutex<HashMap<InterruptId, ExternalInterruptHandler>>>,
    interrupt_devices: spin::Lazy<
        spin::Mutex<
            HashMap<
                InterruptId,
                alloc::vec::Vec<Arc<dyn crate::device::events::InterruptCapableDevice>>,
            >,
        >,
    >,
}

impl InterruptManager {
    pub fn new() -> Self {
        Self {
            controllers: spin::Once::new(),
            irq_descs: spin::Lazy::new(|| spin::Mutex::new(HashMap::new())),
            external_handlers: spin::Lazy::new(|| spin::Mutex::new(HashMap::new())),
            interrupt_devices: spin::Lazy::new(|| spin::Mutex::new(HashMap::new())),
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

        let mut controllers = self.controllers().lock();
        match controllers.init_external_controller() {
            Ok(()) => {}
            Err(e) => {
                crate::early_println!("Failed to initialize external controller: {}", e);
            }
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
        let device = {
            let devices = self.interrupt_devices.lock();
            devices.get(&interrupt_id).cloned()
        };

        if let Some(devices) = device {
            for device in devices {
                device.handle_interrupt()?;
            }
            self.finish_pending_irq(&pending)
        } else {
            let handler = {
                let handlers = self.external_handlers.lock();
                handlers.get(&interrupt_id).copied()
            };

            if let Some(handler_fn) = handler {
                let mut handle = InterruptHandle::new_pending(pending);
                handler_fn(&mut handle)
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

    pub fn claim_and_handle_external_interrupt(
        &self,
        cpu_id: CpuId,
    ) -> InterruptResult<Option<InterruptId>> {
        let pending = {
            let controllers = self.controllers().lock();
            if let Some(controller) = controllers.external_controller() {
                controller.claim_pending_irq(cpu_id)?
            } else {
                return Err(InterruptError::ControllerNotFound);
            }
        };

        if let Some(pending) = pending {
            let virq = pending.mapping.virq;
            self.handle_pending_irq(pending)?;
            Ok(Some(virq))
        } else {
            Ok(None)
        }
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
        self.irq_desc_or_legacy(interrupt_id);
        let mut devices = self.interrupt_devices.lock();
        devices.entry(interrupt_id).or_default().push(device);
        Ok(())
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
            controller.enable_interrupt(desc.mapping.hwirq, cpu_id)
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
        let mut result = Err(InterruptError::NotSupported);
        let mut controller_seen = false;

        DeviceManager::get_manager().for_each_msi_controller(|controller| {
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
            controller.disable_interrupt(desc.mapping.hwirq, cpu_id)
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
        DeviceManager::get_manager().clear_for_test();
        let manager = InterruptManager::new();

        let error = manager.allocate_msi_vectors(test_request()).unwrap_err();

        assert_eq!(error, InterruptError::NotSupported);
    }

    #[test_case]
    fn test_allocate_msi_vectors_returns_allocation_when_controller_succeeds() {
        DeviceManager::get_manager().clear_for_test();
        let controller = Arc::new(FakeMsiController::new(Ok(test_allocation(64))));
        DeviceManager::get_manager().register_msi_controller(1, controller.clone());
        let manager = InterruptManager::new();

        let allocation = manager
            .allocate_msi_vectors(test_request())
            .expect("expected MSI allocation");

        assert_eq!(controller.calls(), 1);
        assert_eq!(allocation.vectors.len(), 1);
        assert_eq!(allocation.vectors[0].virq, 64);
        assert_eq!(allocation.vectors[0].hwirq, 96);
        DeviceManager::get_manager().clear_for_test();
    }

    #[test_case]
    fn test_allocate_msi_vectors_iterates_until_success() {
        DeviceManager::get_manager().clear_for_test();
        let first = Arc::new(FakeMsiController::new(Err(MsiError::NoVectors)));
        let second = Arc::new(FakeMsiController::new(Ok(test_allocation(80))));
        DeviceManager::get_manager().register_msi_controller(1, first.clone());
        DeviceManager::get_manager().register_msi_controller(2, second.clone());
        let manager = InterruptManager::new();

        let allocation = manager
            .allocate_msi_vectors(test_request())
            .expect("expected second controller to allocate");

        assert_eq!(first.calls(), 1);
        assert_eq!(second.calls(), 1);
        assert_eq!(allocation.vectors[0].virq, 80);
        DeviceManager::get_manager().clear_for_test();
    }
}
