//! Headless desktop settings service.
//!
//! The settings application owns presentation only. This service owns the
//! persistent desktop background configuration and emits one signal after a
//! successful change so renderers can update immediately.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use sbus::{Argument, Message};
use sbus_client::Connection;
use scarlet_desktop_config::{
    BackgroundStyle, DESKTOP_BACKGROUND_CHANGED_SIGNAL, DESKTOP_BACKGROUND_CONFIG_PATH,
    DESKTOP_CONFIG_DIR, DESKTOP_SETTINGS_BUS_NAME, DESKTOP_SETTINGS_INTERFACE,
    DESKTOP_SETTINGS_OBJECT_PATH, DESKTOP_SETTINGS_RESET_BACKGROUND_METHOD,
    DESKTOP_SETTINGS_SERVICE_INTERFACE, DESKTOP_SETTINGS_SERVICE_OBJECT_PATH,
    DESKTOP_SETTINGS_SET_BACKGROUND_METHOD, DESKTOP_SETTINGS_SIGNAL_SENDER,
};
use std::format;
use std::fs;
use std::println;
use std::string::String;
use std::thread;
use std::time::Duration;
use std::vec::Vec;

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

fn argument_string<'a>(args: &'a [Argument], index: usize) -> Option<&'a str> {
    match args.get(index) {
        Some(Argument::String(value)) => Some(value.as_str()),
        _ => None,
    }
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
        _ => send_error(connection, 0, "unknown settings method"),
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
