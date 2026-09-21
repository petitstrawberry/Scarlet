//! Time utilities for the kernel
//!
//! This module provides time-related functionality for the kernel,
//! including current time access for filesystem operations.

use crate::sync::IrqSpinLock;

mod wall_clock;
use wall_clock::WallClock;

use crate::timer::get_time_us;

/// Get the current time in microseconds
///
/// This function returns the current system time in microseconds since boot.
/// For filesystem operations, this provides a monotonic timestamp.
pub fn current_time() -> u64 {
    // Use the current CPU's architected timer. The boot CPU is not guaranteed
    // to be CPU 0, and the supported SMP platforms expose synchronized
    // per-CPU timer counters.
    get_time_us()
}

/// Get the current time in milliseconds
pub fn current_time_ms() -> u64 {
    current_time() / 1000
}

/// Get the current time in seconds
pub fn current_time_s() -> u64 {
    current_time() / 1_000_000
}

/// Get the current time in nanoseconds
///
/// This function returns the current system time in nanoseconds since boot.
/// Useful for high-resolution timestamps in input events and profiling.
pub fn current_time_ns() -> u64 {
    current_time() * 1000
}

pub fn udelay(us: u64) {
    let start = current_time();
    while current_time() - start < us {
        core::hint::spin_loop();
    }
}

// ---------------------------------------------------------------------------
// Wall-clock (system / real) time
// ---------------------------------------------------------------------------

// Keep each UTC/monotonic pair coherent across CPUs, including readers in IRQs.
static WALL_CLOCK: IrqSpinLock<WallClock> = IrqSpinLock::new(WallClock::new());

/// Get the current wall-clock time in nanoseconds since the Unix epoch.
///
/// # Returns
///
/// `Some(ns)` once an RTC source or userspace has initialized the wall clock.
/// Adjustments can step this clock forward or backward; elapsed-time users
/// must use `current_time_ns()` instead.
pub fn system_time_ns() -> Option<u64> {
    let clock = WALL_CLOCK.lock();
    clock.read(current_time_ns())
}

/// Apply UTC valid at a prior monotonic instant, accounting for delivery delay.
/// This neither changes the monotonic clock nor writes any hardware RTC.
pub fn set_system_time_at(unix_ns: u64, monotonic_ns: u64) -> Result<(), &'static str> {
    let mut clock = WALL_CLOCK.lock();
    clock.set(unix_ns, monotonic_ns, current_time_ns())
}

/// Get the current wall-clock time in microseconds since the Unix epoch.
///
/// # Returns
///
/// `Some(us)` if the wall clock is available, or `None` otherwise.
pub fn system_time_us() -> Option<u64> {
    system_time_ns().map(|ns| ns / 1000)
}

/// Get the current wall-clock time in seconds since the Unix epoch.
///
/// # Returns
///
/// `Some(s)` if the wall clock is available, or `None` otherwise.
pub fn system_time_s() -> Option<u64> {
    system_time_ns().map(|ns| ns / 1_000_000_000)
}

/// Whether an RTC source or userspace has initialized the wall clock.
pub fn is_system_time_available() -> bool {
    system_time_ns().is_some()
}

/// Establish the wall-clock epoch from a single RTC sample.
///
/// Intended to be called exactly once by an RTC platform driver during its
/// probe. The caller brackets the RTC read with two monotonic samples
/// (`mono_before_ns` just before the RTC read, `mono_after_ns` just after) so
/// the midpoint is the best estimate of the monotonic instant at which the RTC
/// value was valid. A later RTC probe never replaces a userspace adjustment.
///
/// # Arguments
///
/// * `rtc_epoch_ns` - Wall-clock nanoseconds since the Unix epoch read from RTC.
/// * `mono_before_ns` - Monotonic nanoseconds since boot, sampled just before
///   the RTC read.
/// * `mono_after_ns` - Monotonic nanoseconds since boot, sampled just after
///   the RTC read.
///
/// # Returns
///
/// `Ok(())` on success, or an error string if the sample is invalid or the
/// wall clock was already initialized.
pub fn initialize_wall_clock_from_rtc_sample(
    rtc_epoch_ns: u64,
    mono_before_ns: u64,
    mono_after_ns: u64,
) -> Result<(), &'static str> {
    WALL_CLOCK
        .lock()
        .initialize(rtc_epoch_ns, mono_before_ns, mono_after_ns)
}

/// Convert microseconds to a human-readable format (for debugging)
pub fn format_time_us(time_us: u64) -> (u64, u64, u64) {
    let seconds = time_us / 1_000_000;
    let minutes = seconds / 60;
    let hours = minutes / 60;

    (hours, minutes % 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_format_time() {
        let (hours, minutes, seconds) = format_time_us(3_661_000_000); // 1 hour, 1 minute, 1 second
        assert_eq!(hours, 1);
        assert_eq!(minutes, 1);
        assert_eq!(seconds, 1);

        let (hours, minutes, seconds) = format_time_us(123_000_000); // 2 minutes, 3 seconds
        assert_eq!(hours, 0);
        assert_eq!(minutes, 2);
        assert_eq!(seconds, 3);
    }
}
