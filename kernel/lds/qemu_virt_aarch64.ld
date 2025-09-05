OUTPUT_ARCH("aarch64")
ENTRY(_entry);

MEMORY {
  RAM (rwx) : ORIGIN = 0x40200000, LENGTH = 128M
}

/* Define sections */
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
      __INICALL_LATE_START = .;
      *(.initcall.late)
      __INITCALL_END = .;
    } > RAM
    .trampoline : { *(.trampoline .trampoline.*) } > RAM
    .rodata : { *(.rodata .rodata.*) } > RAM
    .rodata.device_tree : {
        __DEVICE_TREE_START = .;
        *(.rodata.device_tree)
        __DEVICE_TREE_END = .;
    } > RAM
    .data : { *(.data .data.*) } > RAM
    .sdata : { *(.sdata .sdata.*) } > RAM
    .bss : {
        __BSS_START = .;
        *(.bss .bss.*)
        *(COMMON)
        __BSS_END = .;
    } > RAM
    . = ALIGN(16);
    __KERNEL_SPACE_END = .;
    /DISCARD/ : {
        *(.note .note.*)
        *(.eh_frame*)
        *(.debug*)
        *(.gnu*)
    }
}