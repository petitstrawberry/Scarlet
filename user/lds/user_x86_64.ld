OUTPUT_ARCH("x86_64")
ENTRY(_entry);

MEMORY {
    RAM (rwx) : ORIGIN = 0x0000000000010000, LENGTH = 4096M
}

PHDRS {
  text PT_LOAD FLAGS(5);    /* R + X */
  rodata PT_LOAD FLAGS(4);  /* R     */
  data PT_LOAD FLAGS(6);    /* R + W */
}

SECTIONS {
    . = ORIGIN(RAM);

    .init ALIGN(4096) : {
        *(.init .init.*)
    } > RAM :text

    .text ALIGN(4096) : {
        *(.text .text.*)
    } > RAM :text

    .rodata ALIGN(4096) : {
        *(.rodata .rodata.*)
    } > RAM :rodata

    .data ALIGN(4096) : {
        *(.data .data.*)
    } > RAM :data

    .bss ALIGN(4096) (NOLOAD) : {
        __BSS_START = .;
        *(.bss .bss.*)
        . = ALIGN(4096);
        __BSS_END = .;
    } > RAM :data
}
