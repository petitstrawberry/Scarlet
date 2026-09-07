//! Inspect and override the system-wide Scarlet tablet presentation state.

use core::time::Duration;
use std::process::ExitCode;
use std::thread;

use clap::{Parser, Subcommand, ValueEnum};
use sws_client::{Connection, Event, InputEnvironment, WindowingMode};

#[derive(Debug, Parser)]
#[command(
    name = "tabletctl",
    version,
    about = "Inspect or override system-wide tablet presentation"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the current posture, windowing mode, override sources, and devices.
    Status,
    /// Print the current state and every subsequent system-wide change.
    Watch,
    /// Force tablet/laptop posture, or return posture to hardware detection.
    Posture {
        #[arg(value_enum)]
        mode: PostureArgument,
    },
    /// Force focused/freeform windowing, or return to posture-derived policy.
    Windowing {
        #[arg(value_enum)]
        mode: WindowingArgument,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PostureArgument {
    Tablet,
    Laptop,
    Auto,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum WindowingArgument {
    Focused,
    Freeform,
    Auto,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let connection = match Connection::connect_default() {
        Ok(connection) => connection,
        Err(error) => {
            eprintln!("tabletctl: failed to connect to SWS: {}", error.as_str());
            return ExitCode::from(1);
        }
    };

    let result = match cli.command.unwrap_or(Command::Status) {
        Command::Status => connection.get_input_environment().map(print_environment),
        Command::Watch => return watch_environment(&connection),
        Command::Posture { mode } => connection
            .set_tablet_mode_override(match mode {
                PostureArgument::Tablet => Some(true),
                PostureArgument::Laptop => Some(false),
                PostureArgument::Auto => None,
            })
            .map(print_environment),
        Command::Windowing { mode } => connection
            .set_windowing_mode_override(match mode {
                WindowingArgument::Focused => Some(WindowingMode::Focused),
                WindowingArgument::Freeform => Some(WindowingMode::Freeform),
                WindowingArgument::Auto => None,
            })
            .map(print_environment),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tabletctl: request failed: {}", error.as_str());
            ExitCode::from(1)
        }
    }
}

fn watch_environment(connection: &Connection) -> ExitCode {
    match connection.get_input_environment() {
        Ok(environment) => print_environment(environment),
        Err(error) => {
            eprintln!("tabletctl: request failed: {}", error.as_str());
            return ExitCode::from(1);
        }
    }

    loop {
        if let Err(error) = connection.dispatch() {
            eprintln!("tabletctl: connection failed: {}", error.as_str());
            return ExitCode::from(1);
        }
        while let Some(event) = connection.poll_event() {
            if let Event::InputEnvironmentChanged(environment) = event {
                print_environment(environment);
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn print_environment(environment: InputEnvironment) {
    let posture = match environment.tablet_mode() {
        Some(true) => "tablet",
        Some(false) => "laptop",
        None => "unknown",
    };
    let posture_source = match environment.tablet_mode_override_active() {
        Some(true) => "override",
        Some(false) => "hardware",
        None => "unknown",
    };
    let windowing = match environment.windowing_mode() {
        Some(WindowingMode::Focused) => "focused",
        Some(WindowingMode::Freeform) => "freeform",
        None => "unknown",
    };
    let windowing_source = match environment.windowing_mode_override_active() {
        Some(true) => "override",
        Some(false) => "posture",
        None => "unknown",
    };

    println!(
        "generation={} posture={} posture_source={} windowing={} windowing_source={} direct_touch={} fine_pointer={} keyboard={} pen={}",
        environment.generation,
        posture,
        posture_source,
        windowing,
        windowing_source,
        environment.has_direct_touch(),
        environment.has_fine_pointer(),
        environment.has_keyboard(),
        environment.has_pen(),
    );
}
