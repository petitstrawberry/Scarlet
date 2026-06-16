//! Scarlet Native monotonic and wall-clock time APIs.

use core::time::Duration;
use scarlet_sys::{Syscall, syscall0};

/// Return boot-relative monotonic time in nanoseconds.
///
/// The value is suitable for measuring elapsed time and is not affected by
/// realtime clock adjustments. It is sourced from the kernel monotonic clock.
///
/// # Returns
///
/// Nanoseconds elapsed since boot.
pub fn monotonic_time_ns() -> u64 {
    syscall0(Syscall::MonotonicTime) as u64
}

/// Return boot-relative monotonic time as a [`Duration`].
///
/// # Returns
///
/// Duration elapsed since boot.
pub fn monotonic_time() -> Duration {
    Duration::from_nanos(monotonic_time_ns())
}

/// Return wall-clock nanoseconds since the Unix epoch.
///
/// # Returns
///
/// `Some(ns)` if an RTC source has initialized the wall clock, or `None` if
/// wall-clock time is unavailable (e.g. no RTC present).
pub fn system_time_ns() -> Option<u64> {
    let ns = syscall0(Syscall::SystemTime) as u64;
    if ns == u64::MAX { None } else { Some(ns) }
}

/// Return wall-clock time since the Unix epoch as a [`Duration`].
///
/// # Returns
///
/// `Some(Duration)` if the wall clock is available, or `None` otherwise.
pub fn system_time() -> Option<Duration> {
    system_time_ns().map(Duration::from_nanos)
}
