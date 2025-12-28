OUTPUT_ARCH("aarch64")
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

    .init ALIGN(0x1000) : {
        *(.init .init.*)
    } > RAM :text

    .text ALIGN(0x1000) : {
        *(.text .text.*)
    } > RAM :text

    .rodata ALIGN(0x1000) : {
        *(.rodata .rodata.*)
    } > RAM :rodata

    .data ALIGN(0x1000) : {
        *(.data .data.*)
    } > RAM :data

    .bss ALIGN(0x1000) (NOLOAD) : {
        __BSS_START = .;
        *(.bss .bss.*)
        . = ALIGN(0x1000);
        __BSS_END = .;
    } > RAM :data
}
