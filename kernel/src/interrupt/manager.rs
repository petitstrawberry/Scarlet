//! Interrupt Manager implementation.
//!
//! The InterruptManager is the central component that coordinates all interrupt
//! handling in the Scarlet kernel. It manages multiple interrupt controllers,
//! routes interrupts to appropriate handlers, and provides a unified API for
//! interrupt management.

use alloc::boxed::Box;
use alloc::vec::Vec;
use spin::RwLock;

use crate::arch::Trapframe;
use crate::environment::NUM_OF_CPUS;

use super::{
    InterruptController, InterruptHandler, InterruptInfo, InterruptPriority,
    MAX_INTERRUPTS
};

/// Global interrupt manager instance
static INTERRUPT_MANAGER: RwLock<Option<InterruptManager>> = RwLock::new(None);

/// Central interrupt management system
pub struct InterruptManager {
    /// Registered interrupt controllers
    controllers: Vec<Box<dyn InterruptController>>,
    
    /// Interrupt handlers indexed by IRQ number
    handlers: [Option<Box<dyn InterruptHandler>>; MAX_INTERRUPTS],
    
    /// Interrupt configuration
    interrupt_info: [Option<InterruptInfo>; MAX_INTERRUPTS],
    
    /// Per-CPU interrupt statistics
    cpu_stats: [InterruptStats; NUM_OF_CPUS],
    
    /// Global interrupt enable/disable state
    global_enabled: bool,
}

/// Per-CPU interrupt statistics
#[derive(Debug, Clone, Default)]
pub struct InterruptStats {
    pub total_interrupts: u64,
    pub handled_interrupts: u64,
    pub failed_interrupts: u64,
    pub nested_interrupts: u64,
    pub max_nesting_level: u32,
    pub current_nesting_level: u32,
}

impl InterruptManager {
    /// Create a new interrupt manager
    pub fn new() -> Self {
        Self {
            controllers: Vec::new(),
            handlers: core::array::from_fn(|_| None),
            interrupt_info: core::array::from_fn(|_| None),
            cpu_stats: core::array::from_fn(|_| InterruptStats::default()),
            global_enabled: false,
        }
    }

    /// Initialize the global interrupt manager
    pub fn init() -> Result<(), &'static str> {
        let mut manager_lock = INTERRUPT_MANAGER.write();
        if manager_lock.is_some() {
            return Err("Interrupt manager already initialized");
        }

        let manager = InterruptManager::new();
        *manager_lock = Some(manager);
        
        // Enable global interrupts after initialization
        drop(manager_lock);
        Self::enable_global_interrupts();
        
        Ok(())
    }

    /// Get reference to the global interrupt manager
    pub fn get() -> Result<&'static RwLock<Option<InterruptManager>>, &'static str> {
        Ok(&INTERRUPT_MANAGER)
    }

    /// Register an interrupt controller
    pub fn register_controller(&mut self, controller: Box<dyn InterruptController>) -> Result<(), &'static str> {
        // Initialize the controller
        let mut controller = controller;
        controller.init()?;
        
        crate::early_println!("[InterruptManager] Registered controller: {}", controller.name());
        self.controllers.push(controller);
        Ok(())
    }

    /// Register an interrupt handler for a specific IRQ
    pub fn register_handler(
        &mut self,
        irq: usize,
        handler: Box<dyn InterruptHandler>,
        priority: InterruptPriority,
        cpu_id: usize,
    ) -> Result<(), &'static str> {
        if irq >= MAX_INTERRUPTS {
            return Err("IRQ number out of range");
        }

        if self.handlers[irq].is_some() {
            return Err("Handler already registered for this IRQ");
        }

        // Find a controller that can handle this interrupt
        let mut controller_found = false;
        for controller in &mut self.controllers {
            if let Ok(()) = controller.enable_interrupt(irq, priority, cpu_id) {
                controller_found = true;
                break;
            }
        }

        if !controller_found {
            return Err("No controller available for this IRQ");
        }

        let handler_name = handler.name();
        self.handlers[irq] = Some(handler);
        self.interrupt_info[irq] = Some(InterruptInfo {
            irq,
            priority,
            cpu_id,
            enabled: true,
            handler_name,
        });

        crate::early_println!("[InterruptManager] Registered handler '{}' for IRQ {}", handler_name, irq);
        Ok(())
    }

    /// Unregister an interrupt handler
    pub fn unregister_handler(&mut self, irq: usize) -> Result<(), &'static str> {
        if irq >= MAX_INTERRUPTS {
            return Err("IRQ number out of range");
        }

        if self.handlers[irq].is_none() {
            return Err("No handler registered for this IRQ");
        }

        // Disable the interrupt in all controllers
        for controller in &mut self.controllers {
            let _ = controller.disable_interrupt(irq);
        }

        let handler_name = self.handlers[irq].as_ref().unwrap().name();
        self.handlers[irq] = None;
        self.interrupt_info[irq] = None;

        crate::early_println!("[InterruptManager] Unregistered handler '{}' for IRQ {}", handler_name, irq);
        Ok(())
    }

    /// Handle an interrupt
    pub fn handle_interrupt(&mut self, irq: usize, trapframe: &mut Trapframe) -> Result<(), &'static str> {
        let cpu_id = trapframe.get_cpuid();
        
        // Update statistics
        self.cpu_stats[cpu_id].total_interrupts += 1;
        self.cpu_stats[cpu_id].current_nesting_level += 1;
        if self.cpu_stats[cpu_id].current_nesting_level > 1 {
            self.cpu_stats[cpu_id].nested_interrupts += 1;
        }
        if self.cpu_stats[cpu_id].current_nesting_level > self.cpu_stats[cpu_id].max_nesting_level {
            self.cpu_stats[cpu_id].max_nesting_level = self.cpu_stats[cpu_id].current_nesting_level;
        }

        let result = if let Some(ref mut handler) = self.handlers[irq] {
            handler.handle(irq, trapframe)
        } else {
            crate::println!("[InterruptManager] No handler for IRQ {}", irq);
            Err("No handler registered for this IRQ")
        };

        // Complete interrupt processing in controllers
        for controller in &mut self.controllers {
            let _ = controller.complete_interrupt(irq);
        }

        // Update statistics based on result
        match result {
            Ok(()) => self.cpu_stats[cpu_id].handled_interrupts += 1,
            Err(_) => self.cpu_stats[cpu_id].failed_interrupts += 1,
        }

        self.cpu_stats[cpu_id].current_nesting_level -= 1;
        result
    }

    /// Enable a specific interrupt
    pub fn enable_interrupt(&mut self, irq: usize) -> Result<(), &'static str> {
        if irq >= MAX_INTERRUPTS {
            return Err("IRQ number out of range");
        }

        if let Some(ref mut info) = self.interrupt_info[irq] {
            info.enabled = true;
            
            // Enable in all controllers
            for controller in &mut self.controllers {
                let _ = controller.enable_interrupt(irq, info.priority, info.cpu_id);
            }
            Ok(())
        } else {
            Err("No handler registered for this IRQ")
        }
    }

    /// Disable a specific interrupt
    pub fn disable_interrupt(&mut self, irq: usize) -> Result<(), &'static str> {
        if irq >= MAX_INTERRUPTS {
            return Err("IRQ number out of range");
        }

        if let Some(ref mut info) = self.interrupt_info[irq] {
            info.enabled = false;
            
            // Disable in all controllers
            for controller in &mut self.controllers {
                let _ = controller.disable_interrupt(irq);
            }
            Ok(())
        } else {
            Err("No handler registered for this IRQ")
        }
    }

    /// Set interrupt priority
    pub fn set_priority(&mut self, irq: usize, priority: InterruptPriority) -> Result<(), &'static str> {
        if irq >= MAX_INTERRUPTS {
            return Err("IRQ number out of range");
        }

        if let Some(ref mut info) = self.interrupt_info[irq] {
            info.priority = priority;
            
            // Update priority in all controllers
            for controller in &mut self.controllers {
                let _ = controller.set_priority(irq, priority);
            }
            Ok(())
        } else {
            Err("No handler registered for this IRQ")
        }
    }

    /// Get interrupt information
    pub fn get_interrupt_info(&self, irq: usize) -> Option<&InterruptInfo> {
        if irq < MAX_INTERRUPTS {
            self.interrupt_info[irq].as_ref()
        } else {
            None
        }
    }

    /// Get all registered interrupts
    pub fn get_all_interrupts(&self) -> Vec<InterruptInfo> {
        self.interrupt_info
            .iter()
            .filter_map(|info| info.as_ref())
            .cloned()
            .collect()
    }

    /// Get CPU interrupt statistics
    pub fn get_cpu_stats(&self, cpu_id: usize) -> Option<&InterruptStats> {
        if cpu_id < NUM_OF_CPUS {
            Some(&self.cpu_stats[cpu_id])
        } else {
            None
        }
    }

    /// Enable global interrupts
    pub fn enable_global_interrupts() {
        crate::arch::enable_interrupt();
    }

    /// Disable global interrupts
    pub fn disable_global_interrupts() {
        crate::arch::disable_interrupt();
    }

    /// Check if global interrupts are enabled
    pub fn is_global_enabled(&self) -> bool {
        self.global_enabled
    }
}

/// Helper functions for accessing the global interrupt manager
impl InterruptManager {
    /// Register a handler with the global interrupt manager
    pub fn register_global_handler(
        irq: usize,
        handler: Box<dyn InterruptHandler>,
        priority: InterruptPriority,
        cpu_id: usize,
    ) -> Result<(), &'static str> {
        let mut manager_lock = INTERRUPT_MANAGER.write();
        if let Some(ref mut manager) = *manager_lock {
            manager.register_handler(irq, handler, priority, cpu_id)
        } else {
            Err("Interrupt manager not initialized")
        }
    }

    /// Handle interrupt with the global interrupt manager
    pub fn handle_global_interrupt(irq: usize, trapframe: &mut Trapframe) -> Result<(), &'static str> {
        let mut manager_lock = INTERRUPT_MANAGER.write();
        if let Some(ref mut manager) = *manager_lock {
            manager.handle_interrupt(irq, trapframe)
        } else {
            Err("Interrupt manager not initialized")
        }
    }

    /// Register controller with the global interrupt manager
    pub fn register_global_controller(controller: Box<dyn InterruptController>) -> Result<(), &'static str> {
        let mut manager_lock = INTERRUPT_MANAGER.write();
        if let Some(ref mut manager) = *manager_lock {
            manager.register_controller(controller)
        } else {
            Err("Interrupt manager not initialized")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestController {
        name: &'static str,
        enabled_interrupts: Vec<usize>,
    }

    impl TestController {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                enabled_interrupts: Vec::new(),
            }
        }
    }

    impl InterruptController for TestController {
        fn init(&mut self) -> Result<(), &'static str> {
            Ok(())
        }

        fn enable_interrupt(&mut self, irq: usize, _priority: InterruptPriority, _cpu_id: usize) -> Result<(), &'static str> {
            if !self.enabled_interrupts.contains(&irq) {
                self.enabled_interrupts.push(irq);
            }
            Ok(())
        }

        fn disable_interrupt(&mut self, irq: usize) -> Result<(), &'static str> {
            self.enabled_interrupts.retain(|&x| x != irq);
            Ok(())
        }

        fn set_priority(&mut self, _irq: usize, _priority: InterruptPriority) -> Result<(), &'static str> {
            Ok(())
        }

        fn get_priority(&self, _irq: usize) -> Result<InterruptPriority, &'static str> {
            Ok(0)
        }

        fn claim_interrupt(&mut self) -> Option<usize> {
            None
        }

        fn complete_interrupt(&mut self, _irq: usize) -> Result<(), &'static str> {
            Ok(())
        }

        fn is_pending(&self, _irq: usize) -> bool {
            false
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    struct TestHandler {
        name: &'static str,
        call_count: core::sync::atomic::AtomicUsize,
    }

    impl TestHandler {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                call_count: core::sync::atomic::AtomicUsize::new(0),
            }
        }
    }

    impl InterruptHandler for TestHandler {
        fn handle(&mut self, _irq: usize, _trapframe: &mut Trapframe) -> Result<(), &'static str> {
            self.call_count.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            Ok(())
        }

        fn name(&self) -> &'static str {
            self.name
        }
        
        fn is_interruptible(&self) -> bool {
            true
        }
    }

    #[test_case]
    fn test_interrupt_manager_creation() {
        let manager = InterruptManager::new();
        assert!(!manager.is_global_enabled());
        assert_eq!(manager.controllers.len(), 0);
    }

    #[test_case]
    fn test_controller_registration() {
        let mut manager = InterruptManager::new();
        let controller = Box::new(TestController::new("test_controller"));
        
        let result = manager.register_controller(controller);
        assert!(result.is_ok());
        assert_eq!(manager.controllers.len(), 1);
    }

    #[test_case]
    fn test_handler_registration() {
        let mut manager = InterruptManager::new();
        let controller = Box::new(TestController::new("test_controller"));
        manager.register_controller(controller).unwrap();
        
        let handler = Box::new(TestHandler::new("test_handler"));
        let result = manager.register_handler(5, handler, 10, 0);
        assert!(result.is_ok());
        
        let info = manager.get_interrupt_info(5);
        assert!(info.is_some());
        assert_eq!(info.unwrap().irq, 5);
        assert_eq!(info.unwrap().priority, 10);
        assert_eq!(info.unwrap().handler_name, "test_handler");
    }
}
