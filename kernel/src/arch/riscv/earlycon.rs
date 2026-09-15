//! Early console driver for RISC-V64 architecture.
//!

use crate::arch::instruction::sbi::sbi_debug_console_write_byte;

/// SBI debug output is firmware-backed rather than a registered UART device.
pub(crate) const fn has_active_uart() -> bool {
    false
}

pub fn early_putc(c: u8) {
    // Call SBI to print a character.
    sbi_debug_console_write_byte(c as char);
}
