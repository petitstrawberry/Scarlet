//! Early console for generic architecture.
//!
//! This module provides a simple early console interface for the kernel. It is
//! used by `print!` and `println!` until the normal console is enabled, including
//! before the kernel heap is initialized. Callers do not select the backend.
//!
//! The early console is architecture-specific and must be implemented for each
//! architecture.

use core::fmt::Write;

use crate::arch::early_putc;

/// Early-console writer that also records output in the kernel log ring.
pub struct EarlyConsole;

impl EarlyConsole {
    pub const fn new() -> Self {
        Self
    }
}
impl Write for EarlyConsole {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.bytes() {
            write_console_byte(c);
            crate::log::write_byte(c);
        }
        Ok(())
    }
}

/// Architecture early-console output without recording the message in the log ring.
///
/// Used when the print path has already recorded the message. The caller must
/// serialize output; architecture backends may still acquire their own locks.
pub(crate) struct ConsoleOutput;

impl Write for ConsoleOutput {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.bytes() {
            write_console_byte(c);
        }
        Ok(())
    }
}

fn write_console_byte(byte: u8) {
    if byte == b'\n' {
        early_putc(b'\r');
    }
    early_putc(byte);
}

pub fn print(args: core::fmt::Arguments) {
    let _guard = crate::log::PrintGuard::acquire();
    let mut writer = EarlyConsole {};
    let _ = writer.write_fmt(args);
}
