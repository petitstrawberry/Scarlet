//! Interrupt management system
//!
//! This module provides a comprehensive interrupt management system for the Scarlet kernel.
//! It supports both local interrupts (via CLINT) and external interrupts (via PLIC) on RISC-V architecture.

use alloc::sync::Arc;
use core::fmt;
use hashbrown::HashMap;

use crate::arch::{self, interrupt::enable_external_interrupts};

pub mod controllers;

static INTERRUPT_MANAGER: spin::Once<InterruptManager> = spin::Once::new();

/// Interrupt ID type
pub type InterruptId = u32;

/// CPU ID type
pub type CpuId = u32;

/// Priority level for interrupts
pub type Priority = u32;

/// Handler function type for external interrupts
pub type ExternalInterruptHandler = fn(&mut InterruptHandle) -> InterruptResult<()>;

/// Handler function type for local interrupts (timer, software)
pub type LocalInterruptHandler =
    fn(cpu_id: CpuId, interrupt_type: controllers::LocalInterruptType) -> InterruptResult<()>;

pub struct InterruptManager {
    controllers: spin::Once<spin::Mutex<controllers::InterruptControllers>>,
    external_handlers: spin::Lazy<spin::Mutex<HashMap<InterruptId, ExternalInterruptHandler>>>,
    interrupt_devices: spin::Lazy<
        spin::Mutex<HashMap<InterruptId, Arc<dyn crate::device::events::InterruptCapableDevice>>>,
    >,
}

impl InterruptManager {
    pub fn new() -> Self {
        Self {
            controllers: spin::Once::new(),
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
        disable_interrupts();
        crate::early_println!("[interrupt] init: local controllers...");

        let mut controllers = self.controllers().lock();
        match controllers.init_local_controllers() {
            Ok(()) => {}
            Err(e) => {
                crate::early_println!("Failed to initialize local controllers: {}", e);
            }
        }

        crate::early_println!("[interrupt] init: local controllers done");
        crate::early_println!("[interrupt] init: external controller...");

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

        if let Some(controller) = controllers.local_controller_mut_for_cpu(cpu_id) {
            if let Err(e) = controller.init(cpu_id) {
                crate::early_println!(
                    "[interrupt] AP {}: failed to init local controller: {}",
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

    pub fn handle_external_interrupt(
        &self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
        let device = {
            let devices = self.interrupt_devices.lock();
            devices.get(&interrupt_id).cloned()
        };

        if let Some(device) = device {
            device.handle_interrupt()?;
            self.complete_external_interrupt(cpu_id, interrupt_id)
        } else {
            let handler = {
                let handlers = self.external_handlers.lock();
                handlers.get(&interrupt_id).copied()
            };

            if let Some(handler_fn) = handler {
                let mut handle = InterruptHandle::new(interrupt_id, cpu_id);
                handler_fn(&mut handle)
            } else {
                self.complete_external_interrupt(cpu_id, interrupt_id)
            }
        }
    }

    pub fn claim_and_handle_external_interrupt(
        &self,
        cpu_id: CpuId,
    ) -> InterruptResult<Option<InterruptId>> {
        let interrupt_id = {
            let controllers = self.controllers().lock();
            if let Some(controller) = controllers.external_controller() {
                controller.claim_interrupt(cpu_id)?
            } else {
                return Err(InterruptError::ControllerNotFound);
            }
        };

        if let Some(id) = interrupt_id {
            self.handle_external_interrupt(id, cpu_id)?;
            Ok(Some(id))
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
        if let Some(controller) = controllers.local_controller_for_cpu(cpu_id) {
            controller.enable_interrupt(cpu_id, interrupt_type)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn disable_local_interrupt(
        &self,
        cpu_id: CpuId,
        interrupt_type: controllers::LocalInterruptType,
    ) -> InterruptResult<()> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.local_controller_for_cpu(cpu_id) {
            controller.disable_interrupt(cpu_id, interrupt_type)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn send_software_interrupt(&self, target_cpu: CpuId) -> InterruptResult<()> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.local_controller_for_cpu(target_cpu) {
            controller.send_software_interrupt(target_cpu)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn set_timer(&self, cpu_id: CpuId, time: u64) -> InterruptResult<()> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.local_controller_for_cpu(cpu_id) {
            controller.set_timer(cpu_id, time)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn get_time(&self, cpu_id: CpuId) -> InterruptResult<u64> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.local_controller_for_cpu(cpu_id) {
            Ok(controller.get_time())
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn get_timer_frequency_hz(&self, cpu_id: CpuId) -> InterruptResult<u64> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.local_controller_for_cpu(cpu_id) {
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
        controllers
            .local_controller_for_cpu(cpu_id)
            .map(|controller| controller.is_pending(cpu_id, interrupt_type))
            .unwrap_or(false)
    }

    pub fn register_local_controller(
        &self,
        controller: alloc::boxed::Box<dyn controllers::LocalInterruptController>,
        cpu_ids: &[CpuId],
    ) -> InterruptResult<usize> {
        let mut controllers = self.controllers().lock();
        Ok(controllers.register_local_controller(controller, cpu_ids))
    }

    pub fn register_local_controller_for_range(
        &self,
        controller: alloc::boxed::Box<dyn controllers::LocalInterruptController>,
        cpu_range: core::ops::Range<CpuId>,
    ) -> InterruptResult<usize> {
        let mut controllers = self.controllers().lock();
        Ok(controllers.register_local_controller_for_range(controller, cpu_range))
    }

    pub fn register_local_controller_for_cpu(
        &self,
        controller: alloc::boxed::Box<dyn controllers::LocalInterruptController>,
        cpu_id: CpuId,
    ) -> InterruptResult<usize> {
        let mut controllers = self.controllers().lock();
        Ok(controllers.register_local_controller_for_cpu(controller, cpu_id))
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
        handlers.insert(interrupt_id, handler);
        Ok(())
    }

    pub fn register_interrupt_device(
        &self,
        interrupt_id: InterruptId,
        device: Arc<dyn crate::device::events::InterruptCapableDevice>,
    ) -> InterruptResult<()> {
        let mut devices = self.interrupt_devices.lock();
        if devices.contains_key(&interrupt_id) {
            return Err(InterruptError::HandlerAlreadyRegistered);
        }
        devices.insert(interrupt_id, device);
        Ok(())
    }

    pub fn complete_external_interrupt(
        &self,
        cpu_id: CpuId,
        interrupt_id: InterruptId,
    ) -> InterruptResult<()> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.external_controller() {
            controller.complete_interrupt(cpu_id, interrupt_id)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn enable_external_interrupt(
        &self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.external_controller() {
            controller.enable_interrupt(interrupt_id, cpu_id)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn disable_external_interrupt(
        &self,
        interrupt_id: InterruptId,
        cpu_id: CpuId,
    ) -> InterruptResult<()> {
        let controllers = self.controllers().lock();
        if let Some(controller) = controllers.external_controller() {
            controller.disable_interrupt(interrupt_id, cpu_id)
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
    interrupt_id: InterruptId,
    cpu_id: CpuId,
    completed: bool,
}

impl InterruptHandle {
    /// Create a new interrupt handle
    pub fn new(interrupt_id: InterruptId, cpu_id: CpuId) -> Self {
        Self {
            interrupt_id,
            cpu_id,
            completed: false,
        }
    }

    /// Get the interrupt ID
    pub fn interrupt_id(&self) -> InterruptId {
        self.interrupt_id
    }

    /// Get the CPU ID
    pub fn cpu_id(&self) -> CpuId {
        self.cpu_id
    }

    /// Mark the interrupt as completed
    ///
    /// This should be called when the handler has finished processing the interrupt.
    pub fn complete(&mut self) -> InterruptResult<()> {
        if self.completed {
            return Err(InterruptError::InvalidOperation);
        }

        InterruptManager::global().complete_external_interrupt(self.cpu_id, self.interrupt_id)?;
        self.completed = true;
        Ok(())
    }

    /// Check if the interrupt has been completed
    pub fn is_completed(&self) -> bool {
        self.completed
    }

    /// Enable another interrupt
    pub fn enable_interrupt(&self, target_interrupt: InterruptId) -> InterruptResult<()> {
        InterruptManager::global().enable_external_interrupt(target_interrupt, self.cpu_id)
    }

    /// Disable another interrupt
    pub fn disable_interrupt(&self, target_interrupt: InterruptId) -> InterruptResult<()> {
        InterruptManager::global().disable_external_interrupt(target_interrupt, self.cpu_id)
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

/// Enable CPU interrupt reception (stage 2).
///
/// This should run after device drivers have registered their handlers and
/// enabled the desired interrupt lines in the controller.
pub fn enable_cpu_interrupts() {
    enable_external_interrupts();
}
