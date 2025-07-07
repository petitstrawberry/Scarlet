//! Interrupt system initialization.
//!
//! This module handles the initialization of the interrupt management system,
//! including setting up controllers and registering default handlers.

use alloc::boxed::Box;
use crate::interrupt::{
    controller::{SoftwareInterruptController, plic::Plic, clint::Clint},
    manager::InterruptManager,
    handlers::timer::init_timer_interrupt,
};

/// Initialize the interrupt management system
pub fn init_interrupt_system() -> Result<(), &'static str> {
    crate::early_println!("[Interrupt] Initializing interrupt management system");

    // Initialize the global interrupt manager
    InterruptManager::init()?;

    // Register a software interrupt controller as fallback
    let sw_controller = Box::new(SoftwareInterruptController::new("fallback_sw"));
    InterruptManager::register_global_controller(sw_controller)?;

    // Register CLINT for timer and software interrupts
    // Note: Using a placeholder address - should be obtained from device tree
    let clint = Box::new(Clint::new(0x2000000)); // Default QEMU virt address
    InterruptManager::register_global_controller(clint)?;

    // Register PLIC for external interrupts
    // Note: Using a placeholder address - should be obtained from device tree  
    let plic = Box::new(Plic::new(0x0c000000, Some(53))); // Default QEMU virt address
    InterruptManager::register_global_controller(plic)?;

    crate::early_println!("[Interrupt] Interrupt management system initialized");
    Ok(())
}

/// Initialize standard interrupt handlers
pub fn init_standard_handlers() -> Result<(), &'static str> {
    crate::early_println!("[Interrupt] Initializing standard interrupt handlers");

    // Initialize timer interrupt handler
    init_timer_interrupt()?;

    crate::early_println!("[Interrupt] Standard interrupt handlers initialized");
    Ok(())
}

/// Initialize interrupt system with device tree information
pub fn init_interrupt_system_with_fdt() -> Result<(), &'static str> {
    // This will be implemented when FDT parsing for interrupt controllers is added
    crate::early_println!("[Interrupt] FDT-based interrupt initialization not yet implemented");
    Ok(())
}
