//! Time utilities for the kernel
//!
//! This module provides time-related functionality for the kernel,
//! including current time access for filesystem operations.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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

static WALL_CLOCK_BASE_NS: AtomicU64 = AtomicU64::new(0);
static WALL_CLOCK_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Get the current wall-clock time in nanoseconds since the Unix epoch.
///
/// # Returns
///
/// `Some(ns)` once an RTC source has initialized the wall clock, or `None`
/// before the first RTC probe completes.
pub fn system_time_ns() -> Option<u64> {
    if WALL_CLOCK_INITIALIZED.load(Ordering::Acquire) {
        let base = WALL_CLOCK_BASE_NS.load(Ordering::Relaxed);
        Some(base.wrapping_add(current_time_ns()))
    } else {
        None
    }
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

/// Whether the wall clock has been initialized from an RTC source.
pub fn is_system_time_available() -> bool {
    WALL_CLOCK_INITIALIZED.load(Ordering::Acquire)
}

/// Establish the wall-clock epoch from a single RTC sample.
///
/// Intended to be called exactly once by an RTC platform driver during its
/// probe. The caller brackets the RTC read with two monotonic samples
/// (`mono_before_ns` just before the RTC read, `mono_after_ns` just after) so
/// the midpoint is the best estimate of the monotonic instant at which the RTC
/// value was valid. The offset is `rtc_epoch_ns - midpoint_ns`.
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
pub(crate) fn initialize_wall_clock_from_rtc_sample(
    rtc_epoch_ns: u64,
    mono_before_ns: u64,
    mono_after_ns: u64,
) -> Result<(), &'static str> {
    // Best estimate of the monotonic instant matching the RTC reading.
    let midpoint_ns = mono_before_ns + (mono_after_ns.saturating_sub(mono_before_ns) / 2);

    if rtc_epoch_ns < midpoint_ns {
        // rtc_epoch_ns - midpoint would underflow: the RTC reports an epoch
        // older than the system's uptime, which is not a usable base.
        return Err("RTC epoch precedes monotonic uptime; rejected to avoid underflow");
    }

    let base_ns = rtc_epoch_ns - midpoint_ns;

    // First-wins: only the first RTC source seeds the wall clock.
    if WALL_CLOCK_INITIALIZED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("wall clock already initialized");
    }

    WALL_CLOCK_BASE_NS.store(base_ns, Ordering::Relaxed);
    Ok(())
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
