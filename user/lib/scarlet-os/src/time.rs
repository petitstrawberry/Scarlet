//! Scarlet Native monotonic time APIs.

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
