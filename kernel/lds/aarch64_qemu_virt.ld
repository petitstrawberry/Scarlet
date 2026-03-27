OUTPUT_ARCH("aarch64")
ENTRY(_head);

#include "include/scarlet-lds.h"

/*
 * Memory layout for QEMU virt machine:
 * 0x40000000 - 0x401FFFFF: Reserved for DTB (2MB)
 * 0x40200000 - onwards:    Kernel
 *
 * U-Boot loads kernel at 0x40200000 and passes DTB address (0x40000000) in x0
 */
MEMORY {
  RAM (rwx) : ORIGIN = 0x40200000, LENGTH = 126M
}

/* Define sections using common macros */
SECTIONS {
. = ORIGIN(RAM);
    __KERNEL_SPACE_START = .;
    PROVIDE(__SCARLET_KSYMS_START = 0);
    PROVIDE(__SCARLET_KSYMS_END = 0);
    
    /* Linux arm64 Image header - required for U-Boot booti command */
    .head : {
        KEEP(*(.head.text))
    } > RAM
    
    .init : { 
        *(.init .init.*)
        /* Place arch-specific entry points close to init */
        *(.text.arch_start_kernel .text._entry)
    } > RAM 
    .text : { *(.text .text.*) } > RAM
    .initcall : { 
      __INITCALL_START = .;
      __INITCALL_EARLY_START = .;
      *(.initcall.early)
      __INITCALL_EARLY_END = .;
      __INITCALL_DRIVER_START = .;
      *(.initcall.driver)
      __INITCALL_DRIVER_END = .;
      __INITCALL_LATE_START = .;
      *(.initcall.late)
      __INITCALL_END = .;
    } > RAM
    .scarlet_ksyms : {
      __SCARLET_KSYMS_START = .;
      KEEP(*(.scarlet_ksyms))
      __SCARLET_KSYMS_END = .;
    } > RAM
    .trampoline :
    {
      . = ALIGN(0x1000);
      __TRAMPOLINE_START = .;
      *(.trampoline .trampoline.*)
      . = ALIGN(0x1000);
      __TRAMPOLINE_END = .;
    } > RAM
    .rodata : { *(.rodata .rodata.*) } > RAM 
    .data : { *(.data .data.*) } > RAM 
    .bss (NOLOAD) : {
      __BSS_START = .;
      *(.bss .bss.*)
      . = ALIGN(0x1000);
      __BSS_END = .;
    } > RAM
    .fdt_reserved : {
      . = ALIGN(0x1000);
      __FDT_RESERVED_START = .;
      . += 256K; /* Reserve 256KiB for FDT relocation */
      __FDT_RESERVED_END = .;
    } > RAM
    __KERNEL_SPACE_END = .;
}