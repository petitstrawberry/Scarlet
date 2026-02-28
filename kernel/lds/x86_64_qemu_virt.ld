OUTPUT_ARCH("x86_64")
ENTRY(_start)

/*
 * Higher-half kernel address space for x86_64.
 * Limine requires the kernel to be loaded in higher half (above 4GB).
 */
KERNEL_VMA = 0xFFFFFFFF80000000;
KERNEL_LMA = 0x100000;

MEMORY {
  RAM (rwx) : ORIGIN = 0x100000, LENGTH = 128M
}

/* Define sections */
SECTIONS {
    . = KERNEL_VMA;
    __KERNEL_SPACE_START = .;

    .head : AT(ADDR(.head) - KERNEL_VMA + KERNEL_LMA) {
        KEEP(*(.head.text))
        *(.text.arch_start_kernel .text._entry)
    }

    .init : AT(ADDR(.init) - KERNEL_VMA + KERNEL_LMA) {
        *(.init .init.*)
        . = ALIGN(16);
    }

    .text : AT(ADDR(.text) - KERNEL_VMA + KERNEL_LMA) {
        *(.text .text.*)
        . = ALIGN(16);
    }

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
