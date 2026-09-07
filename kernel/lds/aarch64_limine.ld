OUTPUT_ARCH("aarch64")
ENTRY(arch_start_kernel);

PHDRS {
  limine_requests PT_LOAD FLAGS(4);
  text PT_LOAD FLAGS(5);
  trampoline PT_LOAD FLAGS(7);
  rodata PT_LOAD FLAGS(4);
  data PT_LOAD FLAGS(6);
}

SECTIONS {
    . = 0xffffffff80000000;
    __KERNEL_SPACE_START = .;

    PROVIDE(__SCARLET_KSYMS_START = 0);
    PROVIDE(__SCARLET_KSYMS_END = 0);

    .limine_requests : {
        KEEP(*(.limine_requests_start))
        KEEP(*(.limine_requests))
        KEEP(*(.limine_requests_end))
    } :limine_requests

    . = ALIGN(CONSTANT(MAXPAGESIZE));
    .init : {
        *(.init .init.*)
    } :text

    .text : {
        *(.text .text.*)
    } :text

    . = ALIGN(0x1000);
    .trampoline : {
        __TRAMPOLINE_START = .;
        *(.trampoline .trampoline.*)
        . = ALIGN(0x1000);
        __TRAMPOLINE_END = .;
    } :trampoline

    . = ALIGN(CONSTANT(MAXPAGESIZE));
    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(8);
    .initcall : {
      __INITCALL_START = .;
      __INITCALL_EARLY_START = .;
      *(.initcall.early)
      __INITCALL_EARLY_END = .;
      __INITCALL_DRIVER_START = .;
      *(.initcall.driver)
      __INITCALL_DRIVER_END = .;
      __INITCALL_LATE_START = .;
      *(.initcall.late)
      __INITCALL_END = .;
    } :rodata

    . = ALIGN(8);
    .scarlet_ksyms : {
      __SCARLET_KSYMS_START = .;
      KEEP(*(.scarlet_ksyms))
      __SCARLET_KSYMS_END = .;
    } :rodata

    . = ALIGN(CONSTANT(MAXPAGESIZE));
    .data : {
        *(.data .data.*)
    } :data

    .bss (NOLOAD) : {
      __BSS_START = .;
      *(.bss .bss.*)
      . = ALIGN(0x1000);
      __BSS_END = .;
    } :data

    .fdt_reserved (NOLOAD) : {
      . = ALIGN(0x1000);
      __FDT_RESERVED_START = .;
      . += 256K;
      __FDT_RESERVED_END = .;
    } :data

    __KERNEL_SPACE_END = .;

    .lsm_symbols : {
      KEEP(*(.lsm_symbols))
    }

    /DISCARD/ : {
      *(.eh_frame*)
      *(.note .note.*)
      *(.interp)
    }
}
