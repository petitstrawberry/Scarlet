ENTRY(reset_handler)

MEMORY
{
  FLASH : ORIGIN = 0x00000000, LENGTH = 64K
  RAM   : ORIGIN = 0x10000000, LENGTH = 48K
}

__stack_top = ORIGIN(RAM) + LENGTH(RAM);

SECTIONS
{
  .vector_table ORIGIN(FLASH) :
  {
    KEEP(*(.vector_table .vector_table.*));
  } > FLASH

  .text :
  {
    *(.text .text.*);
    *(.rodata .rodata.*);
  } > FLASH

  .data :
  {
    *(.data .data.*);
  } > RAM AT > FLASH

  .bss (NOLOAD) :
  {
    *(.bss .bss.*);
    *(COMMON);
  } > RAM

  /DISCARD/ :
  {
    *(.ARM.exidx .ARM.exidx.*);
  }
}
