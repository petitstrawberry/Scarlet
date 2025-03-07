OUTPUT_ARCH("riscv")
ENTRY(_entry);

MEMORY {
  RAM (rwx) : ORIGIN = 0x84000000, LENGTH = 16M
}

/* Define sections */
SECTIONS {
    __HEAP_SIZE = 0x800000; /* 8MB heap size */

    . = ORIGIN(RAM);
    __KERNEL_SPACE_START = .;
    .init : { *(.init .init.*) } > RAM 
    .text : { *(.text .text.*) } > RAM
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
    .bss (NOLOAD) : { *(.bss .bss.*) } > RAM
    .heap (NOLOAD) :
    {
      *(.heap .heap.*)       
      . = ALIGN(0x1000);
      __HEAP_START = .;
      . += __HEAP_SIZE;
      __HEAP_END = .;
    } > RAM
    . = ALIGN(0x1000);
    __KERNEL_SPACE_END = .;
}