//! Flattened Device Tree (FDT) management module.
//! 
//! This module provides functionality for managing the Flattened Device Tree (FDT),
//! which is a data structure used to describe the hardware components of a system.
//!
//! # Overview
//!
//! The FDT is passed by the bootloader to the kernel and contains information about
//! the hardware configuration of the system. This module parses and provides access
//! to that information.
//!
//! # Core Components
//!
//! - `FdtManager`: A singleton that manages access to the parsed FDT
//! - `init_fdt()`: Function to initialize the FDT subsystem
//!
//! # Usage
//!
//! Before using the FDT functions, you must:
//! 1. Set the FDT address using `FdtManager::set_fdt_addr()`
//! 2. Call `init_fdt()` to parse the FDT
//!
//! After initialization, you can access the FDT using the static manager:
//! ```
//! let fdt_manager = FdtManager::get_manager();
//! if let Some(fdt) = fdt_manager.get_fdt() {
//!     // Use the FDT data
//! }
//! ```
//!
//! # Implementation Details
//!
//! The module uses the `fdt` crate to parse the device tree. It maintains a static
//! global manager instance to provide access throughout the kernel. The FDT address
//! is stored in a static variable that must be set before initialization.


use fdt::{Fdt, FdtError};

use crate::early_println;
use crate::early_print;

#[unsafe(link_section = ".data")]
static mut FDT_ADDR: usize = 0;

static mut MANAGER: FdtManager = FdtManager::new();


pub struct FdtManager<'a> {
    fdt: Option<Fdt<'a>>,
}

impl<'a> FdtManager<'a> {
    const fn new() -> Self {
        FdtManager {
            fdt: None,
        }
    }

    pub fn init(&mut self) -> Result<(), FdtError> {
        match unsafe { Fdt::from_ptr(FDT_ADDR as *const u8) } {
            Ok(fdt) => {
                self.fdt = Some(fdt);
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }

    pub fn set_fdt_addr(addr: usize) {
        unsafe {
            FDT_ADDR = addr;
        }
    }
    
    pub fn get_fdt(&self) -> Option<&Fdt<'a>> {
        self.fdt.as_ref()
    }

    /// Returns a mutable reference to the FdtManager.
    /// This is unsafe because it allows mutable access to a static variable.
    /// It should be used with caution to avoid data races.
    /// 
    /// # Safety
    /// This function provides mutable access to the static FdtManager instance.
    /// Ensure that no other references to the manager are active to prevent data races.
    /// 
    /// # Returns
    /// A mutable reference to the static FdtManager instance.
    #[allow(static_mut_refs)]
    pub unsafe fn get_mut_manager() -> &'static mut FdtManager<'a> {
        unsafe { &mut MANAGER }
    }

    /// Returns a reference to the FdtManager.
    /// 
    /// # Returns
    /// A reference to the static FdtManager instance.
    #[allow(static_mut_refs)]
    pub fn get_manager() -> &'static FdtManager<'a> {
        unsafe { &MANAGER }
    }
}

/// Initializes the FDT subsystem.
pub fn init_fdt() {
    let fdt_manager = unsafe { FdtManager::get_mut_manager() };
    match fdt_manager.init() {
        Ok(_) => {
            early_println!("FDT initialized");
            let fdt =  fdt_manager.get_fdt().unwrap();
            
            match fdt.chosen().bootargs() {
                Some(bootargs) => early_println!("Bootargs: {}", bootargs),
                None => early_println!("No bootargs found"),
            }
            let model = fdt.root().model();
            early_println!("Model: {}", model);
        }
        Err(e) => {
            early_println!("FDT error: {:?}", e);
        }
    }
}