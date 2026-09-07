MEMORY_OFFSET = 0x84000000;

SECTIONS
{
    . = MEMORY_OFFSET;
    
    .text : {
        *(.text.start)
        *(.text*)
    }
    
    .rodata : {
        *(.rodata*)
    }
    
    .data : {
        *(.data*)
    }
    
    .bss : {
        *(.bss*)
    }
    
    . = ALIGN(16);
    . += 0x10000;
    _stack_top = .;
}
