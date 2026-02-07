//! Early console for generic architecture.
//!
//! This module provides a simple early console interface for the kernel. It is
//! used to print messages before the kernel heap is initialized.
//!
//! The early console is architecture-specific and must be implemented for each
//! architecture.
//!
//! A global spinlock (`CONSOLE_LOCK`) serializes all output so that lines from
//! different CPUs do not interleave.

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::early_putc;

/// Global console lock shared by `early_println!` and `println!`.
///
/// Acquired with `acquire_console_lock()`, released with `release_console_lock()`.
/// The panic handler uses `try_acquire_console_lock()` to avoid deadlock.
pub static CONSOLE_LOCK: AtomicBool = AtomicBool::new(false);

/// Acquire the console lock (spinning until available).
///
/// Disables interrupts before acquiring to prevent self-deadlock (a CPU
/// holding the lock gets interrupted and the handler tries to print).
/// Returns `true` if interrupts were previously enabled, so that
/// `release_console_lock()` can restore the original state.
#[inline]
pub fn acquire_console_lock() -> bool {
    let was_enabled = crate::arch::interrupt::are_interrupts_enabled();
    crate::arch::disable_interrupt();
    while CONSOLE_LOCK
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    was_enabled
}

/// Try to acquire the console lock without spinning.
///
/// Disables interrupts if the lock is acquired. Returns `Some(was_enabled)`
/// if acquired (where `was_enabled` indicates the prior interrupt state),
/// or `None` if the lock is already held (panic path uses this).
#[inline]
pub fn try_acquire_console_lock() -> Option<bool> {
    let was_enabled = crate::arch::interrupt::are_interrupts_enabled();
    crate::arch::disable_interrupt();
    if CONSOLE_LOCK
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        Some(was_enabled)
    } else {
        // Failed to acquire — restore interrupt state.
        if was_enabled {
            crate::arch::enable_interrupt();
        }
        None
    }
}

/// Release the console lock and restore the interrupt state.
///
/// # Arguments
///
/// * `was_enabled` - If `true`, re-enable interrupts after releasing.
#[inline]
pub fn release_console_lock(was_enabled: bool) {
    CONSOLE_LOCK.store(false, Ordering::Release);
    if was_enabled {
        crate::arch::enable_interrupt();
    }
}

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
    let was_enabled = acquire_console_lock();
    let mut writer = EarlyConsole {};
    let _ = writer.write_fmt(args);
    release_console_lock(was_enabled);
}

/// Print without acquiring the console lock.
///
/// Used by the panic handler when it already holds or cannot acquire the lock.
pub fn print_unlocked(args: core::fmt::Arguments) {
    let mut writer = EarlyConsole {};
    let _ = writer.write_fmt(args);
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
