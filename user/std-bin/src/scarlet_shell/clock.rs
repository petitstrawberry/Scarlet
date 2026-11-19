//! Wall time and presentation shared by the desktop and login screens.
use scarlet_desktop_config::ClockFormat;
use std::fmt::Write;

pub fn local_seconds_of_day() -> Option<u32> {
    let seconds = (scarlet_os::time::system_time_ns()? / 1_000_000_000) as i64;
    let offset = scarlet_os::time::local_utc_offset_seconds().unwrap_or(0);
    Some((seconds + offset).rem_euclid(86_400) as u32)
}

/// Formats a local time using a configured 12-hour or 24-hour format.
///
/// # Arguments
///
/// * `hour` - Local hour. Values outside 0 through 23 are reduced modulo 24.
/// * `minute` - Local minute. Values outside 0 through 59 are reduced modulo 60.
/// * `format` - The configured clock presentation.
///
/// # Returns
///
/// A compact clock label. The clock is intentionally independent of optional
/// status-item visibility and ordering.
pub fn format_clock(hour: u8, minute: u8, format: ClockFormat) -> String {
    let hour = hour % 24;
    let minute = minute % 60;
    let mut label = String::new();
    match format {
        ClockFormat::TwentyFourHour => {
            let _ = write!(label, "{:02}:{:02}", hour, minute);
        }
        ClockFormat::TwelveHour => {
            let meridiem = if hour < 12 { "AM" } else { "PM" };
            let hour = match hour % 12 {
                0 => 12,
                value => value,
            };
            let _ = write!(label, "{}:{:02} {}", hour, minute, meridiem);
        }
    }
    label
}
