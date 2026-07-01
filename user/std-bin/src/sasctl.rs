use std::process::ExitCode;

use clap::{Parser, Subcommand};
use sas_client::{ControlState, OutputInfo, OutputRequest, SasClient};
use sas_protocol::{
    CONTROL_FLAG_MUTED, MASTER_VOLUME_UNITY_Q16, OUTPUT_ENTRY_FLAG_COMPATIBLE,
    OUTPUT_ENTRY_FLAG_CURRENT, OUTPUT_PREFERENCE_HEADPHONES, OUTPUT_PREFERENCE_NAME,
    OUTPUT_PREFERENCE_PATH, OUTPUT_PREFERENCE_SPEAKERS,
};

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
    /// Control SAS output device.
    Output {
        #[command(subcommand)]
        command: Option<OutputCommand>,
    },
}

#[derive(Debug, Subcommand)]
enum OutputCommand {
    /// Show current output device.
    Status,
    /// List output devices.
    List,
    /// Switch output device.
    Set {
        /// Output: speakers, headphones, /dev/audioN, or stable device name.
        value: String,
    },
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
        Command::Output { command: None } => client.control_state(),
        Command::Output {
            command: Some(OutputCommand::Status),
        } => client.control_state(),
        Command::Output {
            command: Some(OutputCommand::List),
        } => {
            return print_outputs(&mut client);
        }
        Command::Output {
            command: Some(OutputCommand::Set { value }),
        } => match parse_output_request(&value) {
            Ok(request) => client.set_output(request),
            Err(message) => {
                eprintln!("sasctl: {message}");
                return ExitCode::from(2);
            }
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

fn parse_output_request(input: &str) -> Result<OutputRequest, &'static str> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("output must be speakers, headphones, /dev/audioN, or a device name");
    }

    let (preference, value) = match trimmed {
        "speaker" | "speakers" => (OUTPUT_PREFERENCE_SPEAKERS, ""),
        "headphone" | "headphones" | "headset" => (OUTPUT_PREFERENCE_HEADPHONES, ""),
        _ if trimmed.starts_with("/dev/audio") => (OUTPUT_PREFERENCE_PATH, trimmed),
        _ => (OUTPUT_PREFERENCE_NAME, trimmed),
    };
    OutputRequest::new(preference, value).ok_or("output value is too long")
}

fn fixed_str(bytes: &[u8]) -> &str {
    let len = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..len]).unwrap_or("")
}

fn output_kind_name(kind: u32) -> &'static str {
    match kind {
        1 => "speakers",
        2 => "headphones",
        _ => "unknown",
    }
}

fn print_outputs(client: &mut SasClient) -> ExitCode {
    match client.list_outputs() {
        Ok(outputs) => {
            for output in outputs {
                print_output(output);
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("sasctl: request failed: {}", error.as_str());
            ExitCode::from(1)
        }
    }
}

fn print_output(output: OutputInfo) {
    let current = if output.flags & OUTPUT_ENTRY_FLAG_CURRENT != 0 {
        "*"
    } else {
        " "
    };
    let compatible = if output.flags & OUTPUT_ENTRY_FLAG_COMPATIBLE != 0 {
        "compatible"
    } else {
        "unsupported"
    };
    println!(
        "{} {} kind={} type={} name={} description={} {}",
        current,
        fixed_str(&output.path),
        output.kind,
        output_kind_name(output.kind),
        fixed_str(&output.name),
        fixed_str(&output.description),
        compatible
    );
}

fn print_state(state: ControlState) {
    println!(
        "master_volume={}%, muted={}, output={} kind={} name={} description={}",
        q16_to_percent(state.master_volume_q16),
        state_muted(state),
        fixed_str(&state.output_path),
        state.output_kind,
        fixed_str(&state.output_name),
        fixed_str(&state.output_description)
    );
}
