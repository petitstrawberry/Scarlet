use std::process::ExitCode;

use clap::{Parser, Subcommand};
use sas_client::{ControlState, SasClient};
use sas_protocol::{CONTROL_FLAG_MUTED, MASTER_VOLUME_UNITY_Q16};

#[derive(Debug, Parser)]
#[command(name = "sasctl", version, about = "Control Scarlet Audio Server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show current SAS master output state.
    Status,
    /// Show or set master volume as a percentage.
    Volume {
        /// Volume in the range 0..100, with an optional trailing '%'.
        value: Option<String>,
    },
    /// Mute SAS master output.
    Mute,
    /// Unmute SAS master output.
    Unmute,
    /// Toggle SAS master mute.
    ToggleMute,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut client = match SasClient::connect() {
        Ok(client) => client,
        Err(error) => {
            eprintln!("sasctl: failed to connect to SAS: {}", error.as_str());
            return ExitCode::from(1);
        }
    };

    let result = match cli.command.unwrap_or(Command::Status) {
        Command::Status => client.control_state(),
        Command::Volume { value: None } => client.control_state(),
        Command::Volume { value: Some(value) } => match parse_volume_percent(&value) {
            Ok(percent) => client.set_master_volume_q16(percent_to_q16(percent)),
            Err(message) => {
                eprintln!("sasctl: {message}");
                return ExitCode::from(2);
            }
        },
        Command::Mute => client.set_master_muted(true),
        Command::Unmute => client.set_master_muted(false),
        Command::ToggleMute => match client.control_state() {
            Ok(state) => client.set_master_muted(!state_muted(state)),
            Err(error) => Err(error),
        },
    };

    match result {
        Ok(state) => {
            print_state(state);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("sasctl: request failed: {}", error.as_str());
            ExitCode::from(1)
        }
    }
}

fn parse_volume_percent(input: &str) -> Result<u32, &'static str> {
    let trimmed = input.trim();
    let number = trimmed.strip_suffix('%').unwrap_or(trimmed);
    if number.is_empty() {
        return Err("volume must be 0..100%");
    }

    let percent = number
        .parse::<u32>()
        .map_err(|_| "volume must be an integer percentage")?;
    if percent > 100 {
        return Err("volume must be 0..100%");
    }
    Ok(percent)
}

fn percent_to_q16(percent: u32) -> u32 {
    ((percent as u64 * MASTER_VOLUME_UNITY_Q16 as u64 + 50) / 100) as u32
}

fn q16_to_percent(volume_q16: u32) -> u32 {
    ((volume_q16 as u64 * 100 + (MASTER_VOLUME_UNITY_Q16 / 2) as u64)
        / MASTER_VOLUME_UNITY_Q16 as u64) as u32
}

fn state_muted(state: ControlState) -> bool {
    state.flags & CONTROL_FLAG_MUTED != 0
}

fn print_state(state: ControlState) {
    println!(
        "master_volume={}%, muted={}",
        q16_to_percent(state.master_volume_q16),
        state_muted(state)
    );
}
