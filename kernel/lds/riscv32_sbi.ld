OUTPUT_ARCH("riscv")
OUTPUT_FORMAT(elf32-littleriscv)
ENTRY(__SBI_PHYSICAL_ENTRY)
EXTERN(_sbi_start)

KERNEL_VIRTUAL_BASE = 0xb0000000;
KERNEL_PHYSICAL_BASE = 0x80400000;
KERNEL_OFFSET = KERNEL_VIRTUAL_BASE - KERNEL_PHYSICAL_BASE;
__SBI_PHYSICAL_ENTRY = _sbi_start - KERNEL_OFFSET;

PHDRS {
  text PT_LOAD FLAGS(5);
  trampoline PT_LOAD FLAGS(7);
  rodata PT_LOAD FLAGS(4);
  data PT_LOAD FLAGS(6);
}

SECTIONS {
  . = KERNEL_VIRTUAL_BASE;
  __KERNEL_SPACE_START = .;
  .init : AT(ADDR(.init) - KERNEL_OFFSET) {
    KEEP(*(.init.sbi_entry))
    *(.init .init.*)
  } :text
  .text : AT(ADDR(.text) - KERNEL_OFFSET) { *(.text .text.*) } :text

  . = ALIGN(0x1000);
  .trampoline : AT(ADDR(.trampoline) - KERNEL_OFFSET) {
    __TRAMPOLINE_START = .;
    *(.trampoline .trampoline.*)
    . = ALIGN(0x1000);
    __TRAMPOLINE_END = .;
  } :trampoline

  . = ALIGN(0x1000);
  .rodata : AT(ADDR(.rodata) - KERNEL_OFFSET) { *(.rodata .rodata.* .srodata .srodata.*) } :rodata
  . = ALIGN(8);
  .initcall : AT(ADDR(.initcall) - KERNEL_OFFSET) {
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
  .scarlet_ksyms : AT(ADDR(.scarlet_ksyms) - KERNEL_OFFSET) {
    __SCARLET_KSYMS_START = .;
    KEEP(*(.scarlet_ksyms))
    __SCARLET_KSYMS_END = .;
  } :rodata
  .lsm_symbols : AT(ADDR(.lsm_symbols) - KERNEL_OFFSET) { KEEP(*(.lsm_symbols)) } :rodata

  . = ALIGN(0x1000);
  .data : AT(ADDR(.data) - KERNEL_OFFSET) {
    *(.data .data.*)
    PROVIDE(__global_pointer$ = . + 0x800);
    *(.sdata .sdata.*)
  } :data
  .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_OFFSET) {
    __BSS_START = .;
    *(.bss .bss.* .sbss .sbss.* COMMON)
    . = ALIGN(0x1000);
    __BSS_END = .;
  } :data
  .fdt_reserved (NOLOAD) : AT(ADDR(.fdt_reserved) - KERNEL_OFFSET) {
    __FDT_RESERVED_START = .;
    . += 256K;
    __FDT_RESERVED_END = .;
  } :data
  __KERNEL_SPACE_END = .;
  ASSERT(__KERNEL_SPACE_END - __KERNEL_SPACE_START <= 64M, "kernel exceeds SBI bootstrap image window")
  /DISCARD/ : { *(.eh_frame*) *(.note .note.*) *(.interp) }
}
