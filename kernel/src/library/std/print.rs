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
use core::fmt::Write;

use crate::device::manager::DeviceManager;
use crate::device::char::CharDevice;
use crate::early_println;

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::library::std::print::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    ($fmt:expr) => ($crate::print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::print!(concat!($fmt, "\n"), $($arg)*));
}

pub fn _print(args: fmt::Arguments) {
    let manager = DeviceManager::get_manager();
    
    // Try to find a character device (UART)
    if let Some(borrowed_device) = manager.get_first_device_by_type(crate::device::DeviceType::Char) {
        if let Some(char_device) = borrowed_device.as_char_device() {
            // Use CharDevice trait methods to write
            struct CharDeviceWriter<'a>(&'a dyn CharDevice);
            
            impl<'a> fmt::Write for CharDeviceWriter<'a> {
                fn write_str(&mut self, s: &str) -> fmt::Result {
                    for byte in s.bytes() {
                        if self.0.write_byte(byte).is_err() {
                            return Err(fmt::Error);
                        }
                    }
                    Ok(())
                }
            }
            
            let mut writer = CharDeviceWriter(char_device);
            if writer.write_fmt(args).is_ok() {
                return;
            }
        }
    }
    
    // Fallback to early_println if no character device found
    early_println!("[print] No character device found, using early console");
}
