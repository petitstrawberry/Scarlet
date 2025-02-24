//! Early console driver for RISC-V64 architecture.
//! 

use super::instruction::sbi::sbi_console_putchar;

pub fn early_putc(c: u8) {
    // Call SBI to print a character.
    sbi_console_putchar(c as char);
}