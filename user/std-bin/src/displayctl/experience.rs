//! Inspect physical posture and configure a display's desktop or tablet experience.

use core::time::Duration;

use clap::{Args, Subcommand, ValueEnum};
use sws_client::{Connection, Error, Event, InputEnvironment};
use sws_protocol::error_codes;
use sws_protocol::outputs::{
    OutputDescriptor, OutputEvent, OutputExperience, OutputRole, OutputSnapshot, OutputTransaction,
};

#[derive(Debug, Args)]
pub struct Target {
    /// Display ID from 'displayctl outputs'. Defaults to the built-in or primary desktop.
    #[arg(long, value_name = "ID")]
    output: Option<u32>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List connected compositor output IDs and their roles.
    Outputs,
    /// Show the display's configured and applied experience and the physical posture.
    Status(Target),
    /// Follow changes to the display and its input environment.
    Watch(Target),
    /// Set the selected display's Experience preference in Settings.
    Experience {
        #[arg(value_enum)]
        mode: ExperienceArgument,
        #[command(flatten)]
        target: Target,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ExperienceArgument {
    /// Follow the built-in display's posture sensor. External displays use Desktop.
    Auto,
    Desktop,
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
        let output = snapshot
            .outputs
            .iter()
            .find(|output| output.id == id)
            .ok_or_else(|| format!("output {id} is disconnected or unknown"))?;
        if !snapshot
            .configuration
            .iter()
            .any(|config| config.id == id && config.role == OutputRole::Extend)
        {
            return Err(format!("output {id} has no independent desktop"));
        }
        return Ok(output);
    }
    sws_protocol::outputs::extended_primary(&snapshot.outputs, &snapshot.configuration)
        .and_then(|id| snapshot.outputs.iter().find(|output| output.id == id))
        .ok_or_else(|| "no active desktop display is available".into())
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
    let experience = if tablet { "tablet" } else { "desktop" };
    Ok(format!(
        "output={} name={:?} posture={} experience={} experience_source={} configured={} generation={} output_generation={} direct_touch={} fine_pointer={} keyboard={} pen={}",
        output.id,
        output.name,
        posture,
        experience,
        source,
        configured,
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

pub fn run(command: Command) -> Result<(), String> {
    if !cfg!(target_os = "scarlet") {
        return Err("this utility requires Scarlet OS".into());
    }
    let connection = Connection::connect_default().map_err(request_error)?;
    let mut snapshot = connection.outputs().map_err(request_error)?;
    match command {
        Command::Outputs => {
            for output in &snapshot.outputs {
                let role = snapshot
                    .configuration
                    .iter()
                    .find(|entry| entry.id == output.id)
                    .map(|entry| match entry.role {
                        OutputRole::Extend => "extend",
                        OutputRole::Mirror => "mirror",
                        OutputRole::Disabled => "disabled",
                    })
                    .unwrap_or("unknown");
                println!(
                    "output={} name={:?} internal={} primary={} role={}",
                    output.id, output.name, output.internal, output.primary, role
                );
            }
        }
        Command::Status(selected) => {
            println!("{}", read_status(&connection, &snapshot, selected.output)?)
        }
        Command::Experience {
            mode,
            target: selected,
        } => {
            let id = target(&snapshot, selected.output)?.id;
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
        Command::Watch(selected) => {
            let mut previous = read_status(&connection, &snapshot, selected.output)?;
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
                    let current = read_status(&connection, &snapshot, selected.output)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use sws_protocol::{
        input_environment_known_flags as known, input_environment_state_flags as state,
        outputs::{OutputConfiguration, OutputMode, OutputRole},
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
                    role: OutputRole::Extend,
                    experience: OutputExperience::Auto,
                },
                OutputConfiguration {
                    id: 8,
                    x: 0,
                    y: 0,
                    scale_milli: 1250,
                    role: OutputRole::Extend,
                    experience: OutputExperience::Tablet,
                },
            ],
        }
    }

    #[test]
    fn setters_preserve_other_outputs_and_use_the_settings_generation() {
        use crate::{Cli, Command::Session};
        use clap::Parser;
        for (argument, experience) in [
            ("auto", OutputExperience::Auto),
            ("desktop", OutputExperience::Desktop),
            ("tablet", OutputExperience::Tablet),
        ] {
            let cli = Cli::try_parse_from(["displayctl", "experience", argument, "--output", "8"])
                .unwrap();
            let Session(Command::Experience {
                mode,
                target: selected,
            }) = cli.command
            else {
                panic!("expected experience");
            };
            assert_eq!(OutputExperience::from(mode), experience);
            let snapshot = displays();
            let id = target(&snapshot, selected.output).unwrap().id;
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
        snapshot.configuration[0].role = OutputRole::Disabled;
        assert_eq!(target(&snapshot, None).unwrap().id, 8);
        assert!(target(&snapshot, Some(3)).is_err());
        snapshot.configuration[0].role = OutputRole::Mirror;
        assert!(target(&snapshot, Some(3)).is_err());
        snapshot.configuration[0].role = OutputRole::Extend;
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
