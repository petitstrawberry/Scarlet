//! Opt-in, bounded Home diagnostics shared by the shell and compositor.
//! Enable SCARLET_HOME_TRACE=1 in both service environments for one boot.
use core::sync::atomic::{AtomicUsize, Ordering};
use std::fmt::Arguments;
use std::sync::OnceLock;

static ENABLED: OnceLock<bool> = OnceLock::new();
static COUNTS: [AtomicUsize; 10] = [const { AtomicUsize::new(0) }; 10];

pub(crate) fn enabled() -> bool {
    *ENABLED.get_or_init(|| std::env::var("SCARLET_HOME_TRACE").as_deref() == Ok("1"))
}

/// Log the first 16 occurrences, then powers of two through 4096. Each
/// boundary emits at most 24 lines, including samples of delayed recovery.
pub(crate) fn record(stage: usize, details: Arguments<'_>) {
    if !enabled() {
        return;
    }
    let Some(counter) = COUNTS.get(stage) else { return };
    let Ok(previous) = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        (n < 4096).then_some(n + 1)
    }) else { return };
    let n = previous + 1;
    if n <= 16 || n.is_power_of_two() {
        std::println!(
            "[HOME_TRACE] ns={} stage={} n={} {}",
            scarlet_os::time::monotonic_time_ns(), stage, n, details,
        );
    }
}
