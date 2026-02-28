OUTPUT_ARCH("x86_64")
ENTRY(_start)

/*
 * Memory layout for x86_64 QEMU:
 * Kernel is loaded by the bootloader at 1MB (0x100000)
 * This avoids the reserved lower 1MB of memory
 */
MEMORY {
  RAM (rwx) : ORIGIN = 0x100000, LENGTH = 128M
}

/* Define sections */
SECTIONS {
. = ORIGIN(RAM);
    __KERNEL_SPACE_START = .;

    /* Kernel entry point */
    .head : {
        KEEP(*(.head.text))
        *(.text.arch_start_kernel .text._entry)
    } > RAM

    /* Init code */
    .init : {
        *(.init .init.*)
        . = ALIGN(16);
    } > RAM

    /* Text section */
    .text : {
        *(.text .text.*)
        . = ALIGN(16);
    } > RAM

    /* Initcall table */
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
      __INITCALL_LATE_END = .;
      __INITCALL_END = .;
      . = ALIGN(16);
    } > RAM

    /* Trampoline code */
    .trampoline : {
      . = ALIGN(4096);
      __TRAMPOLINE_START = .;
      *(.trampoline .trampoline.*)
      . = ALIGN(4096);
      __TRAMPOLINE_END = .;
    } > RAM

    /* Read-only data */
    .rodata : {
        *(.rodata .rodata.*)
        . = ALIGN(16);
    } > RAM

    /* Read-only data after trampoline (string literals, etc.) */
    .rodata AFTER_TRAMPOLINE : {
        *(.rodata.str1.1)
        *(.rodata.str1.4)
        *(.rodata.str1.8)
        . = ALIGN(16);
    } > RAM

    /* Global offset table (for PIC code) */
    .got : {
        *(.got .got.*)
        . = ALIGN(16);
    } > RAM

    /* Data section */
    .data : {
        *(.data .data.*)
        . = ALIGN(16);
    } > RAM

    /* BSS section */
    .bss (NOLOAD) : {
      __BSS_START = .;
      *(.bss .bss.*)
      *(COMMON)
      . = ALIGN(4096);
      __BSS_END = .;
    } > RAM

    /* Reserved space for FDT */
    .fdt_reserved : {
      . = ALIGN(4096);
      __FDT_RESERVED_START = .;
      . += 256K; /* Reserve 256KiB for FDT relocation */
      __FDT_RESERVED_END = .;
    } > RAM

    /* Kernel stack */
    .stack (NOLOAD) : {
      __STACK_START = .;
      . += 64K; /* 64KB kernel stack */
      __STACK_END = .;
    } > RAM

    /* End of kernel space */
    __KERNEL_SPACE_END = .;

    /* Discard unwind tables and other debug info */
    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.comment)
    }
}
