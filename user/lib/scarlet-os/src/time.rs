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

extern crate alloc;

use alloc::vec::Vec;

/// UTC offset (seconds, east positive) from a POSIX `TZ` string.
pub fn utc_offset_from_tz(tz: &str) -> Option<i64> {
    let rest = if let Some(stripped) = tz.strip_prefix('<') {
        stripped.split_once('>').map(|(_, r)| r).unwrap_or("")
    } else {
        let idx = tz.find(|c: char| !c.is_ascii_alphabetic())?;
        &tz[idx..]
    };
    parse_posix_offset(rest).map(|p| -p)
}

fn parse_posix_offset(s: &str) -> Option<i64> {
    let (sign, rest) = match s.as_bytes().first()? {
        b'-' => (-1i64, &s[1..]),
        b'+' => (1i64, &s[1..]),
        _ => (1i64, s),
    };
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == ':'))
        .unwrap_or(rest.len());
    let mut parts = rest[..end].split(':');
    let h: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts
        .next()
        .and_then(|p| p.parse::<i64>().ok())
        .unwrap_or(0);
    let sec: i64 = parts
        .next()
        .and_then(|p| p.parse::<i64>().ok())
        .unwrap_or(0);
    if parts.next().is_some() {
        return None;
    }
    Some(sign * (h * 3600 + m * 60 + sec))
}

/// Parse a TZif binary file and return the UTC offset (east positive) for `now_secs`.
fn tzif_offset(data: &[u8], now_secs: i64) -> Option<i64> {
    if data.len() < 44 || &data[0..4] != b"TZif" {
        return None;
    }
    let rd = |off: usize| -> i32 {
        i32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
    };
    let tzh_timecnt = rd(32) as usize;
    let tzh_typecnt = rd(36) as usize;

    // v1 transitions start at offset 44, each int32 BE
    let trans_start = 44;
    let trans_size = 4;
    let types_start = trans_start + tzh_timecnt * trans_size;
    let ttinfo_start = types_start + tzh_timecnt;

    if ttinfo_start + tzh_typecnt * 6 > data.len() {
        return None;
    }

    // Find last transition <= now
    let mut applicable_type: u8 = 0;
    for i in 0..tzh_timecnt {
        let t = rd(trans_start + i * trans_size) as i64;
        if t > now_secs {
            break;
        }
        applicable_type = data[types_start + i];
    }

    // Read ttinfo for that type: int32 utoff (BE), uint8 isdst, uint8 abbrind
    let base = ttinfo_start + applicable_type as usize * 6;
    if base + 6 > data.len() {
        return None;
    }
    Some(rd(base) as i64)
}

#[cfg(feature = "std")]
fn read_file(path: &str) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

#[cfg(not(feature = "std"))]
fn read_file(path: &str) -> Option<Vec<u8>> {
    use scarlet_sys::{syscall1, syscall3};
    let path_c = alloc::format!("{}\0", path);
    let handle = syscall3(Syscall::VfsOpen, path_c.as_ptr() as usize, 0, 0);
    if handle == usize::MAX {
        return None;
    }
    let mut buf = alloc::vec![0u8; 4096];
    let n = syscall3(
        Syscall::StreamRead,
        handle,
        buf.as_mut_ptr() as usize,
        buf.len(),
    );
    syscall1(Syscall::HandleClose, handle);
    if n == usize::MAX || n == 0 {
        return None;
    }
    buf.truncate(n);
    Some(buf)
}

/// Local UTC offset (seconds, east positive).
///
/// Priority: `TZ` env var → `/etc/localtime` (TZif) → UTC (None).
#[cfg(feature = "std")]
pub fn local_utc_offset_seconds() -> Option<i64> {
    if let Some(tz) = std::env::var("TZ")
        .ok()
        .and_then(|s| utc_offset_from_tz(&s))
    {
        return Some(tz);
    }
    if let Some(data) = read_file("/etc/localtime") {
        let now = system_time_ns().map(|ns| (ns / 1_000_000_000) as i64)?;
        return tzif_offset(&data, now);
    }
    None
}

#[cfg(not(feature = "std"))]
pub fn local_utc_offset_seconds() -> Option<i64> {
    if let Some(tz_str) = scarlet_rt::env::try_var("TZ") {
        if let Some(off) = utc_offset_from_tz(&tz_str) {
            return Some(off);
        }
    }
    if let Some(data) = read_file("/etc/localtime") {
        let now = system_time_ns().map(|ns| (ns / 1_000_000_000) as i64)?;
        return tzif_offset(&data, now);
    }
    None
}
