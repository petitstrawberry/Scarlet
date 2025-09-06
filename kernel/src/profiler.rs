//! Simple profiler for measuring function execution time.
//!
//! This module provides a simple profiling utility to measure the execution time,
//! call count, and other statistics for functions and code blocks. It uses a
//! global registry to store profiling data and a RAII guard (`ProfileGuard`)
//! to automatically record timings.
//!
//! # Usage
//!
//! To profile a scope, use the `profile_scope!` macro at the beginning of the
//! function or block you want to measure.
//!
//! ```
//! use crate::profiler::profile_scope;
//!
//! fn my_function() {
//!     profile_scope!("my_function");
//!     // ... function logic ...
//! }
//! ```
//!
//! At the end of your program or at a convenient checkpoint, call
//! `print_profiling_results()` to display the collected statistics.

#[macro_export]
macro_rules! profile_scope {
    ($name:expr) => {
        #[cfg(feature = "profiler")]
        let _guard = $crate::profiler::ProfileGuard::new($name);
    };
}

#[cfg(feature = "profiler")]
pub use self::profiler_impl::*;

#[cfg(feature = "profiler")]
mod profiler_impl {
    use alloc::collections::BTreeMap;
    use alloc::string::{String, ToString};
    use alloc::sync::Arc;
    use spin::Mutex;
    use lazy_static::lazy_static;
    use crate::early_println;
    use crate::timer::get_time_ns;

    /// Holds performance statistics for a single profiled scope.
    pub struct ProfileData {
        pub call_count: u64,
        pub total_time_ns: u64,
        pub min_time_ns: u64,
        pub max_time_ns: u64,
    }

    lazy_static! {
        /// Global registry for storing profiling data.
        ///
        /// It's a `BTreeMap` to ensure the output is alphabetically sorted by function name.
        pub static ref PROFILER_REGISTRY: Arc<Mutex<BTreeMap<String, ProfileData>>> =
            Arc::new(Mutex::new(BTreeMap::new()));
    }

    /// A RAII guard that records the execution time of its scope.
    ///
    /// When created, it records the start time. When it goes out of scope (and is dropped),
    /// it calculates the elapsed time and updates the global profiler registry.
    pub struct ProfileGuard {
        name: &'static str,
        start_time: u64,
    }

    impl ProfileGuard {
        /// Creates a new `ProfileGuard`.
        pub fn new(name: &'static str) -> Self {
            ProfileGuard {
                name,
                start_time: get_time_ns(),
            }
        }
    }

    impl Drop for ProfileGuard {
        fn drop(&mut self) {
            let end_time = get_time_ns();
            let elapsed = end_time.saturating_sub(self.start_time);

            let mut registry = PROFILER_REGISTRY.lock();
            let data = registry.entry(self.name.to_string()).or_insert(ProfileData {
                call_count: 0,
                total_time_ns: 0,
                min_time_ns: u64::MAX,
                max_time_ns: 0,
            });

            data.call_count += 1;
            data.total_time_ns += elapsed;
            if elapsed < data.min_time_ns {
                data.min_time_ns = elapsed;
            }
            if elapsed > data.max_time_ns {
                data.max_time_ns = elapsed;
            }
        }
    }

    /// Prints the collected profiling results to the console.
    ///
    /// The results are formatted into a table for readability, including call count,
    /// total time, average time, min/max time, and the percentage of total time
    /// consumed by each function.
    pub fn print_profiling_results() {
        let registry = PROFILER_REGISTRY.lock();
        if registry.is_empty() {
            early_println!("[Profiler] No profiling data collected.");
            return;
        }

        let total_system_time: u64 = registry.values().map(|d| d.total_time_ns).sum();

        early_println!("\n--- Profiling Results ---");
        early_println!(
            "{:<40} | {:>10} | {:>15} | {:>15} | {:>15} | {:>15} | {:>8}",
            "Function", "Count", "Total (ms)", "Average (μs)", "Min (μs)", "Max (μs)", "% Total"
        );
        early_println!("{:-<130}", "");

        for (name, data) in registry.iter() {
            if data.call_count == 0 {
                continue;
            }
            let total_ms = data.total_time_ns as f64 / 1_000_000.0;
            let avg_us = (data.total_time_ns / data.call_count) as f64 / 1_000.0;
            let min_us = data.min_time_ns as f64 / 1_000.0;
            let max_us = data.max_time_ns as f64 / 1_000.0;
            let percent_total = if total_system_time > 0 {
                (data.total_time_ns as f64 / total_system_time as f64) * 100.0
            } else {
                0.0
            };

            early_println!(
                "{:<40} | {:>10} | {:>15.3} | {:>15.3} | {:>15.3} | {:>15.3} | {:>7.2}%",
                name, data.call_count, total_ms, avg_us, min_us, max_us, percent_total
            );
        }
        early_println!("-------------------------\n");
    }
}
