//! Command-line control for Scarlet displays.
//!
//! `info` lists every display endpoint and its connector; `get`/`set` control
//! the backlight through the display endpoint or the desktop compositor.

use std::process::ExitCode;

#[cfg(target_os = "scarlet")]
use std::env;

#[cfg(target_os = "scarlet")]
use framebuffer::{
    DISPLAY_CONNECTOR_KIND_EXTERNAL, DISPLAY_CONNECTOR_KIND_INTERNAL,
    DISPLAY_CONNECTOR_KIND_VIRTUAL, DisplayControl, DisplayOutputInfo,
};

/// Highest `/dev/displayN` index `info` probes.
#[cfg(target_os = "scarlet")]
const DISPLAY_SCAN_LIMIT: usize = 16;

#[cfg(target_os = "scarlet")]
fn usage() {
    eprintln!("usage: displayctl <info|get|set <0..100>>");
}

#[cfg(target_os = "scarlet")]
fn backlight(percent: Option<u8>) -> Result<u8, String> {
    if let Ok(display) = DisplayControl::open_primary() {
        if let Some(percent) = percent {
            display.set_brightness_percent(percent).map_err(|error| format!("{error:?}"))?;
        }
        return display.get_brightness_percent().map_err(|error| format!("{error:?}"));
    }

    let display = sws_client::Connection::connect_backlight()
        .map_err(|error| format!("{error:?}"))?;
    match percent {
        Some(percent) => display.set_backlight_percent(percent),
        None => display.backlight_percent(),
    }
    .map_err(|error| format!("{error:?}"))
}

#[cfg(target_os = "scarlet")]
fn kind_name(kind: u32) -> &'static str {
    match kind {
        DISPLAY_CONNECTOR_KIND_INTERNAL => "internal",
        DISPLAY_CONNECTOR_KIND_EXTERNAL => "external",
        DISPLAY_CONNECTOR_KIND_VIRTUAL => "virtual",
        _ => "unknown",
    }
}

#[cfg(target_os = "scarlet")]
fn describe(path: &str, info: &DisplayOutputInfo) -> String {
    let mut line = format!(
        "{path}: connector={} ({}/{}) kind={} {} {}x{}",
        info.name(),
        info.connector_index,
        info.connector_count,
        kind_name(info.kind),
        if info.is_connected() {
            "connected"
        } else {
            "disconnected"
        },
        info.width,
        info.height,
    );
    if info.refresh_mhz != 0 {
        line.push_str(&format!(
            "@{}.{:03}Hz",
            info.refresh_mhz / 1000,
            info.refresh_mhz % 1000
        ));
    }
    if info.physical_width_mm != 0 && info.physical_height_mm != 0 {
        line.push_str(&format!(
            " {}x{}mm",
            info.physical_width_mm, info.physical_height_mm
        ));
    }
    if info.is_primary() {
        line.push_str(" primary");
    }
    line
}

#[cfg(target_os = "scarlet")]
fn print_info() -> ExitCode {
    let mut found = false;
    for index in 0..DISPLAY_SCAN_LIMIT {
        let path = format!("/dev/display{index}");
        let Ok(display) = DisplayControl::open(&path) else {
            continue;
        };
        found = true;
        match display.get_output_info() {
            Ok(info) => println!("{}", describe(&path, &info)),
            Err(error) => println!("{path}: output info unavailable: {error:?}"),
        }
    }
    if found {
        ExitCode::SUCCESS
    } else {
        eprintln!("displayctl: no display endpoints found");
        ExitCode::from(1)
    }
}

fn parse_percent(value: &str) -> Result<u8, &'static str> {
    let percent = value
        .parse::<u8>()
        .map_err(|_| "brightness must be an integer from 0 through 100")?;
    if percent > 100 {
        return Err("brightness must be an integer from 0 through 100");
    }
    Ok(percent)
}

#[cfg(not(target_os = "scarlet"))]
fn main() -> ExitCode {
    eprintln!("displayctl: this utility requires Scarlet OS");
    ExitCode::from(1)
}

#[cfg(target_os = "scarlet")]
fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        usage();
        return ExitCode::from(2);
    };

    match command.as_str() {
        "info" if args.next().is_none() => print_info(),
        "get" if args.next().is_none() => {
            match backlight(None) {
                Ok(percent) => {
                    println!("brightness={percent}%");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("displayctl: failed to read brightness: {error:?}");
                    ExitCode::from(1)
                }
            }
        }
        "set" => {
            let Some(value) = args.next() else {
                usage();
                return ExitCode::from(2);
            };
            if args.next().is_some() {
                usage();
                return ExitCode::from(2);
            }
            let percent = match parse_percent(&value) {
                Ok(percent) => percent,
                Err(message) => {
                    eprintln!("displayctl: {message}");
                    return ExitCode::from(2);
                }
            };
            match backlight(Some(percent)) {
                Ok(percent) => {
                    println!("brightness={percent}%");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("displayctl: failed to set brightness: {error:?}");
                    ExitCode::from(1)
                }
            }
        }
        _ => {
            usage();
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_percent;

    #[test]
    fn brightness_percent_accepts_only_the_supported_range() {
        assert_eq!(parse_percent("0"), Ok(0));
        assert_eq!(parse_percent("100"), Ok(100));
        assert!(parse_percent("101").is_err());
        assert!(parse_percent("-1").is_err());
        assert!(parse_percent("half").is_err());
    }
}
