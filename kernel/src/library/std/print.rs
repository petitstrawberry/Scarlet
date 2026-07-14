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
    // Serialize formatting only for the lock-free log ring buffer and the
    // early framebuffer. Both sinks are non-blocking, so holding PrintGuard
    // (which masks interrupts to prevent same-CPU FIQ re-entrancy) across
    // them is safe.
    {
        let _guard = crate::log::PrintGuard::acquire();

        struct LogWriter;

        impl fmt::Write for LogWriter {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                for &b in s.as_bytes() {
                    crate::log::write_byte(b);
                }
                Ok(())
            }
        }
        let mut log = LogWriter;
        let _ = log.write_fmt(args);

        if crate::earlyfb::is_initialized() {
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
    }
    // PrintGuard and its interrupt mask are released here. Char-device
    // emission runs with interrupts enabled and without the global print
    // lock: a slow or interrupt-driven device (e.g. a UART blocking on a
    // TX-ready FIQ) must never hold PrintGuard or mask FIQ, otherwise every
    // other CPU that prints spins forever with FIQ masked. This also makes
    // re-entrant print! from a driver's write callback safe instead of a
    // non-reentrant self-deadlock. Devices own their concurrency safety.

    let manager = DeviceManager::get_manager();

    // Helper: write to a specific CharDevice implementation
    struct CharDeviceWriter<'a>(&'a dyn CharDevice);
    impl<'a> fmt::Write for CharDeviceWriter<'a> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            if self.0.write(s.as_bytes()).is_err() {
                return Err(fmt::Error);
            }
            Ok(())
        }
    }

    // 1) Prefer devices that advertise Serial capability (raw UART-like)
    let count = manager.get_devices_count();
    for id in 1..=count {
        if let Some(dev) = manager.get_device(id) {
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
    }

    // 2) Otherwise choose any Char device that is NOT TTY-capable and NOT the null sink
    for id in 1..=count {
        if let Some(dev) = manager.get_device(id) {
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
    }

    // Final fallback: write directly to the early console. PrintGuard has
    // already been released above, so this runs without the global print lock.
    if !crate::earlyfb::is_initialized() {
        let mut early = crate::earlycon::EarlyConsole::new();
        let _ = early.write_fmt(args);
    }
}
