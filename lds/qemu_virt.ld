OUTPUT_ARCH("riscv")
ENTRY(_entry);

MEMORY {
  RAM (rwx) : ORIGIN = 0x84000000, LENGTH = 16M
}

/* Define sections */
SECTIONS {
    __STACK_SHIFT = 14; /* 0x4000 per core */
    __STACK_SIZE = (1 << __STACK_SHIFT); /* Size of stack per core */
    __HEAP_SIZE = 0x800000; /* 8MB heap size */

    . = ORIGIN(RAM);
    .init : { *(.init) } > RAM 
    .text : { *(.text .text.*) } > RAM 
    .rodata : { *(.rodata) } > RAM 
    .data : { *(.data) } > RAM 
    .bss : { *(.bss) } > RAM
    .heap :
    {
      *(.heap)       
      . = ALIGN(0x1000);
      __HEAP_START = .;
      . += __HEAP_SIZE;
      __HEAP_END = .;
    } > RAM
    .stack :
    {
        *(.stack .stack.*)
        . = ALIGN(0x1000);
        __KERNEL_STACK_TOP = .;
        . += __STACK_SIZE*4;
        __KERNEL_STACK_BOTTOM = .;
        . = ALIGN(0x1000);
        __KERNEL_TRAP_STACK_TOP = .;
        . += __STACK_SIZE*4;
        __KERNEL_TRAP_STACK_BOTTOM = .;
    } > RAM
    . = ALIGN(4);
}