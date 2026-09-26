OUTPUT_ARCH("aarch64")
ENTRY(_head)

PHDRS {
    text PT_LOAD FLAGS(5);
    trampoline PT_LOAD FLAGS(7);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS {
    . = 0x40200000;
    __KERNEL_SPACE_START = .;
    .head : {
        KEEP(*(.head.text.header))
        *(.head.text.entry .head.text.entry.*)
    } :text
    .init : { *(.init .init.*) } :text
    .text : { *(.text .text.*) } :text
    . = ALIGN(0x1000);
    .trampoline : {
        __TRAMPOLINE_START = .;
        *(.trampoline .trampoline.*)
        . = ALIGN(0x1000);
        __TRAMPOLINE_END = .;
    } :trampoline
    . = ALIGN(0x1000);
    .rodata : { *(.rodata .rodata.*) } :rodata
    . = ALIGN(8);
    .initcall : {
        __INITCALL_START = .;
        __INITCALL_EARLY_START = .;
        KEEP(*(.initcall.early))
        __INITCALL_EARLY_END = .;
        __INITCALL_DRIVER_START = .;
        KEEP(*(.initcall.driver))
        __INITCALL_DRIVER_END = .;
        __INITCALL_LATE_START = .;
        KEEP(*(.initcall.late))
        __INITCALL_END = .;
    } :rodata
    .scarlet_ksyms : {
        __SCARLET_KSYMS_START = .;
        KEEP(*(.scarlet_ksyms))
        __SCARLET_KSYMS_END = .;
    } :rodata
    .lsm_symbols : { KEEP(*(.lsm_symbols)) } :rodata
    . = ALIGN(0x1000);
    .data : { *(.data .data.*) } :data
    .bss (NOLOAD) : {
        __BSS_START = .;
        *(.bss .bss.* COMMON)
        . = ALIGN(0x1000);
        __BSS_END = .;
    } :data
    .fdt_reserved (NOLOAD) : {
        __FDT_RESERVED_START = .;
        . += 2M;
        __FDT_RESERVED_END = .;
    } :data
    __KERNEL_SPACE_END = .;
    __KERNEL_IMAGE_SIZE = __KERNEL_SPACE_END - __KERNEL_SPACE_START;
    ASSERT(_head == 0x40200000, "Image header must be first")
    ASSERT(__KERNEL_SPACE_END < 0x48000000, "Kernel overlaps the QEMU initramfs placement")
    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.interp)
    }
}
