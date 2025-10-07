OUTPUT_ARCH("aarch64")
ENTRY(_entry);

#include "include/scarlet-lds.h"

MEMORY {
  RAM (rwx) : ORIGIN = 0x40000000, LENGTH = 128M
}

/* Define sections using common macros */
SECTIONS {
. = ORIGIN(RAM);
    __KERNEL_SPACE_START = .;
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