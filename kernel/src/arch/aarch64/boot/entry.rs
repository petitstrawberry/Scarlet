//! AArch64 Boot Entry Points
//!
//! This module provides the low-level entry points for the AArch64 architecture,
//! including assembly stubs for primary and secondary core initialization.

use core::{arch::naked_asm, mem::transmute};

use core::arch::asm;

use crate::{
    arch::{Aarch64, aarch64::{TRAPFRAME, trap_init}}, 
    device::fdt::{init_fdt, relocate_fdt, create_bootinfo_from_fdt}, 
    environment::STACK_SIZE, 
    mem::{__FDT_RESERVED_START, init_bss}, 
    start_kernel
};

/// Entry point for the primary core
/// 
/// This function is called by the bootloader/firmware following the Linux 
/// AArch64 boot protocol. The register state on entry must be:
///
/// Register usage on entry (Linux AArch64 boot protocol):
/// - x0: Device Tree Blob (DTB) physical address - MANDATORY
/// - x1: 0 (reserved for future use)
/// - x2: 0 (reserved for future use)  
/// - x3: 0 (reserved for future use)
/// - pc: kernel image entry point
/// - EL: EL2 (Hypervisor) or EL1 (Kernel) depending on bootloader
///
/// CPU ID is obtained from MPIDR_EL1 register, not from boot parameters.
/// MPIDR_EL1.Aff0 (bits 7:0) contains the core ID within the cluster.
///
/// The DTB in x0 contains hardware configuration including:
/// - Memory layout and size
/// - CPU core count and topology
/// - Peripheral device addresses and IRQ mappings
/// - Clock frequencies and other hardware parameters
#[unsafe(link_section = ".init")]
#[unsafe(export_name = "_entry")]
#[unsafe(naked)]
pub extern "C" fn _entry() {
    unsafe {
        unsafe {
        naked_asm!("
        // Linux AArch64 boot protocol:
        // x0 = DTB physical address (MANDATORY - contains hardware config)
        // x1 = 0 (reserved)
        // x2 = 0 (reserved) 
        // x3 = 0 (reserved)
        // EL = EL1 or EL2 (depending on bootloader configuration)
        
        // Get CPU core ID from MPIDR_EL1 register
        mrs     x4, MPIDR_EL1
        and     x4, x4, #0xFF           // Extract Aff0 (core ID within cluster)
        
        // Validate that we have a DTB pointer in x0
        // A null pointer would be invalid according to the boot protocol
        // cbz     x0, 2f
        
        // Set up stack for this core
        // Load STACK_SIZE into x2
        mov     x2, {stack_size}
        
        // Load stack base address
        adrp    x3, KERNEL_STACK
        add     x3, x3, :lo12:KERNEL_STACK
        
        // Calculate stack top: KERNEL_STACK + ((core_id + 1) * STACK_SIZE)
        add     x5, x4, #1              // core_id + 1
        mul     x5, x5, x2              // (core_id + 1) * STACK_SIZE
        add     sp, x3, x5              // Final stack pointer
        
        // Preserve registers for arch_start_kernel
        mov     x1, x0                  // DTB pointer (x0 -> x1)
        mov     x0, x4                  // Core ID (from MPIDR_EL1)
        
        // Jump to arch_start_kernel
        // x0 = core ID, x1 = DTB pointer
        bl      arch_start_kernel
        
        // Should never return, but just in case
        1:
        wfi
        b       1b
        
        // Error handling for invalid DTB (label 2)
        2:
        // If DTB is null, we can't proceed - enter infinite loop
        // In a real implementation, this might try to use a fallback DTB
        // or signal an error to the bootloader
        wfi
        b       2b
        ", 
        stack_size = const STACK_SIZE,
        );
    }
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
        // Get core ID from MPIDR_EL1 register for secondary cores too
        mrs     x4, MPIDR_EL1
        and     x4, x4, #0xFF           // Extract Aff0 (core ID within cluster)
        
        // Load STACK_SIZE into x2
        mov     x2, {stack_size}
        
        // Load stack base address
        adrp    x3, KERNEL_STACK
        add     x3, x3, :lo12:KERNEL_STACK
        
        // Calculate stack top: KERNEL_STACK + ((core_id + 1) * STACK_SIZE)
        add     x5, x4, #1              // core_id + 1
        mul     x5, x5, x2              // (core_id + 1) * STACK_SIZE
        add     sp, x3, x5              // Final stack pointer
        
        // Pass core ID to start_ap
        mov     x0, x4                  // Core ID from MPIDR_EL1
        
        // For now, secondary cores just wait
        // TODO: Implement proper secondary core initialization  
        bl      start_ap
        
        // Should never return, but just in case
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
/// setup is complete. It validates the boot protocol compliance and
/// handles DTB registration before calling main kernel initialization.
///
/// # Arguments
/// * `core_id` - ID of the current processor core (0 for primary core)
/// * `dtb_ptr` - Physical address of Device Tree Blob from bootloader
///
/// # Boot Protocol Validation
/// According to Linux AArch64 boot protocol, the DTB pointer must be:
/// - Non-zero (valid physical address)
/// - Aligned to 8-byte boundary  
/// - Point to valid DTB magic number (0xd00dfeed)
#[unsafe(no_mangle)]
pub extern "C" fn arch_start_kernel(core_id: usize, dtb_ptr: usize) {
    crate::early_println!("[aarch64] Core {}: Starting kernel with DTB at {:#x}", core_id, dtb_ptr);
    
    // Check current Exception Level for boot protocol compliance
    let current_el: u64;
    unsafe {
        asm!("mrs {0}, CurrentEL", out(reg) current_el);
    }
    let el = (current_el >> 2) & 0x3;
    crate::early_println!("[aarch64] Core {}: Running at Exception Level {}", core_id, el);
    
    // Validate DTB pointer according to boot protocol
    if dtb_ptr == 0 {
        panic!("[aarch64] Invalid DTB pointer: null address violates boot protocol");
    }
    
    if dtb_ptr & 0x7 != 0 {
        crate::early_println!("[aarch64] Warning: DTB pointer {:#x} not 8-byte aligned", dtb_ptr);
    }
    
    // Initialize .bss section
    init_bss();
    
    // Initialize FDT - this will validate the DTB magic number
    init_fdt(dtb_ptr);
    
    // Relocate FDT to safe memory
    let fdt_reloc_start = unsafe { &__FDT_RESERVED_START as *const usize as usize };
    let dest_ptr = fdt_reloc_start as *mut u8;
    let relocated_fdt_area = relocate_fdt(dest_ptr);
    
    // Create BootInfo with relocated FDT address
    let bootinfo = create_bootinfo_from_fdt(core_id, relocated_fdt_area.start);

    crate::early_println!("[aarch64] Core {}: Boot protocol validation passed", core_id);
    crate::early_println!("[aarch64] Core {}: Initializing architecture support...", core_id);
    
    // Get raw Aarch64 struct
    let aarch64: &mut Aarch64 = unsafe { transmute(&TRAPFRAME[core_id] as *const _ as usize) };
    trap_init(aarch64);

    start_kernel(&bootinfo);
}
