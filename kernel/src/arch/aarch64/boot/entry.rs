//! AArch64 Boot Entry Points
//!
//! This module provides the low-level entry points for the AArch64 architecture,
//! including assembly stubs for primary and secondary core initialization.
//!
//! The kernel binary includes a Linux arm64 Image header at the beginning,
//! which allows U-Boot to boot it using the `booti` command. This header
//! contains the magic number, image size, and other metadata required by
//! the Linux boot protocol.

use core::{arch::naked_asm, mem::transmute};

use core::arch::asm;

use crate::{
    arch::{
        Aarch64,
        aarch64::{CPUS, trap_init},
    },
    device::fdt::{create_bootinfo_from_fdt, init_fdt, relocate_fdt},
    environment::STACK_SIZE,
    mem::{__FDT_RESERVED_START, init_bss},
    start_kernel,
};

/// QEMU virt machine RAM base address
/// In bare-metal boot mode, QEMU places the DTB at this address
const QEMU_VIRT_RAM_BASE: usize = 0x4000_0000;

/// Check if the given address looks like a valid FDT
/// FDT header magic is 0xd00dfeed (big-endian)
fn looks_like_fdt(ptr: usize) -> bool {
    if ptr == 0 {
        return false;
    }
    // Read magic number and convert from big-endian
    let raw = unsafe { (ptr as *const u32).read_volatile() };
    u32::from_be(raw) == 0xd00dfeed
}

/// Linux arm64 Image header
///
/// This is the entry point of the kernel binary. The header is 64 bytes
/// and conforms to the Linux arm64 boot protocol, allowing bootloaders
/// like U-Boot to use the `booti` command.
///
/// Reference: Linux Documentation/arch/arm64/booting.rst
#[unsafe(link_section = ".head.text")]
#[unsafe(export_name = "_head")]
#[unsafe(naked)]
pub extern "C" fn _head() {
    unsafe {
        naked_asm!(
            // code0: branch to _entry (skip header)
            "b      _entry",
            // code1: reserved (NOP for padding)
            ".word  0",
            // text_offset: kernel load offset from RAM base (2MB)
            ".quad  0x200000",
            // image_size: filled in by linker (use max for now)
            ".quad  0x2000000",
            // flags: little endian, 4K pages
            ".quad  0",
            // res2, res3, res4: reserved
            ".quad  0",
            ".quad  0",
            ".quad  0",
            // magic: ARM\x64 (0x644d5241 in little endian)
            ".ascii \"ARM\\x64\"",
            // res5: reserved (PE/COFF offset, not used)
            ".word  0",
        );
    }
}

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
        
        // Preserve x0 (DTB pointer) in a callee-saved register
        mov     x19, x0
        
        // Disable MMU, D-cache, and I-cache to ensure we run in identity-mapped mode
        // This is required because U-Boot may have left MMU enabled
        // Also disable alignment check (A bit) for unaligned SIMD access
        mrs     x1, sctlr_el1
        bic     x1, x1, #(1 << 0)       // Clear M bit (MMU enable)
        bic     x1, x1, #(1 << 1)       // Clear A bit (Alignment check)
        bic     x1, x1, #(1 << 2)       // Clear C bit (D-cache enable)  
        bic     x1, x1, #(1 << 12)      // Clear I bit (I-cache enable)
        msr     sctlr_el1, x1
        isb
        
        // Enable FP/SIMD access (required for Rust code which may use SIMD)
        // CPACR_EL1.FPEN (bits 21:20) = 0b11 enables FP/SIMD at EL0 and EL1
        mov     x1, #(3 << 20)
        msr     cpacr_el1, x1
        isb
        
        // Use SP_EL1 instead of SP_EL0 for EL1 stack operations
        // SPSel = 1 means use SP_ELx for EL x
        mov     x1, #1
        msr     spsel, x1
        isb
        
        // Invalidate TLB
        tlbi    vmalle1
        dsb     sy
        isb
        
        // Restore DTB pointer
        mov     x0, x19
        
        // Get CPU core ID from MPIDR_EL1 register
        mrs     x4, MPIDR_EL1
        and     x4, x4, #0xFF           // Extract Aff0 (core ID within cluster)
        
        // Set up stack for this core
        // Load STACK_SIZE into x2
        mov     x2, {stack_size}
        
        // Load stack base address
        adrp    x3, KERNEL_STACK
        add     x3, x3, :lo12:KERNEL_STACK
        
        // Calculate stack top: KERNEL_STACK + ((core_id + 1) * STACK_SIZE)
        add     x5, x4, #1              // core_id + 1
        mul     x5, x5, x2              // (core_id + 1) * STACK_SIZE
        add     x5, x3, x5              // Stack top address
        
        // Ensure 16-byte alignment for AArch64 ABI (required for SIMD)
        and     sp, x5, #~0xF           // Align SP to 16-byte boundary
        
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
    // Initialize .bss section first - required before using any static variables
    init_bss();

    crate::early_println!(
        "[aarch64] Core {}: Starting kernel with DTB at {:#x}",
        core_id,
        dtb_ptr
    );

    // Validate DTB pointer according to boot protocol
    if dtb_ptr == 0 {
        panic!("[aarch64] Invalid DTB pointer: null address violates boot protocol");
    }

    if !looks_like_fdt(dtb_ptr) {
        panic!("[aarch64] Invalid DTB at {:#x}: bad magic number", dtb_ptr);
    }

    if dtb_ptr & 0x7 != 0 {
        crate::early_println!(
            "[aarch64] Warning: DTB pointer {:#x} not 8-byte aligned",
            dtb_ptr
        );
    }

    // Initialize FDT - this will validate the DTB magic number
    init_fdt(dtb_ptr);

    // Relocate FDT to safe memory
    let fdt_reloc_start = unsafe { &__FDT_RESERVED_START as *const usize as usize };
    let dest_ptr = fdt_reloc_start as *mut u8;
    let relocated_fdt_area = relocate_fdt(dest_ptr);

    // Create BootInfo with relocated FDT address
    let bootinfo = create_bootinfo_from_fdt(core_id, relocated_fdt_area.start);

    crate::early_println!(
        "[aarch64] Core {}: Initializing architecture support...",
        core_id
    );

    // Get raw Aarch64 struct
    let aarch64: &mut Aarch64 = unsafe { transmute(&CPUS[core_id] as *const _ as usize) };
    trap_init(aarch64);

    start_kernel(&bootinfo);
}
