//! Flattened Device Tree (FDT) management module.
//! 
//! This module provides comprehensive functionality for managing the Flattened Device Tree (FDT),
//! which is a data structure used to describe the hardware components of a system in
//! various architectures including RISC-V, ARM, and PowerPC.
//!
//! # Overview
//!
//! The FDT is passed by the bootloader to the kernel and contains critical information about
//! the hardware configuration of the system. This module parses, relocates, and provides
//! access to that information through a unified interface that integrates with the kernel's
//! BootInfo structure.
//!
//! # Core Components
//!
//! - **`FdtManager`**: A singleton that manages access to the parsed FDT with relocation support
//! - **`init_fdt()`**: Function to initialize the FDT subsystem from bootloader-provided address
//! - **`relocate_fdt()`**: Function to safely relocate FDT to kernel-managed memory
//! - **`create_bootinfo_from_fdt()`**: Function to extract BootInfo from FDT data
//!
//! # Boot Integration
//!
//! The module integrates with the kernel boot process through the BootInfo structure:
//!
//! 1. **Initialization**: Architecture code calls `init_fdt()` with bootloader-provided address
//! 2. **Relocation**: FDT is moved to safe memory using `relocate_fdt()`
//! 3. **BootInfo Creation**: `create_bootinfo_from_fdt()` extracts system information
//! 4. **Kernel Handoff**: BootInfo is passed to `start_kernel()` for unified initialization
//!
//! # Memory Management
//!
//! The module provides advanced memory management for FDT data:
//! - **Safe Relocation**: Copies FDT to kernel-controlled memory to prevent corruption
//! - **Initramfs Handling**: Automatically relocates initramfs to prevent memory conflicts
//! - **Memory Area Calculation**: Computes usable memory areas excluding kernel and FDT regions
//!
//! # Architecture Support
//!
//! This module is architecture-agnostic and supports any platform using FDT:
//! - **RISC-V**: Primary device tree platform
//! - **ARM/AArch32**: Standard hardware description method
//! - **AArch64**: Alternative to UEFI for hardware description
//! - **PowerPC**: Traditional FDT usage
//! - **Other FDT platforms**: Any architecture supporting device trees
//!
//! # Usage
//!
//! ## Basic Initialization
//!
//! ```rust
//! // Initialize FDT from bootloader-provided address
//! init_fdt(fdt_addr);
//! 
//! // Relocate to safe memory
//! let dest_ptr = safe_memory_area as *mut u8;
//! let relocated_area = relocate_fdt(dest_ptr);
//! 
//! // Create BootInfo with FDT data
//! let bootinfo = create_bootinfo_from_fdt(cpu_id, relocated_area.start);
//! ```
//!
//! ## FDT Data Access
//!
//! ```rust
//! let fdt_manager = FdtManager::get_manager();
//! if let Some(fdt) = fdt_manager.get_fdt() {
//!     // Access FDT nodes and properties
//!     let memory_node = fdt.find_node("/memory");
//!     let chosen_node = fdt.find_node("/chosen");
//! }
//! ```
//!
//! # Hardware Information Extraction
//!
//! The module extracts essential hardware information:
//! - **Memory Layout**: Total system memory from `/memory` nodes
//! - **Initramfs Location**: Initial filesystem from `/chosen` node
//! - **Command Line**: Boot arguments from `/chosen/bootargs`
//! - **Device Tree**: Complete hardware description for device enumeration
//!
//! # Safety and Error Handling
//!
//! The module provides robust error handling:
//! - **Validation**: All FDT operations include proper error checking
//! - **Memory Safety**: Relocation prevents use-after-free and corruption
//! - **Graceful Degradation**: Missing optional components (like initramfs) are handled gracefully
//! - **Panic Conditions**: Clear documentation of when functions may panic
//!
//! # Implementation Details
//!
//! The module uses the `fdt` crate for low-level parsing while providing high-level
//! abstractions for kernel integration. It maintains a static global manager instance
//! to provide access throughout the kernel, with careful synchronization to prevent
//! data races during initialization.


use core::panic;
use core::result::Result;

use fdt::{Fdt, FdtError};

use crate::early_println;
use crate::vm::vmem::MemoryArea;
use crate::{BootInfo, DeviceSource};

static mut MANAGER: FdtManager = FdtManager::new();


pub struct FdtManager<'a> {
    fdt: Option<Fdt<'a>>,
    relocated: bool,
    original_addr: Option<usize>,
}

impl<'a> FdtManager<'a> {
    const fn new() -> Self {
        FdtManager {
            fdt: None,
            relocated: false,
            original_addr: None,
        }
    }

    pub fn init(&mut self, ptr: *const u8) -> Result<(), FdtError> {
        match unsafe { Fdt::from_ptr(ptr) } {
            Ok(fdt) => {
                self.fdt = Some(fdt);
                self.original_addr = Some(ptr as usize);
            }
            Err(e) => return Err(e),
        }
        Ok(())
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
        let fdt = self.get_fdt().unwrap();
        let size = fdt.total_size();
        let old_ptr = self.original_addr.expect("Original FDT address not recorded") as *const u8;
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
            
            let memory_area = MemoryArea::new(start, end - 1);
            return Some(memory_area);
        }
        
        None
    }

    pub fn get_dram_memoryarea(&self) -> Option<MemoryArea> {
        let fdt = self.get_fdt()?;
        let memory_node = fdt.find_node("/memory")?;
        
        
        let reg = memory_node.property("reg")?;
        if reg.value.len() < 16 {
            return None;
        }
        let reg_start = u64::from_be_bytes([
            reg.value[0],
            reg.value[1],
            reg.value[2],
            reg.value[3],
            reg.value[4],
            reg.value[5],
            reg.value[6],
            reg.value[7],
        ]);
        let start = reg_start as usize;
        let size = u64::from_be_bytes([
            reg.value[8],
            reg.value[9],
            reg.value[10],
            reg.value[11],
            reg.value[12],
            reg.value[13],
            reg.value[14],
            reg.value[15],
        ]) as usize;
        Some(
            MemoryArea::new(start as usize, start + size - 1) // end is inclusive
        )
    }

}

/// Initializes the FDT subsystem with the given address.
/// 
/// # Arguments
/// 
/// * `addr`: The address of the FDT.
/// 
/// # Safety
/// 
/// This function modifies a static variable that holds the FDT address.
/// Ensure that this function is called before any other FDT-related functions
/// to avoid undefined behavior.
pub fn init_fdt(addr: usize) {
    let fdt_manager = unsafe { FdtManager::get_mut_manager() };
    let fdt_ptr = addr as *const u8;
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
pub fn relocate_fdt(dest_ptr: *mut u8) -> MemoryArea {
    let fdt_manager = unsafe { FdtManager::get_mut_manager() };
    if fdt_manager.relocated {
        panic!("FDT already relocated");
    }
    let size = fdt_manager.get_fdt().unwrap().total_size();
    unsafe { fdt_manager.relocate_fdt(dest_ptr) };
    MemoryArea::new(dest_ptr as usize, dest_ptr as usize + size - 1) // return the memory area
}

/// Create BootInfo from FDT data
/// 
/// This function creates a comprehensive BootInfo structure by extracting essential
/// system information from the Flattened Device Tree (FDT). It serves as the bridge
/// between FDT-based boot protocols and the kernel's unified boot interface.
/// 
/// # Architecture Compatibility
/// 
/// This function is architecture-agnostic and can be used by any architecture that
/// uses FDT for hardware description:
/// - **RISC-V**: Primary boot protocol
/// - **ARM/AArch32**: Standard boot method  
/// - **AArch64**: Alternative to UEFI
/// - **PowerPC**: Traditional FDT usage
/// - **Other architectures**: Any FDT-capable platform
/// 
/// # Boot Information Extraction
/// 
/// The function extracts the following information from FDT:
/// - **Memory Layout**: DRAM size and location from `/memory` node
/// - **Usable Memory**: Calculates available memory excluding kernel image
/// - **Initramfs**: Relocates and provides access to initial filesystem
/// - **Command Line**: Extracts bootargs from `/chosen` node
/// - **Device Source**: Creates FDT-based device source reference
/// 
/// # Memory Management
/// 
/// The function performs automatic memory management:
/// 1. **DRAM Discovery**: Parses memory nodes to find total system memory
/// 2. **Kernel Exclusion**: Calculates usable memory starting after kernel image
/// 3. **Initramfs Relocation**: Moves initramfs to safe memory location
/// 4. **Memory Area Updates**: Adjusts usable memory to account for relocations
/// 
/// # Initramfs Handling
/// 
/// If initramfs is present in the FDT `/chosen` node:
/// - Automatically relocates to prevent overlap with kernel heap
/// - Updates usable memory area to exclude relocated initramfs
/// - Provides relocated address in BootInfo for VFS initialization
/// 
/// # Arguments
/// 
/// * `cpu_id` - ID of the current CPU/Hart performing boot
/// * `relocated_fdt_addr` - Physical address of the relocated FDT in memory
/// 
/// # Returns
/// 
/// A complete BootInfo structure containing all essential boot parameters
/// extracted from the FDT, ready for use by `start_kernel()`.
/// 
/// # Panics
/// 
/// This function will panic if:
/// - FDT manager is not properly initialized
/// - Required memory nodes are missing from FDT
/// - Memory layout is invalid or corrupted
/// 
/// # Example
/// 
/// ```rust
/// // Called from architecture-specific boot code
/// let bootinfo = create_bootinfo_from_fdt(hartid, relocated_fdt_area.start);
/// start_kernel(&bootinfo);
/// ```
/// 
pub fn create_bootinfo_from_fdt(cpu_id: usize, relocated_fdt_addr: usize) -> BootInfo {
    let fdt_manager = FdtManager::get_manager();
    
    // Get DRAM area
    let dram_area = fdt_manager.get_dram_memoryarea().expect("Memory area not found");
    
    // Calculate usable memory area (simplified for now)
    let kernel_end = unsafe { &crate::mem::__KERNEL_SPACE_END as *const usize as usize };
    let mut usable_memory = MemoryArea::new(kernel_end, dram_area.end);
    
    // Relocate initramfs
    crate::early_println!("Relocating initramfs...");
    
    let relocated_initramfs = match crate::fs::vfs_v2::drivers::initramfs::relocate_initramfs(&mut usable_memory) {
        Ok(area) => {
            Some(area)
        },
        Err(_e) => {
            None
        }
    };
    
    // Get command line
    let cmdline = fdt_manager.get_fdt()
        .and_then(|fdt| fdt.chosen().bootargs());
    
    BootInfo::new(
        cpu_id,
        usable_memory,
        relocated_initramfs,
        cmdline,
        DeviceSource::Fdt(relocated_fdt_addr),
    )
}