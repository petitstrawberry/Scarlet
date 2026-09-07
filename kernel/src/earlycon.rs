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

pub struct EarlyConsole;

impl EarlyConsole {
    pub const fn new() -> Self {
        Self
    }
}
impl Write for EarlyConsole {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.bytes() {
            if c == b'\n' {
                early_putc(b'\r');
            }
            early_putc(c);
            crate::log::write_byte(c);
        }
        Ok(())
    }
}

pub fn print(args: core::fmt::Arguments) {
    let _guard = crate::log::PrintGuard::acquire();
    let mut writer = EarlyConsole {};
    let _ = writer.write_fmt(args);
}
