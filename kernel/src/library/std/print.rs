//! # Print Macros and UART Handling
//!
//! This module provides functionality for formatted printing through a UART device.
//! It defines the core printing macros (`print!` and `println!`) used throughout the kernel,
//! along with the necessary infrastructure to handle UART output.
//!
//! ## Examples
//!
//! ```
//! println!("Hello, world!");
//! println!("Value: {}", 42);
//! print!("No newline here");
//! ```
//!
//! ## Implementation Details
//!
//! The module initializes a UART writer lazily when first used and provides the
//! core implementation of the `Write` trait for the UART device. It automatically
//! handles CR+LF conversion for newlines.

/// Implements core printing functionality by writing formatted text to the UART.
/// This function is called by the `print!` macro and handles lazy initialization
/// of the UART writer if it doesn't exist.
///
/// # Arguments
///
/// * `args` - Formatted arguments to print
///
/// # Note
///
/// This function is not meant to be called directly. Use the `print!` or
/// `println!` macros instead.

/// Wraps a UART device to implement the `core::fmt::Write` trait.
///
/// This allows the UART to be used with the standard formatting macros.
use core::fmt::{self, Write};

use crate::driver::uart::virt::Uart;
use crate::traits::serial::Serial;

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::library::std::print::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    ($fmt:expr) => (print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => (print!(concat!($fmt, "\n"), $($arg)*));
}

pub fn _print(args: fmt::Arguments) {
    unsafe {
        match UART_WRITER {
            Some(ref mut writer) => writer.write_fmt(args).unwrap(),
            None => {
                UART_WRITER = Some(UartWriter {
                    serial: Uart::new(0x1000_0000),
                });
                if let Some(ref mut writer) = UART_WRITER {
                    writer.serial.init();
                }
                _print(args);
            }
            
        }
    }
}

static mut UART_WRITER: Option<UartWriter> = None;

#[derive(Clone)]
struct UartWriter {
    serial: Uart,
}

impl Write for UartWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.bytes() {
            if c == b'\n' {
                self.serial.write_byte(b'\r');
            }
            self.serial.write_byte(c);
        }
        Ok(())
    }
}
