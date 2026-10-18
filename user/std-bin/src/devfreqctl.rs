//! Control the kernel's common non-CPU frequency policies.
use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    process::ExitCode,
};

fn command(args: &[String]) -> Result<Option<String>, &'static str> {
    let words: Vec<_> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["get"] => Ok(None),
        ["set", device, value] => {
            if value.parse::<u64>().ok().filter(|v| *v != 0).is_none() {
                return Err("frequency must be a positive integer in kHz");
            }
            validate_device(device)?;
            Ok(Some(format!("device {device} frequency {value}\n")))
        }
        ["governor", device, governor] => {
            validate_device(device)?;
            if !matches!(
                *governor,
                "performance" | "powersave" | "userspace" | "simple_ondemand"
            ) {
                return Err("unknown governor");
            }
            Ok(Some(format!("device {device} governor {governor}\n")))
        }
        _ => Err("usage: devfreqctl [get|set <device> <kHz>|governor <device> <name>]"),
    }
}
fn validate_device(name: &str) -> Result<(), &'static str> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    {
        Err("invalid device name")
    } else {
        Ok(())
    }
}
fn run() -> Result<(), String> {
    if let Some(command) = command(&env::args().skip(1).collect::<Vec<_>>())? {
        OpenOptions::new()
            .write(true)
            .open("/dev/devfreq")
            .and_then(|mut file| file.write_all(command.as_bytes()))
            .map_err(|error| error.to_string())?;
    }
    print!(
        "{}",
        fs::read_to_string("/dev/devfreq").map_err(|error| error.to_string())?
    );
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("devfreqctl: {error}");
            ExitCode::from(1)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::command;
    fn parse(words: &[&str]) -> Result<Option<String>, &'static str> {
        command(&words.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn rejects_injected_commands_and_invalid_rates() {
        assert!(parse(&["set", "gpu\nfrequency", "300000"]).is_err());
        assert!(parse(&["set", "gpu", "0"]).is_err());
        assert!(parse(&["set", "gpu", "-1"]).is_err());
        assert!(parse(&["governor", "gpu", "unknown"]).is_err());
        assert_eq!(
            parse(&["set", "gpu", "300000"]),
            Ok(Some("device gpu frequency 300000\n".into()))
        );
        assert_eq!(parse(&["get"]), Ok(None));
    }
}
