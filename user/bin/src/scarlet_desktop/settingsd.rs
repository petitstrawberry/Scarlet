//! Headless desktop settings service.
//!
//! The settings application owns presentation only. This service owns the
//! persistent desktop background and status-preferences configuration, and
//! emits one signal after each successful change so renderers can update
//! immediately.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use sbus::{Argument, Message};
use sbus_client::Connection;
use scarlet_desktop_config::{
    BackgroundStyle, DESKTOP_BACKGROUND_CHANGED_SIGNAL, DESKTOP_BACKGROUND_CONFIG_PATH,
    DESKTOP_CONFIG_DIR, DESKTOP_SETTINGS_BUS_NAME, DESKTOP_SETTINGS_GET_STATUS_PREFERENCES_METHOD,
    DESKTOP_SETTINGS_INTERFACE, DESKTOP_SETTINGS_OBJECT_PATH,
    DESKTOP_SETTINGS_RESET_BACKGROUND_METHOD, DESKTOP_SETTINGS_RESET_STATUS_PREFERENCES_METHOD,
    DESKTOP_SETTINGS_SERVICE_INTERFACE, DESKTOP_SETTINGS_SERVICE_OBJECT_PATH,
    DESKTOP_SETTINGS_SET_BACKGROUND_METHOD, DESKTOP_SETTINGS_SET_STATUS_PREFERENCES_METHOD,
    DESKTOP_SETTINGS_SIGNAL_SENDER, DESKTOP_STATUS_CONFIG_PATH,
    DESKTOP_STATUS_PREFERENCES_CHANGED_SIGNAL, StatusPreferences, load_desktop_config,
};
use std::format;
use std::fs;
use std::io::Write;
use std::println;
use std::string::String;
use std::thread;
use std::time::Duration;
use std::vec;
use std::vec::Vec;

/// A same-directory staging path keeps the final rename atomic when the VFS supports it.
const DESKTOP_STATUS_CONFIG_TEMP_PATH: &str = "/etc/scarlet-desktop.d/status.toml.tmp";

fn parse_color(value: &str) -> Option<[u8; 3]> {
    let value = value.strip_prefix('#')?;
    if value.len() != 6 {
        return None;
    }
    Some([
        u8::from_str_radix(&value[0..2], 16).ok()?,
        u8::from_str_radix(&value[2..4], 16).ok()?,
        u8::from_str_radix(&value[4..6], 16).ok()?,
    ])
}

fn escape_toml_string(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn write_background_config(
    color: [u8; 3],
    style: BackgroundStyle,
    image: Option<&str>,
) -> Result<(), &'static str> {
    let _ = fs::create_directory(DESKTOP_CONFIG_DIR);
    let mut content = format!(
        "[theme]\nbackground = \"#{:02x}{:02x}{:02x}\"\nbackground_style = \"{}\"\n",
        color[0],
        color[1],
        color[2],
        style.as_str()
    );
    if let Some(image) = image.filter(|image| !image.is_empty()) {
        content.push_str(&format!(
            "background_image = \"{}\"\n",
            escape_toml_string(image)
        ));
    }

    let mut file = fs::File::create(DESKTOP_BACKGROUND_CONFIG_PATH)
        .map_err(|_| "failed to create desktop background config")?;
    file.write_all(content.as_bytes())
        .map_err(|_| "failed to write desktop background config")?;
    Ok(())
}

fn emit_background_changed(connection: &mut Connection) {
    if let Err(error) = connection.emit_signal(
        DESKTOP_SETTINGS_SIGNAL_SENDER,
        DESKTOP_SETTINGS_OBJECT_PATH,
        DESKTOP_SETTINGS_INTERFACE,
        DESKTOP_BACKGROUND_CHANGED_SIGNAL,
        Vec::new(),
    ) {
        println!("[settingsd] failed to emit BackgroundChanged: {:?}", error);
    }
}

fn emit_status_preferences_changed(connection: &mut Connection) {
    if let Err(error) = connection.emit_signal(
        DESKTOP_SETTINGS_SIGNAL_SENDER,
        DESKTOP_SETTINGS_OBJECT_PATH,
        DESKTOP_SETTINGS_INTERFACE,
        DESKTOP_STATUS_PREFERENCES_CHANGED_SIGNAL,
        Vec::new(),
    ) {
        println!(
            "[settingsd] failed to emit StatusPreferencesChanged: {:?}",
            error
        );
    }
}

fn argument_string<'a>(args: &'a [Argument], index: usize) -> Option<&'a str> {
    match args.get(index) {
        Some(Argument::String(value)) => Some(value.as_str()),
        _ => None,
    }
}

fn require_no_arguments(args: &[Argument]) -> Result<(), &'static str> {
    if args.is_empty() {
        Ok(())
    } else {
        Err("settings method does not accept arguments")
    }
}

fn parse_status_preferences_arguments(
    args: &[Argument],
) -> Result<StatusPreferences, &'static str> {
    if args.len() != 3 {
        return Err("status preferences require order, visible, and clock format strings");
    }

    let order = argument_string(args, 0).ok_or("status preference order must be a string")?;
    let visible =
        argument_string(args, 1).ok_or("status preference visibility must be a string")?;
    let clock_format =
        argument_string(args, 2).ok_or("status preference clock format must be a string")?;
    StatusPreferences::from_ipc_values(order, visible, clock_format)
}

fn status_preferences_arguments(preferences: &StatusPreferences) -> Vec<Argument> {
    vec![
        Argument::String(preferences.order_csv()),
        Argument::String(preferences.visible_csv()),
        Argument::String(String::from(preferences.clock_format.as_str())),
    ]
}

fn load_status_preferences() -> StatusPreferences {
    load_desktop_config().status
}

fn write_status_preferences(preferences: &StatusPreferences) -> Result<(), &'static str> {
    let _ = fs::create_directory(DESKTOP_CONFIG_DIR);
    let content = preferences.to_toml_section();
    let mut file = fs::File::create(DESKTOP_STATUS_CONFIG_TEMP_PATH)
        .map_err(|_| "failed to create temporary desktop status config")?;

    if file.write_all(content.as_bytes()).is_err() || file.flush().is_err() {
        let _ = fs::remove_file(DESKTOP_STATUS_CONFIG_TEMP_PATH);
        return Err("failed to write temporary desktop status config");
    }
    drop(file);

    if fs::rename(DESKTOP_STATUS_CONFIG_TEMP_PATH, DESKTOP_STATUS_CONFIG_PATH).is_err() {
        let _ = fs::remove_file(DESKTOP_STATUS_CONFIG_TEMP_PATH);
        return Err("failed to replace desktop status config");
    }

    Ok(())
}

fn send_error(connection: &mut Connection, serial: u32, message: &'static str) {
    let _ =
        connection.send_method_error(serial, "org.scarlet.desktop.Settings.InvalidArgs", message);
}

fn handle_message(connection: &mut Connection, message: Message) {
    let Message::CallMethod {
        path,
        interface,
        method,
        args,
        ..
    } = message
    else {
        return;
    };

    if path != DESKTOP_SETTINGS_SERVICE_OBJECT_PATH
        || interface != DESKTOP_SETTINGS_SERVICE_INTERFACE
    {
        return;
    }

    match method.as_str() {
        DESKTOP_SETTINGS_SET_BACKGROUND_METHOD => {
            let Some(color) = argument_string(&args, 0).and_then(parse_color) else {
                send_error(connection, 0, "background color must be #rrggbb");
                return;
            };
            let Some(style) = argument_string(&args, 1).and_then(BackgroundStyle::from_str) else {
                send_error(connection, 0, "invalid background style");
                return;
            };
            let image = argument_string(&args, 2).filter(|value| !value.is_empty());

            match write_background_config(color, style, image) {
                Ok(()) => {
                    let _ = connection.send_method_return(0, Vec::new());
                    emit_background_changed(connection);
                }
                Err(error) => send_error(connection, 0, error),
            }
        }
        DESKTOP_SETTINGS_RESET_BACKGROUND_METHOD => {
            if fs::remove_file(DESKTOP_BACKGROUND_CONFIG_PATH).is_err()
                && fs::File::open(DESKTOP_BACKGROUND_CONFIG_PATH).is_ok()
            {
                send_error(connection, 0, "failed to remove desktop background config");
                return;
            }
            let _ = connection.send_method_return(0, Vec::new());
            emit_background_changed(connection);
        }
        DESKTOP_SETTINGS_GET_STATUS_PREFERENCES_METHOD => {
            if let Err(error) = require_no_arguments(&args) {
                send_error(connection, 0, error);
                return;
            }
            let _ = connection
                .send_method_return(0, status_preferences_arguments(&load_status_preferences()));
        }
        DESKTOP_SETTINGS_SET_STATUS_PREFERENCES_METHOD => {
            let preferences = match parse_status_preferences_arguments(&args) {
                Ok(preferences) => preferences,
                Err(error) => {
                    send_error(connection, 0, error);
                    return;
                }
            };

            match write_status_preferences(&preferences) {
                Ok(()) => {
                    let _ = connection.send_method_return(0, Vec::new());
                    emit_status_preferences_changed(connection);
                }
                Err(error) => send_error(connection, 0, error),
            }
        }
        DESKTOP_SETTINGS_RESET_STATUS_PREFERENCES_METHOD => {
            if let Err(error) = require_no_arguments(&args) {
                send_error(connection, 0, error);
                return;
            }

            match write_status_preferences(&StatusPreferences::default()) {
                Ok(()) => {
                    let _ = connection.send_method_return(0, Vec::new());
                    emit_status_preferences_changed(connection);
                }
                Err(error) => send_error(connection, 0, error),
            }
        }
        _ => send_error(connection, 0, "unknown settings method"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Argument, StatusPreferences, parse_status_preferences_arguments, require_no_arguments,
        status_preferences_arguments,
    };
    use scarlet_desktop_config::{ClockFormat, StatusItemId};
    use std::string::String;
    use std::vec;

    #[test]
    fn status_preferences_set_arguments_require_three_strings() {
        let valid = vec![
            Argument::String(String::from("audio,cpu")),
            Argument::String(String::from("audio")),
            Argument::String(String::from("12h")),
        ];
        assert_eq!(
            parse_status_preferences_arguments(&valid).unwrap(),
            StatusPreferences {
                order: vec![StatusItemId::Audio, StatusItemId::Cpu],
                visible: vec![StatusItemId::Audio],
                clock_format: ClockFormat::TwelveHour,
            }
        );

        assert!(parse_status_preferences_arguments(&valid[..2]).is_err());
        assert!(
            parse_status_preferences_arguments(&[
                Argument::Int(1),
                Argument::String(String::from("audio")),
                Argument::String(String::from("12h")),
            ])
            .is_err()
        );
    }

    #[test]
    fn status_preferences_set_arguments_reject_invalid_preference_values() {
        let invalid = vec![
            Argument::String(String::from("cpu,cpu")),
            Argument::String(String::from("cpu")),
            Argument::String(String::from("24h")),
        ];
        assert!(parse_status_preferences_arguments(&invalid).is_err());
    }

    #[test]
    fn status_preferences_get_result_is_order_visible_then_clock_format() {
        let preferences = StatusPreferences::from_ipc_values("audio,cpu", "cpu", "12h").unwrap();
        let arguments = status_preferences_arguments(&preferences);

        assert!(matches!(arguments.as_slice(), [
            Argument::String(order),
            Argument::String(visible),
            Argument::String(clock_format),
        ] if order == "audio,cpu" && visible == "cpu" && clock_format == "12h"));
    }

    #[test]
    fn reset_status_preferences_uses_the_default_ipc_values() {
        let arguments = status_preferences_arguments(&StatusPreferences::default());

        assert!(matches!(arguments.as_slice(), [
            Argument::String(order),
            Argument::String(visible),
            Argument::String(clock_format),
        ] if order == "cpu,audio" && visible == "cpu,audio" && clock_format == "24h"));
    }

    #[test]
    fn get_and_reset_reject_unexpected_arguments() {
        assert!(require_no_arguments(&[]).is_ok());
        assert!(require_no_arguments(&[Argument::Boolean(true)]).is_err());
    }
}

fn run_service() {
    loop {
        let Ok(mut connection) = Connection::connect() else {
            thread::sleep(Duration::from_millis(100));
            continue;
        };
        if connection
            .register_service(DESKTOP_SETTINGS_BUS_NAME)
            .is_err()
        {
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        println!("[settingsd] registered as {}", DESKTOP_SETTINGS_BUS_NAME);

        loop {
            match connection.receive_message() {
                Ok(message) => handle_message(&mut connection, message),
                Err(error) => {
                    println!("[settingsd] sbus connection lost: {:?}", error);
                    break;
                }
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[settingsd] starting");
    run_service();
    0
}
