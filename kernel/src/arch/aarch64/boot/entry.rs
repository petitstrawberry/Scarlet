//! AArch64 Boot Entry Points
//!
//! This module provides the low-level entry points for the AArch64 architecture,
//! including assembly stubs for primary and secondary core initialization.

use core::arch::naked_asm;

use crate::{device::fdt::FdtManager, environment::STACK_SIZE, start_kernel};

/// Entry point for the primary core
/// 
/// This function is called by the bootloader/firmware and sets up the initial
/// stack before calling into the main kernel initialization.
///
/// Register usage on entry:
/// - x0: Device Tree Blob (DTB) pointer
/// - x1: Unused (may contain additional boot parameters)
#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry")]
#[unsafe(naked)]
pub extern "C" fn _entry() {
    unsafe {
        naked_asm!("
        // Set up stack for primary core (core 0)
        // Load STACK_SIZE into x2
        mov     x2, {stack_size}
        
        // Load stack base address
        adrp    x3, KERNEL_STACK
        add     x3, x3, :lo12:KERNEL_STACK
        
        // Calculate stack top for core 0: KERNEL_STACK + STACK_SIZE
        add     sp, x3, x2
        
        // Preserve DTB pointer in x0 for arch_start_kernel
        // x0 already contains DTB pointer from bootloader
        
        // Jump to arch_start_kernel
        // x0 = DTB pointer
        // x1 = core ID (0 for primary core)
        mov     x1, #0
        bl      arch_start_kernel
        
        // Should never return, but just in case
        1:
        wfi
        b       1b
        ", 
        stack_size = const STACK_SIZE,
        );
    }
}

/// Entry point for secondary cores
///
/// This function handles initialization of application processor cores.
/// Currently implements a simple wait-for-interrupt loop as secondary
/// core support is not yet implemented.
///
/// Register usage on entry:
/// - x0: Core ID
/// - x1: Device Tree Blob (DTB) pointer  
#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry_ap")]
#[unsafe(naked)]
pub extern "C" fn _entry_ap() {
    unsafe {
        naked_asm!("
        // Get core ID from x0
        mov     x4, x0
        
        // Load STACK_SIZE into x2
        mov     x2, {stack_size}
        
        // Load stack base address
        adrp    x3, KERNEL_STACK
        add     x3, x3, :lo12:KERNEL_STACK
        
        // Calculate stack offset: core_id * STACK_SIZE
        mul     x5, x4, x2
        
        // Calculate stack top: KERNEL_STACK + (core_id * STACK_SIZE) + STACK_SIZE
        add     x5, x5, x2
        add     sp, x3, x5
        
        // For now, secondary cores just wait
        // TODO: Implement proper secondary core initialization
        1:
        wfi
        b       1b
        ",
        stack_size = const STACK_SIZE,
        );
    }
}

/// Architecture-specific kernel start function for AArch64
///
/// This function is called from the assembly entry point after basic
/// setup is complete. It handles DTB registration and calls the main
/// kernel initialization.
///
/// # Arguments
/// * `dtb_ptr` - Pointer to the Device Tree Blob from the bootloader
/// * `core_id` - ID of the current processor core
#[unsafe(no_mangle)]
pub extern "C" fn arch_start_kernel(dtb_ptr: usize, core_id: usize) {
    // Register the Device Tree Blob address for later use
    unsafe { 
        FdtManager::set_fdt_addr(dtb_ptr);
    }
    
    // Call the main kernel initialization
    start_kernel(core_id);
}