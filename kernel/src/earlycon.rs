//! Early console for generic architecture.
//! 
//! This module provides a simple early console interface for the kernel. It is
//! used to print messages before the kernel heap is initialized.
//!
//! The early console is architecture-specific and must be implemented for each
//! architecture. 

use core::fmt::Write;

use crate::arch::early_putc;

#[macro_export]
macro_rules! early_print {
    ($($arg:tt)*) => ($crate::earlycon::print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! early_println {
    ($fmt:expr) => ($crate::early_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::early_print!(concat!($fmt, "\n"), $($arg)*));
}

pub fn print(args: core::fmt::Arguments) {
    let mut writer = EarlyConsole {};
    writer.write_fmt(args).unwrap();
}

struct EarlyConsole;

impl Write for EarlyConsole {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.bytes() {
            if c == b'\n' {
                early_putc(b'\r');
            }
            early_putc(c);
        }
        Ok(())
    }
}