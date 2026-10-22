//! Native time queries, with an explicit UTC clock for desktop host previews.

#[cfg(target_os = "scarlet")]
pub use scarlet_os::time::{local_utc_offset_seconds, system_time_ns};

#[cfg(target_os = "scarlet")]
pub fn monotonic_time() -> std::time::Duration {
    scarlet_os::time::monotonic_time()
}

#[cfg(not(target_os = "scarlet"))]
pub fn monotonic_time() -> std::time::Duration {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    ORIGIN.get_or_init(std::time::Instant::now).elapsed()
}

#[cfg(not(target_os = "scarlet"))]
pub fn system_time_ns() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
}

#[cfg(not(target_os = "scarlet"))]
pub fn local_utc_offset_seconds() -> Option<i64> {
    Some(0)
}

pub fn zone_label() -> &'static str {
    #[cfg(target_os = "scarlet")]
    {
        if local_utc_offset_seconds().is_some() {
            "Local time"
        } else {
            "UTC"
        }
    }
    #[cfg(not(target_os = "scarlet"))]
    {
        "UTC (host preview)"
    }
}
