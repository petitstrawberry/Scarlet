//! Print the current wall-clock date and time (local, derived from `TZ`).
//!
//! Reads the wall clock via `scarlet_os::time::system_time_ns` and applies the
//! UTC offset parsed from the `TZ` environment variable.

use std::process::ExitCode;

use scarlet_os::time;

const SECS_PER_DAY: u64 = 86_400;
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

fn main() -> ExitCode {
    let utc_ns = match time::system_time_ns() {
        Some(ns) => ns,
        None => {
            eprintln!("date: wall clock unavailable (no RTC initialized)");
            return ExitCode::FAILURE;
        }
    };

    let offset = time::local_utc_offset_seconds().unwrap_or(0);
    let local_ns = utc_ns as i128 + offset as i128 * 1_000_000_000;
    let secs = (local_ns / 1_000_000_000) as u64;
    let nanos = (local_ns % 1_000_000_000) as u32;

    let (year, month, day, hh, mm, ss, weekday) = civil_from_unix(secs);

    let sign = if offset >= 0 { "+" } else { "-" };
    let oh = offset.unsigned_abs() / 3600;
    let om = (offset.unsigned_abs() % 3600) / 60;
    println!(
        "{} {:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:09} UTC{}{:02}{:02}",
        weekday, year, month, day, hh, mm, ss, nanos, sign, oh, om,
    );
    println!("epoch = {} ns", utc_ns);

    ExitCode::SUCCESS
}

/// Convert Unix epoch seconds to a UTC broken-down time plus weekday name.
///
/// Uses Howard Hinnant's `civil_from_days` algorithm
/// (<http://howardhinnant.github.io/date_algorithms.html>), valid for any
/// non-negative epoch second.
fn civil_from_unix(secs: u64) -> (i64, u32, u32, u32, u32, u32, &'static str) {
    let days = (secs / SECS_PER_DAY) as i64;
    let rem = secs % SECS_PER_DAY;
    let hh = (rem / 3600) as u32;
    let mm = ((rem % 3600) / 60) as u32;
    let ss = (rem % 60) as u32;

    let z = days + 719468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };

    // 1970-01-01 was a Thursday; days >= 0 so no negative modulo.
    let weekday = WEEKDAYS[((days % 7 + 4) % 7) as usize];

    (year, m, d, hh, mm, ss, weekday)
}
