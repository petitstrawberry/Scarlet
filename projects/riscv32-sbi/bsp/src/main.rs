#![no_std]
#![no_main]

// Keep the architecture entry in the final image. The linker sets the ELF
// entry to its physical load alias; Rust execution uses the linked alias.
#[used]
static ENTRY: unsafe extern "C" fn() -> ! = scarlet::arch::riscv::boot::sbi::_sbi_start;
