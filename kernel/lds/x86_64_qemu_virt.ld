OUTPUT_FORMAT(elf64-x86-64)
ENTRY(kmain)

PHDRS
{
    text   PT_LOAD FLAGS(5);  /* R_X */
    rodata PT_LOAD FLAGS(4);  /* R__ */
    data   PT_LOAD FLAGS(6);  /* RW_ */
}

SECTIONS
{
    . = 0xffffffff80000000;
    __KERNEL_SPACE_START = .;

    /* Code segment - executable */
    .text : ALIGN(4K) {
        *(.text .text.*)
        *(.ltext .ltext.*)
    } :text

    . = ALIGN(4K);

    /* Read-only data segment */
    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
        *(.lrodata .lrodata.*)
        
        __INITCALL_EARLY_START = .;
        *(.initcall.early .initcall.early.*)
        __INITCALL_EARLY_END = .;
        
        __INITCALL_DRIVER_START = .;
        *(.initcall.driver .initcall.driver.*)
        __INITCALL_DRIVER_END = .;
        
        __INITCALL_LATE_START = .;
        *(.initcall.late .initcall.late.*)
        __INITCALL_LATE_END = .;
        
        __INITCALL_START = .;
        *(.initcall .initcall.*)
        __INITCALL_END = .;
    } :rodata

    . = ALIGN(4K);

    /* Read-write data segment */
    .data : ALIGN(4K) {
        *(.data .data.*)
        *(.ldata .ldata.*)
        
        KEEP(*(.requests_start_marker))
        KEEP(*(.requests))
        KEEP(*(.requests_end_marker))
    } :data

    .trampoline : ALIGN(4K) {
        __TRAMPOLINE_START = .;
        *(.trampoline .trampoline.*)
        __TRAMPOLINE_END = .;
    } :data

    .bss (NOLOAD) : ALIGN(4K) {
        __BSS_START = .;
        *(.bss .bss.*)
        *(.lbss .lbss.*)
        *(COMMON)
        __BSS_END = .;
    } :data

    __KERNEL_SPACE_END = .;

    /* Metadata sections - not loaded */
    .symtab : { *(.symtab) }
    .strtab : { *(.strtab) }
    .shstrtab : { *(.shstrtab) }

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.comment)
    }
}
