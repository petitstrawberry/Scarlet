//! Inspect physical posture and configure a display's desktop or tablet experience.

use core::time::Duration;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use sws_client::{Connection, Error, Event, InputEnvironment};
use sws_protocol::error_codes;
use sws_protocol::outputs::{
    OutputDescriptor, OutputEvent, OutputExperience, OutputSnapshot, OutputTransaction,
};

#[derive(Debug, Parser)]
#[command(
    name = "tabletctl",
    version,
    about = "Inspect posture and configure display experience"
)]
struct Cli {
    /// Display ID. Defaults to the built-in display, then the primary display.
    #[arg(long, global = true, value_name = "ID")]
    output: Option<u32>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List connected display IDs for --output.
    Outputs,
    /// Print hardware posture, applied experience, policy, and input devices.
    Status,
    /// Print status and follow changes to the display and its input environment.
    Watch,
    /// Set the display's Experience preference in Settings.
    #[command(visible_aliases = ["posture", "windowing"])]
    Experience {
        #[arg(value_enum)]
        mode: ExperienceArgument,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ExperienceArgument {
    /// Follow the posture sensor on a built-in display. External displays use Desktop.
    Auto,
    /// Use desktop presentation (also accepts laptop or freeform).
    #[value(aliases = ["laptop", "freeform"])]
    Desktop,
    /// Use tablet presentation (also accepts focused).
    #[value(alias = "focused")]
    Tablet,
}

impl From<ExperienceArgument> for OutputExperience {
    fn from(value: ExperienceArgument) -> Self {
        match value {
            ExperienceArgument::Auto => Self::Auto,
            ExperienceArgument::Desktop => Self::Desktop,
            ExperienceArgument::Tablet => Self::Tablet,
        }
    }
}

fn request_error(error: Error) -> String {
    match error {
        Error::ServerError(error_codes::STALE_OUTPUT_GENERATION) => {
            "displays changed before the update; run the command again".into()
        }
        Error::ServerError(error_codes::OUTPUT_CONFIGURATION_NOT_SAVED) => {
            "SWS could not save the display configuration".into()
        }
        Error::ServerError(code) => format!("SWS rejected the request (error {code})"),
        error => error.as_str().into(),
    }
}

fn target(snapshot: &OutputSnapshot, requested: Option<u32>) -> Result<&OutputDescriptor, String> {
    if let Some(id) = requested {
        return snapshot
            .outputs
            .iter()
            .find(|output| output.id == id)
            .ok_or_else(|| format!("output {id} is disconnected or unknown"));
    }
    snapshot
        .outputs
        .iter()
        .find(|output| output.internal)
        .or_else(|| snapshot.outputs.iter().find(|output| output.primary))
        .ok_or_else(|| "no built-in or primary display is available".into())
}

fn transaction(
    snapshot: OutputSnapshot,
    id: u32,
    experience: OutputExperience,
) -> Result<OutputTransaction, String> {
    let mut configuration = snapshot.configuration;
    let selected = configuration
        .iter_mut()
        .find(|config| config.id == id)
        .ok_or_else(|| format!("output {id} has no configuration"))?;
    selected.experience = experience;
    Ok(OutputTransaction {
        base_generation: snapshot.generation,
        configuration,
    })
}

fn status(
    snapshot: &OutputSnapshot,
    output: &OutputDescriptor,
    environment: InputEnvironment,
    tablet: bool,
) -> Result<String, String> {
    let policy = snapshot
        .configuration
        .iter()
        .find(|config| config.id == output.id)
        .ok_or_else(|| format!("output {} has no configuration", output.id))?
        .experience;
    let hardware = environment.hardware_tablet_mode();
    let posture = match hardware {
        Some(true) => "tablet",
        Some(false) => "laptop",
        None => "unknown",
    };
    let (configured, source) = match policy {
        OutputExperience::Auto if output.internal && hardware.is_some() => ("auto", "hardware"),
        OutputExperience::Auto => ("auto", "desktop-default"),
        OutputExperience::Desktop => ("desktop", "configured"),
        OutputExperience::Tablet => ("tablet", "configured"),
    };
    let source = if tablet == policy.tablet_experience(output.internal, hardware == Some(true)) {
        source
    } else {
        "compositor"
    };
    let (experience, windowing) = if tablet {
        ("tablet", "focused")
    } else {
        ("desktop", "freeform")
    };
    let posture_source = if hardware.is_some() {
        "hardware"
    } else {
        "unknown"
    };
    Ok(format!(
        "output={} name={:?} posture={} experience={} experience_source={} configured={} posture_source={} windowing={} windowing_source={} generation={} output_generation={} direct_touch={} fine_pointer={} keyboard={} pen={}",
        output.id,
        output.name,
        posture,
        experience,
        source,
        configured,
        posture_source,
        windowing,
        source,
        environment.generation,
        snapshot.generation,
        environment.has_direct_touch(),
        environment.has_fine_pointer(),
        environment.has_keyboard(),
        environment.has_pen(),
    ))
}

fn read_status(
    connection: &Connection,
    snapshot: &OutputSnapshot,
    id: Option<u32>,
) -> Result<String, String> {
    let output = target(snapshot, id)?;
    let context = connection
        .output_context(output.id)
        .map_err(request_error)?;
    let environment = connection.get_input_environment().map_err(request_error)?;
    status(snapshot, output, environment, context.tablet)
}

fn run(cli: Cli) -> Result<(), String> {
    let connection = Connection::connect_default().map_err(request_error)?;
    let mut snapshot = connection.outputs().map_err(request_error)?;
    match cli.command.unwrap_or(Command::Status) {
        Command::Outputs => {
            for output in &snapshot.outputs {
                println!(
                    "output={} name={:?} internal={} primary={}",
                    output.id, output.name, output.internal, output.primary
                );
            }
        }
        Command::Status => println!("{}", read_status(&connection, &snapshot, cli.output)?),
        Command::Experience { mode } => {
            let id = target(&snapshot, cli.output)?.id;
            snapshot = connection
                .configure_outputs(transaction(snapshot, id, mode.into())?)
                .map_err(request_error)?;
            println!(
                "{}",
                read_status(&connection, &snapshot, Some(id)).map_err(|e| format!(
                    "applied experience to output {id}, but reading status failed: {e}"
                ))?
            );
        }
        Command::Watch => {
            let mut previous = read_status(&connection, &snapshot, cli.output)?;
            println!("{previous}");
            loop {
                connection
                    .wait_for_window_events(Duration::from_secs(60))
                    .map_err(request_error)?;
                connection.dispatch().map_err(request_error)?;
                let mut changed = false;
                while let Some(event) = connection.poll_event() {
                    match event {
                        Event::InputEnvironmentChanged(_) => changed = true,
                        Event::Output(OutputEvent::Snapshot(next)) if next != snapshot => {
                            changed = true;
                        }
                        _ => {}
                    }
                }
                if changed {
                    snapshot = connection.outputs().map_err(request_error)?;
                    let current = read_status(&connection, &snapshot, cli.output)?;
                    if previous != current {
                        println!("{current}");
                        previous = current;
                    }
                }
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tabletctl: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sws_protocol::{
        input_environment_known_flags as known, input_environment_state_flags as state,
        outputs::{OutputConfiguration, OutputMode},
    };

    fn display(id: u32, internal: bool, primary: bool) -> OutputDescriptor {
        OutputDescriptor {
            id,
            connector_key: format!("connector-{id}"),
            monitor_key: format!("monitor-{id}"),
            identity_is_unique: true,
            name: format!("screen-{id}"),
            mode: OutputMode {
                pixel_width: 1920,
                pixel_height: 1080,
                refresh_millihz: 60_000,
            },
            primary,
            internal,
            physical_width_mm: 0,
            physical_height_mm: 0,
        }
    }

    fn displays() -> OutputSnapshot {
        OutputSnapshot {
            generation: 27,
            outputs: vec![display(8, false, true), display(3, true, false)],
            // Configuration order is independent of connector enumeration.
            configuration: vec![
                OutputConfiguration {
                    id: 3,
                    x: -1280,
                    y: 17,
                    scale_milli: 1500,
                    experience: OutputExperience::Auto,
                },
                OutputConfiguration {
                    id: 8,
                    x: 0,
                    y: 0,
                    scale_milli: 1250,
                    experience: OutputExperience::Tablet,
                },
            ],
        }
    }

    #[test]
    fn setters_preserve_other_outputs_and_use_the_settings_generation() {
        for (command, argument, experience) in [
            ("experience", "desktop", OutputExperience::Desktop),
            ("posture", "tablet", OutputExperience::Tablet),
            ("posture", "laptop", OutputExperience::Desktop),
            ("posture", "auto", OutputExperience::Auto),
            ("windowing", "focused", OutputExperience::Tablet),
            ("windowing", "freeform", OutputExperience::Desktop),
            ("windowing", "auto", OutputExperience::Auto),
        ] {
            let cli =
                Cli::try_parse_from(["tabletctl", command, argument, "--output", "8"]).unwrap();
            let Some(Command::Experience { mode }) = cli.command else {
                panic!("setter")
            };
            assert_eq!(OutputExperience::from(mode), experience);
            let snapshot = displays();
            let id = target(&snapshot, cli.output).unwrap().id;
            assert_eq!(id, 8);
            let update = transaction(snapshot.clone(), id, mode.into()).unwrap();
            let mut expected = snapshot.configuration;
            expected[1].experience = experience;
            assert_eq!(update.configuration, expected);
            assert_eq!(update.base_generation, snapshot.generation);
        }
    }

    #[test]
    fn defaults_to_internal_then_primary_and_rejects_an_unknown_id() {
        let mut snapshot = displays();
        assert_eq!(target(&snapshot, None).unwrap().id, 3);
        snapshot.outputs[1].internal = false;
        assert_eq!(target(&snapshot, None).unwrap().id, 8);
        assert!(target(&snapshot, Some(42)).is_err());
        assert!(transaction(snapshot, 42, OutputExperience::Desktop).is_err());
    }

    #[test]
    fn reports_sensor_and_applied_experience_independently() {
        let mut snapshot = displays();
        let mut environment = InputEnvironment {
            generation: 11,
            known_flags: known::HARDWARE_TABLET_MODE | known::TABLET_MODE,
            state_flags: state::HARDWARE_TABLET_MODE,
            capability_flags: 0,
        };
        for (index, policy, applied, expected) in [
            (
                0,
                OutputExperience::Desktop,
                false,
                "posture=tablet experience=desktop experience_source=configured configured=desktop",
            ),
            (
                0,
                OutputExperience::Auto,
                false,
                "posture=tablet experience=desktop experience_source=desktop-default configured=auto",
            ),
            (
                1,
                OutputExperience::Auto,
                true,
                "posture=tablet experience=tablet experience_source=hardware configured=auto",
            ),
            (
                1,
                OutputExperience::Auto,
                false,
                "posture=tablet experience=desktop experience_source=compositor configured=auto",
            ),
        ] {
            let output = &snapshot.outputs[index];
            snapshot
                .configuration
                .iter_mut()
                .find(|c| c.id == output.id)
                .unwrap()
                .experience = policy;
            let line = status(&snapshot, output, environment, applied).unwrap();
            assert!(line.contains(expected), "{line}");
        }
        environment.known_flags = known::TABLET_MODE;
        environment.state_flags = state::TABLET_MODE;
        let line = status(&snapshot, &snapshot.outputs[1], environment, false).unwrap();
        assert!(line.contains("posture=unknown experience=desktop experience_source=desktop-default configured=auto"), "{line}");
    }
}
