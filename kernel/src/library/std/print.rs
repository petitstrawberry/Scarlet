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
///
/// Wraps a UART device to implement the `core::fmt::Write` trait.
///
/// This allows the UART to be used with the standard formatting macros.
use core::fmt;
use core::fmt::Write;

use crate::device::char::CharDevice;
use crate::device::manager::DeviceManager;
use crate::device::{DeviceCapability, DeviceType};

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
    let _guard = crate::log::PrintGuard::acquire();

    struct LogWriter;

    impl fmt::Write for LogWriter {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for &byte in s.as_bytes() {
                crate::log::write_byte(byte);
            }
            Ok(())
        }
    }
    let mut log = LogWriter;
    let _ = log.write_fmt(args);

    if crate::earlyfb::is_redirection_enabled() && crate::earlyfb::is_initialized() {
        struct EarlyFramebufferWriter;

        impl fmt::Write for EarlyFramebufferWriter {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                crate::earlyfb::write_str(s);
                Ok(())
            }
        }

        let mut early_framebuffer = EarlyFramebufferWriter;
        let _ = early_framebuffer.write_fmt(args);
    }

    let manager = DeviceManager::get_manager();

    struct CharDeviceWriter<'a>(&'a dyn CharDevice);

    impl<'a> fmt::Write for CharDeviceWriter<'a> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.0.write(s.as_bytes()).map_err(|_| fmt::Error)?;
            Ok(())
        }
    }

    // 1) Prefer devices that advertise Serial capability (raw UART-like)
    let devices = manager.get_devices_with_ids();
    for (_, dev) in &devices {
        if dev.device_type() == DeviceType::Char
            && dev.capabilities().contains(&DeviceCapability::Serial)
            && dev.name() != "null"
        {
            if let Some(char_dev) = dev.as_char_device() {
                let mut writer = CharDeviceWriter(char_dev);
                if writer.write_fmt(args).is_ok() {
                    return;
                }
            }
        }
    }

    // 2) Otherwise choose any Char device that is NOT TTY-capable and NOT the null sink
    for (_, dev) in &devices {
        if dev.device_type() == DeviceType::Char
            && !dev.capabilities().contains(&DeviceCapability::Tty)
        {
            if let Some(char_dev) = dev.as_char_device() {
                let mut writer = CharDeviceWriter(char_dev);
                if writer.write_fmt(args).is_ok() {
                    return;
                }
            }
        }
    }

    if !crate::earlyfb::is_initialized() {
        let mut early = crate::earlycon::EarlyConsole::new();
        let _ = early.write_fmt(args);
    }
}
