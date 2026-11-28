//! Command-line control for Scarlet displays.
//!
//! Inspect display endpoints, control the backlight, and configure per-output
//! Experience through the same SWS transaction as Settings.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[cfg(target_os = "scarlet")]
use framebuffer::{
    DISPLAY_CONNECTOR_KIND_EXTERNAL, DISPLAY_CONNECTOR_KIND_INTERNAL,
    DISPLAY_CONNECTOR_KIND_VIRTUAL, DisplayControl, DisplayOutputInfo,
};

#[path = "displayctl/experience.rs"]
mod experience;

#[derive(Parser)]
#[command(
    name = "displayctl",
    version,
    about = "Inspect displays and configure brightness and display experience"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List physical display endpoints and connectors.
    Info,
    /// Read the primary display's brightness.
    Get,
    /// Set the primary display's brightness (0 through 100).
    Set {
        #[arg(value_parser = parse_percent)]
        percent: u8,
    },
    #[command(flatten)]
    Session(experience::Command),
}

/// Highest `/dev/displayN` index `info` probes.
#[cfg(target_os = "scarlet")]
const DISPLAY_SCAN_LIMIT: usize = 16;

#[cfg(target_os = "scarlet")]
fn backlight(percent: Option<u8>) -> Result<u8, String> {
    if let Ok(display) = DisplayControl::open_primary() {
        if let Some(percent) = percent {
            display
                .set_brightness_percent(percent)
                .map_err(|error| format!("{error:?}"))?;
        }
        return display
            .get_brightness_percent()
            .map_err(|error| format!("{error:?}"));
    }

    let display =
        sws_client::Connection::connect_backlight().map_err(|error| format!("{error:?}"))?;
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

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Session(command) => experience::run(command),
        #[cfg(target_os = "scarlet")]
        Command::Info => return print_info(),
        #[cfg(target_os = "scarlet")]
        Command::Get => backlight(None).map(|value| println!("brightness={value}%")),
        #[cfg(target_os = "scarlet")]
        Command::Set { percent } => {
            backlight(Some(percent)).map(|value| println!("brightness={value}%"))
        }
        #[cfg(not(target_os = "scarlet"))]
        _ => Err("this utility requires Scarlet OS".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("displayctl: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_percent;

    #[test]
    fn only_experience_can_select_a_display_policy() {
        use super::Cli;
        use clap::Parser;
        for (command, value) in [
            ("posture", "tablet"),
            ("windowing", "focused"),
            ("experience", "laptop"),
            ("experience", "freeform"),
            ("experience", "focused"),
        ] {
            assert!(Cli::try_parse_from(["displayctl", command, value]).is_err());
        }
    }

    #[test]
    fn brightness_percent_accepts_only_the_supported_range() {
        assert_eq!(parse_percent("0"), Ok(0));
        assert_eq!(parse_percent("100"), Ok(100));
        assert!(parse_percent("101").is_err());
        assert!(parse_percent("-1").is_err());
        assert!(parse_percent("half").is_err());
    }
}
