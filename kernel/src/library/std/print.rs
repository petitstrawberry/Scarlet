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
use core::fmt;

use crate::device::manager::DeviceManager;
use crate::early_println;
use crate::early_print;

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
    let mut manager = DeviceManager::locked();
    let serial = manager.basic.borrow_mut_serial(0);
    match serial {
        Some(serial) => {
            serial.write_fmt(args).unwrap();
        }
        None => {
            early_println!("[print] No serial device found!");
        }
    }    
}
