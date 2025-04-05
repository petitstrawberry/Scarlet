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


use core::panic;
use core::result::Result;

use fdt::{Fdt, FdtError};

use crate::early_println;
use crate::early_print;
use crate::mem::kmalloc;
use crate::vm::vmem::MemoryArea;

#[unsafe(link_section = ".data")]
static mut FDT_ADDR: usize = 0;

static mut MANAGER: FdtManager = FdtManager::new();


pub struct FdtManager<'a> {
    fdt: Option<Fdt<'a>>,
    relocated: bool,
}

impl<'a> FdtManager<'a> {
    const fn new() -> Self {
        FdtManager {
            fdt: None,
            relocated: false,
        }
    }

    pub fn init(&mut self, ptr: *const u8) -> Result<(), FdtError> {
        match unsafe { Fdt::from_ptr(ptr) } {
            Ok(fdt) => {
                self.fdt = Some(fdt);
            }
            Err(e) => return Err(e),
        }
        Ok(())
    }

    /// Sets the FDT address.
    /// 
    /// # Safety
    /// This function modifies a static variable that holds the FDT address.
    /// Ensure that this function is called before any other FDT-related functions
    /// to avoid undefined behavior.
    /// 
    /// # Arguments
    /// 
    /// * `addr`: The address of the FDT.
    /// 
    /// # Notes
    /// 
    /// This function must be called before initializing the FDT manager.
    /// After once FdtManager is initialized, you cannot change the address.
    /// 
    pub unsafe fn set_fdt_addr(addr: usize) {
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

    /// Relocates the FDT to a new address.
    /// 
    /// # Safety
    /// This function modifies the static FDT address and reinitializes the FdtManager.
    /// Ensure that the new address is valid
    /// and does not cause memory corruption.
    ///
    /// # Parameters
    /// - `ptr`: The pointer to the new FDT address.
    ///
    /// # Panics
    /// 
    /// This function will panic if the FDT has already been relocated.
    /// 
    unsafe fn relocate_fdt(&mut self, ptr: *mut u8) {
        if self.relocated {
            panic!("FDT already relocated");
        }
        // Copy the FDT to the new address
        let size = self.get_fdt().unwrap().total_size();
        let old_ptr = unsafe { FDT_ADDR } as *const u8;
        unsafe { core::ptr::copy_nonoverlapping(old_ptr, ptr, size) };

        // Reinitialize the FDT with the new address
        match self.init(ptr) {
            Ok(_) => {
                self.relocated = true;
                early_println!("FDT relocated to address: {:#x}", ptr as usize);
            }
            Err(e) => {
                panic!("Failed to relocate FDT: {:?}", e);
            }
        }
    }

    /// Get the initramfs memory area from the device tree
    ///
    /// This function searches for the initramfs memory region in the device tree.
    /// It looks for the initrd-start/end or linux,initrd-start/end properties
    /// in the /chosen node.
    ///
    /// # Returns
    /// Option<MemoryArea>: If the initramfs region is found, returns Some(MemoryArea),
    /// otherwise returns None.
    pub fn get_initramfs(&self) -> Option<MemoryArea> {
        let fdt = self.get_fdt()?;
        
        // Find the /chosen node which contains initramfs information
        let chosen_node = fdt.find_node("/chosen")?;
        
        // Try to find initramfs start address
        // First check for "linux,initrd-start" property
        let start_addr = if let Some(prop) = chosen_node.property("linux,initrd-start") {
            if prop.value.len() == 8 {
                let val = u64::from_be_bytes([
                    prop.value[0],
                    prop.value[1],
                    prop.value[2],
                    prop.value[3],
                    prop.value[4],
                    prop.value[5],
                    prop.value[6],
                    prop.value[7],
                ]);
                Some(val as usize)
            } else if prop.value.len() == 4 {
                let val = u32::from_be_bytes([
                    prop.value[0],
                    prop.value[1],
                    prop.value[2],
                    prop.value[3],
                ]);
                Some(val as usize)
            } else {
                None
            }
        // Then check for "initrd-start" property
        } else if let Some(prop) = chosen_node.property("initrd-start") {
            if prop.value.len() >= 4 {
                let val = u32::from_be_bytes([
                    prop.value[0],
                    prop.value[1],
                    prop.value[2],
                    prop.value[3],
                ]);
                Some(val as usize)
            } else {
                None
            }
        } else {
            None
        };
        
        // Try to find initramfs end address
        // First check for "linux,initrd-end" property
        let end_addr = if let Some(prop) = chosen_node.property("linux,initrd-end") {
            if prop.value.len() >= 4 {
                let val = u32::from_be_bytes([
                    prop.value[0],
                    prop.value[1],
                    prop.value[2],
                    prop.value[3],
                ]);
                Some(val as usize)
            } else {
                None
            }
        // Then check for "initrd-end" property
        } else if let Some(prop) = chosen_node.property("initrd-end") {
            if prop.value.len() >= 4 {
                let val = u32::from_be_bytes([
                    prop.value[0],
                    prop.value[1],
                    prop.value[2],
                    prop.value[3],
                ]);
                Some(val as usize)
            } else {
                None
            }
        } else {
            None
        };

        // If we have both start and end addresses, create a memory area
        if let (Some(start), Some(end)) = (start_addr, end_addr) {
            if end <= start {
                return None;
            }
            
            let size = end - start;
            early_println!("[InitRamFS] Found initramfs: start={:#x}, end={:#x}, size={} bytes", 
                start, end, size);
            
            let memory_area = MemoryArea::new(start, end - 1);
            return Some(memory_area);
        }
        
        early_println!("[InitRamFS] No initramfs found in device tree");
        None
    }
}

/// Initializes the FDT subsystem.
pub fn init_fdt() {
    let fdt_manager = unsafe { FdtManager::get_mut_manager() };
    let fdt_ptr = unsafe { FDT_ADDR as *const u8 };
    match fdt_manager.init(fdt_ptr) {
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

/// Relocates the FDT to safe memory.
/// 
/// This function allocates memory for the FDT and relocates it to that address.
/// 
/// # Panic
/// 
/// This function will panic if the FDT has already been relocated or if
/// the memory allocation fails.
/// 
pub fn relocate_fdt() {
    let fdt_manager = unsafe { FdtManager::get_mut_manager() };
    if fdt_manager.relocated {
        panic!("FDT already relocated");
    }
    let size = fdt_manager.get_fdt().unwrap().total_size();
    let ptr = kmalloc(size);
    if ptr.is_null() {
        panic!("Failed to allocate memory for FDT relocation");
    }
    unsafe { fdt_manager.relocate_fdt(ptr) };
}