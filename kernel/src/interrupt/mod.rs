//! Interrupt management system
//! 
//! This module provides a comprehensive interrupt management system for the Scarlet kernel.
//! It supports both local interrupts (via CLINT) and external interrupts (via PLIC) on RISC-V architecture.

use core::fmt;
use hashbrown::HashMap;

use crate::arch::{self, interrupt::enable_external_interrupts};

pub mod controllers;

/// Interrupt ID type
pub type InterruptId = u32;

/// CPU ID type
pub type CpuId = u32;

/// Priority level for interrupts
pub type Priority = u32;


/// Handle for managing interrupt processing
/// 
/// This provides a safe interface for interrupt handlers to interact with
/// the interrupt controller without direct access.
pub struct InterruptHandle<'a> {
    interrupt_id: InterruptId,
    cpu_id: CpuId,
    completed: bool,
    manager: &'a mut InterruptManager,
}

impl<'a> InterruptHandle<'a> {
    /// Create a new interrupt handle
    pub fn new(interrupt_id: InterruptId, cpu_id: CpuId, manager: &'a mut InterruptManager) -> Self {
        Self {
            interrupt_id,
            cpu_id,
            completed: false,
            manager,
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
        
        self.manager.complete_external_interrupt(self.cpu_id, self.interrupt_id)?;
        self.completed = true;
        Ok(())
    }

    /// Check if the interrupt has been completed
    pub fn is_completed(&self) -> bool {
        self.completed
    }

    /// Enable another interrupt
    pub fn enable_interrupt(&mut self, target_interrupt: InterruptId) -> InterruptResult<()> {
        self.manager.enable_external_interrupt(target_interrupt, self.cpu_id)
    }

    /// Disable another interrupt
    pub fn disable_interrupt(&mut self, target_interrupt: InterruptId) -> InterruptResult<()> {
        self.manager.disable_external_interrupt(target_interrupt, self.cpu_id)
    }
}

impl<'a> Drop for InterruptHandle<'a> {
    fn drop(&mut self) {
        if !self.completed {
            // Auto-complete if not manually completed
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

/// Unified interrupt manager
/// 
/// This manages both local and external interrupts in a single structure.
pub struct InterruptManager {
    controllers: controllers::InterruptControllers,
    external_handlers: spin::Mutex<HashMap<InterruptId, ExternalInterruptHandler>>,
    interrupt_devices: spin::Mutex<HashMap<InterruptId, alloc::sync::Arc<dyn crate::device::events::InterruptCapableDevice>>>,
}

impl InterruptManager {

    /// Create a new interrupt manager
    pub fn new() -> Self {
        Self {
            controllers: controllers::InterruptControllers::new(),
            external_handlers: spin::Mutex::new(HashMap::new()),
            interrupt_devices: spin::Mutex::new(HashMap::new()),
        }
    }

    /// Get a reference to the global interrupt manager
    pub fn global() -> &'static spin::Mutex<InterruptManager> {
        static INTERRUPT_MANAGER: spin::Once<spin::Mutex<InterruptManager>> = spin::Once::new();
        INTERRUPT_MANAGER.call_once(|| spin::Mutex::new(InterruptManager::new()))
    }

    /// Get a mutable reference to the global interrupt manager (convenience method)
    /// 
    /// This method locks the global manager and returns a guard.
    /// Use this when you need to perform multiple operations atomically.
    pub fn get_manager() -> spin::MutexGuard<'static, InterruptManager> {
        Self::global().lock()
    }

    /// Execute a closure with mutable access to the global interrupt manager
    /// 
    /// This is a convenience method that automatically handles locking and unlocking.
    pub fn with_manager<F, R>(f: F) -> R
    where
        F: FnOnce(&mut InterruptManager) -> R,
    {
        f(&mut Self::global().lock())
    }

    pub fn init(&mut self) {
        // Initialize local controllers (e.g., CLINT)
        match self.controllers.init_local_controllers() {
            Ok(()) => {}
            Err(e) => {
                crate::early_println!("Failed to initialize local controllers: {}", e);
            }
        }

        // Initialize external controller (e.g., PLIC)
        match self.controllers.init_external_controller() {
            Ok(()) => {}
            Err(e) => {
                crate::early_println!("Failed to initialize external controller: {}", e);
            }
        }

        
        enable_external_interrupts(); // Enable external interrupts
        // Timer interrupts are disabled by default, enable them if needed by scheduler or other components
        enable_interrupts(); // Enable interrupts globally
    }

    /// Handle an external interrupt
    pub fn handle_external_interrupt(&mut self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()> {
        // First, check for device-based handlers
        let device = {
            let devices = self.interrupt_devices.lock();
            devices.get(&interrupt_id).cloned()
        };
        
        if let Some(device) = device {
            // Call device's interrupt handler
            device.handle_interrupt()?;
            self.complete_external_interrupt(cpu_id, interrupt_id)
        } else {
            // Fall back to function-based handlers
            let handler = {
                let handlers = self.external_handlers.lock();
                handlers.get(&interrupt_id).copied()
            };
            
            if let Some(handler_fn) = handler {
                let mut handle = InterruptHandle::new(interrupt_id, cpu_id, self);
                handler_fn(&mut handle)
            } else {
                // No handler registered - just complete the interrupt
                self.complete_external_interrupt(cpu_id, interrupt_id)
            }
        }
    }

    /// Claim and handle the next pending external interrupt
    pub fn claim_and_handle_external_interrupt(&mut self, cpu_id: CpuId) -> InterruptResult<Option<InterruptId>> {
        let interrupt_id = if let Some(ref mut controller) = self.controllers.external_controller_mut() {
            controller.claim_interrupt(cpu_id)?
        } else {
            return Err(InterruptError::ControllerNotFound);
        };

        if let Some(id) = interrupt_id {
            self.handle_external_interrupt(id, cpu_id)?;
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// Enable a local interrupt type for a CPU
    pub fn enable_local_interrupt(&mut self, cpu_id: CpuId, interrupt_type: controllers::LocalInterruptType) -> InterruptResult<()> {
        if let Some(ref mut controller) = self.controllers.local_controller_mut_for_cpu(cpu_id) {
            controller.enable_interrupt(cpu_id, interrupt_type)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    /// Disable a local interrupt type for a CPU
    pub fn disable_local_interrupt(&mut self, cpu_id: CpuId, interrupt_type: controllers::LocalInterruptType) -> InterruptResult<()> {
        if let Some(ref mut controller) = self.controllers.local_controller_mut_for_cpu(cpu_id) {
            controller.disable_interrupt(cpu_id, interrupt_type)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    /// Send a software interrupt to a specific CPU
    pub fn send_software_interrupt(&mut self, target_cpu: CpuId) -> InterruptResult<()> {
        if let Some(ref mut controller) = self.controllers.local_controller_mut_for_cpu(target_cpu) {
            controller.send_software_interrupt(target_cpu)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    /// Set timer interrupt for a specific CPU
    pub fn set_timer(&mut self, cpu_id: CpuId, time: u64) -> InterruptResult<()> {
        if let Some(ref mut controller) = self.controllers.local_controller_mut_for_cpu(cpu_id) {
            controller.set_timer(cpu_id, time)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    pub fn get_time(&self, cpu_id: CpuId) -> InterruptResult<u64> {
        if let Some(ref controller) = self.controllers.local_controller_for_cpu(cpu_id) {
            Ok(controller.get_time())
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    /// Get timer frequency (Hz) for a specific CPU
    pub fn get_timer_frequency_hz(&self, cpu_id: CpuId) -> InterruptResult<u64> {
        if let Some(controller) = self.controllers.local_controller_for_cpu(cpu_id) {
            Ok(controller.get_timer_frequency_hz())
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    /// Register a local interrupt controller (e.g., CLINT) for specific CPUs
    pub fn register_local_controller(&mut self, controller: alloc::boxed::Box<dyn controllers::LocalInterruptController>, cpu_ids: &[CpuId]) -> InterruptResult<usize> {
        Ok(self.controllers.register_local_controller(controller, cpu_ids))
    }

    /// Register a local interrupt controller for a CPU range
    pub fn register_local_controller_for_range(&mut self, controller: alloc::boxed::Box<dyn controllers::LocalInterruptController>, cpu_range: core::ops::Range<CpuId>) -> InterruptResult<usize> {
        Ok(self.controllers.register_local_controller_for_range(controller, cpu_range))
    }

    /// Register a local interrupt controller for a single CPU
    pub fn register_local_controller_for_cpu(&mut self, controller: alloc::boxed::Box<dyn controllers::LocalInterruptController>, cpu_id: CpuId) -> InterruptResult<usize> {
        Ok(self.controllers.register_local_controller_for_cpu(controller, cpu_id))
    }

    /// Register an external interrupt controller (e.g., PLIC)
    pub fn register_external_controller(&mut self, controller: alloc::boxed::Box<dyn controllers::ExternalInterruptController>) -> InterruptResult<()> {
        if self.controllers.has_external_controller() {
            return Err(InterruptError::HardwareError);
        }
        self.controllers.register_external_controller(controller);
        Ok(())
    }

    /// Register a handler for a specific external interrupt
    pub fn register_external_handler(&mut self, interrupt_id: InterruptId, handler: ExternalInterruptHandler) -> InterruptResult<()> {
        let mut handlers = self.external_handlers.lock();
        if handlers.contains_key(&interrupt_id) {
            return Err(InterruptError::HandlerAlreadyRegistered);
        }
        handlers.insert(interrupt_id, handler);
        Ok(())
    }

    /// Register a device-based handler for a specific external interrupt
    pub fn register_interrupt_device(&mut self, interrupt_id: InterruptId, device: alloc::sync::Arc<dyn crate::device::events::InterruptCapableDevice>) -> InterruptResult<()> {
        let mut devices = self.interrupt_devices.lock();
        if devices.contains_key(&interrupt_id) {
            return Err(InterruptError::HandlerAlreadyRegistered);
        }
        devices.insert(interrupt_id, device);
        Ok(())
    }

    /// Complete an external interrupt
    pub fn complete_external_interrupt(&mut self, cpu_id: CpuId, interrupt_id: InterruptId) -> InterruptResult<()> {
        if let Some(ref mut controller) = self.controllers.external_controller_mut() {
            controller.complete_interrupt(cpu_id, interrupt_id)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    /// Enable an external interrupt for a specific CPU
    pub fn enable_external_interrupt(&mut self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()> {
        if let Some(ref mut controller) = self.controllers.external_controller_mut() {
            controller.enable_interrupt(interrupt_id, cpu_id)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    /// Disable an external interrupt for a specific CPU
    pub fn disable_external_interrupt(&mut self, interrupt_id: InterruptId, cpu_id: CpuId) -> InterruptResult<()> {
        if let Some(ref mut controller) = self.controllers.external_controller_mut() {
            controller.disable_interrupt(interrupt_id, cpu_id)
        } else {
            Err(InterruptError::ControllerNotFound)
        }
    }

    /// Check if local interrupt controller is registered
    pub fn has_local_controller(&self) -> bool {
        self.controllers.has_local_controller()
    }

    /// Check if external interrupt controller is registered
    pub fn has_external_controller(&self) -> bool {
        self.controllers.has_external_controller()
    }
}

/// Handler function type for external interrupts
pub type ExternalInterruptHandler = fn(&mut InterruptHandle) -> InterruptResult<()>;

/// Handler function type for local interrupts (timer, software)
pub type LocalInterruptHandler = fn(cpu_id: CpuId, interrupt_type: controllers::LocalInterruptType) -> InterruptResult<()>;