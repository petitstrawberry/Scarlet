//! Interrupt management module for Scarlet kernel.
//!
//! This module provides a unified interface for interrupt handling across different
//! interrupt controllers and devices. It implements a hierarchical interrupt
//! management system with support for:
//! - Multiple interrupt controllers (PLIC, AIA, etc.)
//! - Device interrupt registration and deregistration
//! - Interrupt priority management
//! - CPU-specific interrupt routing
//!
//! ## Architecture
//!
//! ```
//! InterruptManager (Core)
//! ├── InterruptController implementations
//! │   ├── Platform-Level Interrupt Controller (PLIC)
//! │   ├── Advanced Interrupt Architecture (AIA)
//! │   └── Core Local Interruptor (CLINT) for timer/software interrupts
//! ├── Device Interrupt Handlers
//! │   ├── Timer
//! │   ├── UART
//! │   ├── VirtIO devices
//! │   └── Block devices
//! └── Interrupt Routing & Prioritization
//! ```

use alloc::boxed::Box;
use alloc::vec::Vec;
use crate::arch::Trapframe;

pub mod controller;
pub mod manager;
pub mod handlers;
pub mod init;

/// Maximum number of interrupt sources supported
pub const MAX_INTERRUPTS: usize = 256;

/// Interrupt priority levels (higher number = higher priority)
pub type InterruptPriority = u32;

/// Interrupt handler trait
///
/// All device drivers and subsystems that need to handle interrupts
/// must implement this trait.
pub trait InterruptHandler: Send + Sync {
    /// Handle an interrupt
    ///
    /// # Arguments
    /// * `irq` - The interrupt number that was triggered
    /// * `trapframe` - The current trap frame (for context switching if needed)
    ///
    /// # Returns
    /// * `Ok(())` - Interrupt handled successfully
    /// * `Err(msg)` - Error occurred during interrupt handling
    fn handle(&mut self, irq: usize, trapframe: &mut Trapframe) -> Result<(), &'static str>;

    /// Get the name of this interrupt handler (for debugging)
    fn name(&self) -> &'static str;

    /// Check if this handler can be interrupted by higher priority interrupts
    fn is_interruptible(&self) -> bool {
        true
    }
}

/// Interrupt controller trait
///
/// Different interrupt controllers (PLIC, AIA, etc.) implement this trait
/// to provide a unified interface for interrupt management.
pub trait InterruptController: Send + Sync {
    /// Initialize the interrupt controller
    fn init(&mut self) -> Result<(), &'static str>;

    /// Enable a specific interrupt
    ///
    /// # Arguments
    /// * `irq` - Interrupt number to enable
    /// * `priority` - Priority level for this interrupt
    /// * `cpu_id` - Target CPU for this interrupt (if supported)
    fn enable_interrupt(&mut self, irq: usize, priority: InterruptPriority, cpu_id: usize) -> Result<(), &'static str>;

    /// Disable a specific interrupt
    fn disable_interrupt(&mut self, irq: usize) -> Result<(), &'static str>;

    /// Set interrupt priority
    fn set_priority(&mut self, irq: usize, priority: InterruptPriority) -> Result<(), &'static str>;

    /// Get interrupt priority
    fn get_priority(&self, irq: usize) -> Result<InterruptPriority, &'static str>;

    /// Claim (acknowledge) the next pending interrupt
    ///
    /// Returns the interrupt number if one is pending, None otherwise
    fn claim_interrupt(&mut self) -> Option<usize>;

    /// Complete interrupt processing
    ///
    /// This must be called after handling an interrupt to signal completion
    fn complete_interrupt(&mut self, irq: usize) -> Result<(), &'static str>;

    /// Check if a specific interrupt is pending
    fn is_pending(&self, irq: usize) -> bool;

    /// Get the name of this interrupt controller
    fn name(&self) -> &'static str;

    /// Check if this controller supports per-CPU interrupt routing
    fn supports_cpu_routing(&self) -> bool {
        false
    }

    /// Route interrupt to specific CPU (if supported)
    fn route_to_cpu(&mut self, _irq: usize, _cpu_id: usize) -> Result<(), &'static str> {
        if !self.supports_cpu_routing() {
            return Err("CPU routing not supported by this controller");
        }
        Err("CPU routing not implemented")
    }
}

/// Interrupt information structure
#[derive(Debug, Clone)]
pub struct InterruptInfo {
    pub irq: usize,
    pub priority: InterruptPriority,
    pub cpu_id: usize,
    pub enabled: bool,
    pub handler_name: &'static str,
}

/// Common interrupt numbers for RISC-V systems
pub mod irq {
    /// Software interrupts
    pub const SOFTWARE_INTERRUPT: usize = 1;
    
    /// Timer interrupts
    pub const TIMER_INTERRUPT: usize = 5;
    
    /// External interrupts (PLIC-managed)
    pub const EXTERNAL_INTERRUPT_BASE: usize = 16;
    
    /// Common device interrupts (platform-specific)
    pub mod device {
        use super::EXTERNAL_INTERRUPT_BASE;
        
        pub const UART0: usize = EXTERNAL_INTERRUPT_BASE + 1;
        pub const UART1: usize = EXTERNAL_INTERRUPT_BASE + 2;
        pub const VIRTIO_BASE: usize = EXTERNAL_INTERRUPT_BASE + 16;
        pub const BLOCK_DEVICE_BASE: usize = EXTERNAL_INTERRUPT_BASE + 32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestInterruptHandler {
        name: &'static str,
        handled_count: usize,
    }

    impl TestInterruptHandler {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                handled_count: 0,
            }
        }
    }

    impl InterruptHandler for TestInterruptHandler {
        fn handle(&mut self, _irq: usize, _trapframe: &mut Trapframe) -> Result<(), &'static str> {
            self.handled_count += 1;
            Ok(())
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    #[test_case]
    fn test_interrupt_handler_trait() {
        let mut handler = TestInterruptHandler::new("test_handler");
        assert_eq!(handler.name(), "test_handler");
        assert_eq!(handler.handled_count, 0);
        
        // Create a dummy trapframe (in real usage, this would be from arch)
        let mut dummy_trapframe = unsafe { core::mem::zeroed::<Trapframe>() };
        
        let result = handler.handle(1, &mut dummy_trapframe);
        assert!(result.is_ok());
        assert_eq!(handler.handled_count, 1);
    }

    #[test_case]
    fn test_interrupt_info() {
        let info = InterruptInfo {
            irq: 5,
            priority: 10,
            cpu_id: 0,
            enabled: true,
            handler_name: "timer",
        };
        
        assert_eq!(info.irq, 5);
        assert_eq!(info.priority, 10);
        assert_eq!(info.cpu_id, 0);
        assert!(info.enabled);
        assert_eq!(info.handler_name, "timer");
    }
}
