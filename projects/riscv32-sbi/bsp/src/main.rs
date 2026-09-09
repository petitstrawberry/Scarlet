#![no_std]
#![no_main]

use scarlet_modules::{force_link, scarlet};

// Retain the generated module dependencies through the normal initcall table.
// SBI enters through architecture assembly, rather than a BSP Rust entry shim.
scarlet::early_initcall!(force_link);

// Keep the architecture entry in the final image. The linker sets the ELF
// entry to its physical load alias; Rust execution uses the linked alias.
#[used]
static ENTRY: unsafe extern "C" fn() -> ! = scarlet::arch::riscv::boot::sbi::_sbi_start;
