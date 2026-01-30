//! Early console driver for RISC-V64 architecture.
//!

use crate::arch::instruction::sbi::sbi_debug_console_write_byte;

pub fn early_putc(c: u8) {
    // Call SBI to print a character.
    sbi_debug_console_write_byte(c as char);
}
