//! Command-line control for Scarlet display backlights.
//!
//! This utility deliberately uses [`framebuffer::DisplayControl`] rather than
//! [`framebuffer::DisplaySurface`], so querying or setting brightness never
//! creates a framebuffer mapping.

use std::process::ExitCode;

#[cfg(target_os = "scarlet")]
use std::env;

#[cfg(target_os = "scarlet")]
use framebuffer::DisplayControl;

#[cfg(target_os = "scarlet")]
fn usage() {
    eprintln!("usage: displayctl <get|set <0..100>>");
}

#[cfg(target_os = "scarlet")]
fn open_display() -> Result<DisplayControl, ExitCode> {
    DisplayControl::open_primary().map_err(|error| {
        eprintln!("displayctl: no brightness-capable display: {error:?}");
        ExitCode::from(1)
    })
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
        "get" if args.next().is_none() => {
            let Ok(display) = open_display() else {
                return ExitCode::from(1);
            };
            match display.get_brightness_percent() {
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
            let Ok(display) = open_display() else {
                return ExitCode::from(1);
            };
            match display.set_brightness_percent(percent) {
                Ok(()) => {
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
