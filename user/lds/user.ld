OUTPUT_ARCH("riscv")
ENTRY(_entry);

MEMORY {
  RAM (rwx) : ORIGIN = 0x00, LENGTH = 4096M
}

/* Define sections */
SECTIONS {
    . = ORIGIN(RAM);
    .init : { *(.init .init.*) } > RAM 
    .text : { *(.text .text.*) } > RAM
    .rodata : { *(.rodata .rodata.*) } > RAM 
    .data : { *(.data .data.*) } > RAM 
    .bss (NOLOAD) : {
      __BSS_START = .;
      *(.bss .bss.*)
      . = ALIGN(0x1000);
      __BSS_END = .;
    } > RAM
}