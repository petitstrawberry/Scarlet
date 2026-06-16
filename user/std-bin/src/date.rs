//! Print the current wall-clock date and time.
//!
//! Reads the kernel wall clock via `scarlet_os::time::system_time` and formats
//! it as a human-readable UTC timestamp. Fails if no RTC has initialized the
//! wall clock.

use std::process::ExitCode;

use scarlet_os::time;

const SECS_PER_DAY: u64 = 86_400;
const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

fn main() -> ExitCode {
    let Some(elapsed) = time::system_time() else {
        eprintln!("date: wall clock unavailable (no RTC initialized)");
        return ExitCode::FAILURE;
    };

    let total_ns = elapsed.as_nanos();
    let secs = elapsed.as_secs();
    let nanos = elapsed.subsec_nanos();

    let (year, month, day, hh, mm, ss, weekday) = civil_from_unix(secs);

    println!(
        "{} {:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:09} UTC",
        weekday, year, month, day, hh, mm, ss, nanos,
    );
    println!("epoch = {} ns", total_ns);

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
