OUTPUT_ARCH("riscv")
ENTRY(_entry);

MEMORY {
  RAM (rwx) : ORIGIN = 0x00, LENGTH = 4096M
}

/* Define sections */
SECTIONS {
    . = ORIGIN(RAM);
    .init : { *(.init .init.*) } > RAM 
    . = ALIGN(0x1000);
    .text : { *(.text .text.*) } > RAM
    . = ALIGN(0x1000);
    .rodata : { *(.rodata .rodata.*) } > RAM 
    . = ALIGN(0x1000);
    .data : { *(.data .data.*) } > RAM 
    .bss (NOLOAD) : {
      . = ALIGN(0x1000);
      __BSS_START = .;
      *(.bss .bss.*)
      . = ALIGN(0x1000);
      __BSS_END = .;
    } > RAM
}