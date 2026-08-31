//! Scarlet Desktop Settings
//!
//! Modern settings application for Scarlet Desktop

#![no_std]
#![no_main]

extern crate scarlet_std as std;
extern crate scarlet_ui_macros;

use core::f32;
use core::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use framebuffer::DisplayControl;
use sas_client::{Error as SasError, SasClient};
use sas_protocol::{
    CONTROL_FLAG_MUTED, ControlState, MASTER_VOLUME_UNITY_Q16, OUTPUT_ENTRY_FLAG_COMPATIBLE,
    OUTPUT_ENTRY_FLAG_CURRENT, OUTPUT_PREFERENCE_PATH, OutputInfo, OutputRequest,
};
use sbus::{Argument, Message};
use sbus_client::Connection as SbusConnection;
use scarlet_desktop_config::{
    BackgroundStyle, ClockFormat, DESKTOP_FILE_MANAGER_BUS_NAME, DESKTOP_FILE_MANAGER_INTERFACE,
    DESKTOP_FILE_MANAGER_OBJECT_PATH, DESKTOP_FILE_MANAGER_OPEN_FILE_METHOD,
    DESKTOP_FILE_MANAGER_RESPONSE_SIGNAL, DESKTOP_FILES_APP_ID, DESKTOP_SETTINGS_BUS_NAME,
    DESKTOP_SETTINGS_GET_STATUS_PREFERENCES_METHOD, DESKTOP_SETTINGS_RESET_BACKGROUND_METHOD,
    DESKTOP_SETTINGS_RESET_STATUS_PREFERENCES_METHOD, DESKTOP_SETTINGS_SERVICE_INTERFACE,
    DESKTOP_SETTINGS_SERVICE_OBJECT_PATH, DESKTOP_SETTINGS_SET_BACKGROUND_METHOD,
    DESKTOP_SETTINGS_SET_STATUS_PREFERENCES_METHOD, StatusItemId, StatusPreferences,
};
use scarlet_ui::{
    HeaderBar, Icon, IconSize, IconView, NavigationLink, State, StateId, hstack, navigation,
    prelude::*, vstack, zstack,
};
use scarlet_ui_macros::View;
use std::format;
use std::fs;
use std::io::Write;
use std::println;
use std::string::String;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use std::vec;
use std::vec::Vec;
use sws_client::{Connection, Error as SwsError, InputMethodInfo};

// Preset colors - Apple system-style palette
#[derive(Clone, Copy, Debug)]
pub struct PresetColor {
    pub name: &'static str,
    pub color: [u8; 3],
}

const DEFAULT_BG_PREVIEW: [u8; 3] = [40, 40, 50];
const DEFAULT_STYLE: BackgroundStyle = BackgroundStyle::GradientLines;
const SWS_CONFIG_DIR: &str = "/etc/sws";
const SWS_CONFIG_PATH: &str = "/etc/sws/config.toml";
const SWS_CONFIG_TEMP_PATH: &str = "/tmp/sws-config.settings.tmp";
const CURSOR_THEME_ROOT: &str = "/share/cursors";
const DEFAULT_CURSOR_THEME_PATH: &str = "/share/cursors/default";
const MAX_SETTINGS_TEXT_BYTES: usize = 64 * 1024;

static AUDIO_SAS_CLIENT: Mutex<Option<SasClient>> = Mutex::new(None);
static PICKER_REQUEST_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static PICKER_UI_EVENTS: Mutex<Vec<PickerUiEvent>> = Mutex::new(Vec::new());

const SBUS_METHOD_TIMEOUT_MS: u64 = 1_000;
const PICKER_REQUEST_ATTEMPTS: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CursorThemeInfo {
    name: String,
    path: String,
}

enum PickerUiEvent {
    Opened(String),
    OpenFailed(String),
    Response {
        request_id: String,
        accepted: bool,
        path: String,
    },
    /// The file manager service disappeared while a picker was open.
    ServiceGone,
}

const PRESET_COLORS: &[PresetColor] = &[
    PresetColor {
        name: "Default",
        color: DEFAULT_BG_PREVIEW,
    },
    PresetColor {
        name: "Space Gray",
        color: [120, 120, 128],
    },
    PresetColor {
        name: "Blue",
        color: [0, 122, 255],
    },
    PresetColor {
        name: "Green",
        color: [52, 199, 89],
    },
    PresetColor {
        name: "Orange",
        color: [255, 149, 0],
    },
    PresetColor {
        name: "Red",
        color: [255, 59, 48],
    },
    PresetColor {
        name: "Purple",
        color: [175, 82, 222],
    },
    PresetColor {
        name: "Teal",
        color: [90, 200, 250],
    },
];

fn fixed_sas_str(bytes: &[u8]) -> &str {
    let len = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..len]).unwrap_or("")
}

fn q16_to_percent(volume_q16: u32) -> u32 {
    ((volume_q16 as u64 * 100 + (MASTER_VOLUME_UNITY_Q16 / 2) as u64)
        / MASTER_VOLUME_UNITY_Q16 as u64) as u32
}

fn percent_to_q16(percent: u32) -> u32 {
    ((percent.min(100) as u64 * MASTER_VOLUME_UNITY_Q16 as u64 + 50) / 100) as u32
}

fn home_path() -> String {
    std::env::var("HOME")
        .filter(|path| !path.is_empty())
        .unwrap_or_else(|| String::from("/"))
}

fn f32_to_percent(value: f32) -> u32 {
    (value.max(0.0).min(100.0) + 0.5) as u32
}

fn load_display_brightness() -> (f32, String) {
    match DisplayControl::open_primary().and_then(|display| display.get_brightness_percent()) {
        Ok(percent) => {
            let percent = percent.min(100);
            (percent as f32, format!("Current brightness: {}%", percent))
        }
        Err(error) => {
            println!("[settings] failed to read display brightness: {:?}", error);
            (
                80.0,
                String::from("Display brightness is unavailable; showing the default value"),
            )
        }
    }
}

fn output_kind_name(kind: u32) -> &'static str {
    match kind {
        1 => "Speakers",
        2 => "Headphones",
        _ => "Audio",
    }
}

fn output_label_from_parts(kind: u32, name: &str, description: &str, path: &str) -> String {
    let primary = if !description.is_empty() {
        description
    } else if !name.is_empty() {
        name
    } else {
        output_kind_name(kind)
    };

    if path.is_empty() {
        String::from(primary)
    } else {
        format!("{} ({})", primary, path)
    }
}

fn output_label_from_info(output: &OutputInfo) -> String {
    output_label_from_parts(
        output.kind,
        fixed_sas_str(&output.name),
        fixed_sas_str(&output.description),
        fixed_sas_str(&output.path),
    )
}

fn output_label_from_state(state: ControlState) -> String {
    output_label_from_parts(
        state.output_kind,
        fixed_sas_str(&state.output_name),
        fixed_sas_str(&state.output_description),
        fixed_sas_str(&state.output_path),
    )
}

fn audio_status_text(state: ControlState) -> String {
    let muted = state.flags & CONTROL_FLAG_MUTED != 0;
    let volume = q16_to_percent(state.master_volume_q16).min(100);
    let label = output_label_from_state(state);
    if muted {
        format!("Muted - {} - {}%", label, volume)
    } else {
        format!("{} - {}%", label, volume)
    }
}

fn save_background_via_service(
    color: [u8; 3],
    style: BackgroundStyle,
    image: Option<&str>,
) -> core::result::Result<(), sbus_client::Error> {
    let mut connection = SbusConnection::connect()?;
    let method = if style == DEFAULT_STYLE && color == DEFAULT_BG_PREVIEW && image.is_none() {
        DESKTOP_SETTINGS_RESET_BACKGROUND_METHOD
    } else {
        DESKTOP_SETTINGS_SET_BACKGROUND_METHOD
    };
    let args = if method == DESKTOP_SETTINGS_SET_BACKGROUND_METHOD {
        vec![
            Argument::String(format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])),
            Argument::String(String::from(style.as_str())),
            Argument::String(String::from(image.unwrap_or_default())),
        ]
    } else {
        Vec::new()
    };
    connection
        .call_method_timeout(
            DESKTOP_SETTINGS_BUS_NAME,
            DESKTOP_SETTINGS_SERVICE_OBJECT_PATH,
            DESKTOP_SETTINGS_SERVICE_INTERFACE,
            method,
            args,
            SBUS_METHOD_TIMEOUT_MS,
        )
        .map(|_| ())
}

fn parse_status_preferences_response(
    args: &[Argument],
) -> core::result::Result<StatusPreferences, &'static str> {
    if args.len() != 3 {
        return Err("settingsd returned an incomplete status preference response");
    }
    let order = match &args[0] {
        Argument::String(value) => value.as_str(),
        _ => return Err("settingsd returned an invalid status item order"),
    };
    let visible = match &args[1] {
        Argument::String(value) => value.as_str(),
        _ => return Err("settingsd returned an invalid visible item list"),
    };
    let clock_format = match &args[2] {
        Argument::String(value) => value.as_str(),
        _ => return Err("settingsd returned an invalid clock format"),
    };
    StatusPreferences::from_ipc_values(order, visible, clock_format)
}

fn call_status_preferences_method(
    method: &str,
    args: Vec<Argument>,
) -> core::result::Result<Vec<Argument>, String> {
    let mut connection = SbusConnection::connect().map_err(|error| format!("{error:?}"))?;
    connection
        .call_method_timeout(
            DESKTOP_SETTINGS_BUS_NAME,
            DESKTOP_SETTINGS_SERVICE_OBJECT_PATH,
            DESKTOP_SETTINGS_SERVICE_INTERFACE,
            method,
            args,
            SBUS_METHOD_TIMEOUT_MS,
        )
        .map_err(|error| format!("{error:?}"))
}

fn get_status_preferences_via_service() -> core::result::Result<StatusPreferences, String> {
    let response =
        call_status_preferences_method(DESKTOP_SETTINGS_GET_STATUS_PREFERENCES_METHOD, Vec::new())?;
    parse_status_preferences_response(&response).map_err(String::from)
}

fn load_status_preferences() -> (StatusPreferences, String) {
    match get_status_preferences_via_service() {
        Ok(preferences) => (
            preferences,
            String::from("Loaded authoritative preferences from settingsd"),
        ),
        Err(error) => {
            println!(
                "[settings] failed to load status preferences through settingsd: {}",
                error
            );
            (
                scarlet_desktop_config::load_desktop_config().status,
                format!("settingsd unavailable; showing saved configuration ({error})"),
            )
        }
    }
}

fn save_status_preferences_via_service(
    preferences: &StatusPreferences,
) -> core::result::Result<StatusPreferences, String> {
    let order = preferences.order_csv();
    let visible = preferences.visible_csv();
    let clock_format = preferences.clock_format.as_str();
    let strict =
        StatusPreferences::from_ipc_values(&order, &visible, clock_format).map_err(String::from)?;
    let response = call_status_preferences_method(
        DESKTOP_SETTINGS_SET_STATUS_PREFERENCES_METHOD,
        vec![
            Argument::String(order),
            Argument::String(visible),
            Argument::String(String::from(clock_format)),
        ],
    )?;

    if response.is_empty() {
        Ok(strict)
    } else {
        parse_status_preferences_response(&response).map_err(String::from)
    }
}

fn reset_status_preferences_via_service() -> core::result::Result<StatusPreferences, String> {
    let response = call_status_preferences_method(
        DESKTOP_SETTINGS_RESET_STATUS_PREFERENCES_METHOD,
        Vec::new(),
    )?;
    if response.is_empty() {
        get_status_preferences_via_service()
    } else {
        parse_status_preferences_response(&response).map_err(String::from)
    }
}

fn set_status_item_visibility(
    preferences: &StatusPreferences,
    item: StatusItemId,
    visible: bool,
) -> StatusPreferences {
    let mut updated = preferences.clone();
    if visible {
        if !updated.visible.contains(&item) {
            updated.visible.push(item);
        }
    } else {
        updated.visible.retain(|candidate| *candidate != item);
    }
    updated.visible = updated
        .order
        .iter()
        .copied()
        .filter(|candidate| updated.visible.contains(candidate))
        .collect();
    updated
}

fn move_status_item(
    preferences: &StatusPreferences,
    item: StatusItemId,
    offset: isize,
) -> Option<StatusPreferences> {
    let index = preferences
        .order
        .iter()
        .position(|candidate| *candidate == item)?;
    let destination = index.checked_add_signed(offset)?;
    if destination >= preferences.order.len() || destination == index {
        return None;
    }

    let mut updated = preferences.clone();
    updated.order.swap(index, destination);
    updated.visible = updated
        .order
        .iter()
        .copied()
        .filter(|candidate| preferences.visible.contains(candidate))
        .collect();
    Some(updated)
}

fn request_background_picker() -> core::result::Result<String, sbus_client::Error> {
    let mut connection = SbusConnection::connect()?;
    let result = connection.call_method_timeout(
        DESKTOP_FILE_MANAGER_BUS_NAME,
        DESKTOP_FILE_MANAGER_OBJECT_PATH,
        DESKTOP_FILE_MANAGER_INTERFACE,
        DESKTOP_FILE_MANAGER_OPEN_FILE_METHOD,
        vec![
            Argument::String(String::from("Choose Wallpaper")),
            Argument::String(home_path()),
            Argument::String(String::from("image/jpeg")),
            Argument::Boolean(false),
            Argument::Boolean(false),
        ],
        SBUS_METHOD_TIMEOUT_MS,
    )?;

    match result.first() {
        Some(Argument::String(request_id)) => Ok(request_id.clone()),
        _ => Err(sbus_client::Error::ProtocolError(
            "FileManager returned no picker request id",
        )),
    }
}

fn ensure_file_manager_service() -> core::result::Result<(), sbus_client::Error> {
    let mut connection = SbusConnection::connect()?;
    connection
        .call_method_timeout(
            "org.scarlet-os.stemd",
            "/org/scarlet/os/stemd",
            "org.scarlet-os.stemd",
            "LaunchOrFocus",
            vec![Argument::String(String::from(DESKTOP_FILES_APP_ID))],
            SBUS_METHOD_TIMEOUT_MS,
        )
        .map(|_| ())
}

fn start_picker_response_listener() {
    thread::spawn(move || {
        loop {
            let Ok(mut connection) = SbusConnection::connect() else {
                thread::sleep(Duration::from_millis(100));
                continue;
            };

            loop {
                let message = match connection.receive_message() {
                    Ok(message) => message,
                    Err(error) => {
                        println!("[settings] picker response connection lost: {:?}", error);
                        break;
                    }
                };
                let Message::Signal {
                    sender,
                    path,
                    interface,
                    signal,
                    args,
                } = message
                else {
                    continue;
                };
                if sender != DESKTOP_FILE_MANAGER_BUS_NAME
                    || path != DESKTOP_FILE_MANAGER_OBJECT_PATH
                    || interface != DESKTOP_FILE_MANAGER_INTERFACE
                    || signal != DESKTOP_FILE_MANAGER_RESPONSE_SIGNAL
                {
                    // sbusd broadcasts this when a registered service
                    // disconnects. If the file manager vanished while a
                    // picker is open, the response signal will never arrive
                    // — abort the wait so the UI is not stuck forever.
                    if sender == "org.scarlet.sbus" && signal == "ServiceUnregistered" {
                        if let Some(Argument::String(name)) = args.first() {
                            if name == DESKTOP_FILE_MANAGER_BUS_NAME {
                                PICKER_UI_EVENTS.lock().push(PickerUiEvent::ServiceGone);
                            }
                        }
                    }
                    continue;
                }

                let Some(Argument::String(response_id)) = args.first() else {
                    continue;
                };

                let accepted = matches!(args.get(1), Some(Argument::Boolean(true)));
                let path = match args.get(2) {
                    Some(Argument::String(path)) => path.clone(),
                    _ => String::new(),
                };
                PICKER_UI_EVENTS.lock().push(PickerUiEvent::Response {
                    request_id: response_id.clone(),
                    accepted,
                    path,
                });
            }
        }
    });
}

fn start_background_picker_request() {
    if PICKER_REQUEST_IN_FLIGHT
        .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
        .is_err()
    {
        return;
    }

    thread::spawn(move || {
        let mut last_error = String::from("file picker did not respond");

        for attempt in 0..PICKER_REQUEST_ATTEMPTS {
            match request_background_picker() {
                Ok(request_id) => {
                    PICKER_UI_EVENTS
                        .lock()
                        .push(PickerUiEvent::Opened(request_id));
                    return;
                }
                // ServiceNotFound is the only retry-safe failure: sbusd did
                // not deliver the method call, so it cannot create a duplicate
                // picker. A timeout or I/O error is ambiguous because Files may
                // already have accepted the request and only its reply was
                // delayed; retrying that request would open multiple pickers.
                Err(sbus_client::Error::ServiceNotFound) => {
                    last_error = String::from("Files picker service is not available");
                    if attempt == 0
                        && let Err(error) = ensure_file_manager_service()
                    {
                        last_error = format!("failed to start Files: {error:?}");
                    }
                }
                Err(error) => {
                    last_error = format!("{error:?}");
                    break;
                }
            }

            if attempt + 1 < PICKER_REQUEST_ATTEMPTS {
                thread::sleep(Duration::from_millis(100));
            }
        }

        PICKER_UI_EVENTS
            .lock()
            .push(PickerUiEvent::OpenFailed(last_error));
    });
}

fn start_background_autosave(app: &SettingsApp) {
    let style = app.background_style.clone();
    let red = app.red_value.clone();
    let green = app.green_value.clone();
    let blue = app.blue_value.clone();
    let image = app.background_image.clone();

    thread::spawn(move || {
        let mut last = (
            style.get(),
            [
                red.get().max(0.0).min(255.0) as u8,
                green.get().max(0.0).min(255.0) as u8,
                blue.get().max(0.0).min(255.0) as u8,
            ],
            image.get(),
        );

        loop {
            let current = (
                style.get(),
                [
                    red.get().max(0.0).min(255.0) as u8,
                    green.get().max(0.0).min(255.0) as u8,
                    blue.get().max(0.0).min(255.0) as u8,
                ],
                image.get(),
            );
            if current != last {
                if save_background_via_service(current.1, current.0, current.2.as_deref()).is_ok() {
                    last = current;
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
}

fn should_drop_sas_client(error: SasError) -> bool {
    !matches!(error, SasError::ServerError { .. })
}

fn with_sas_client<T>(
    operation: impl FnOnce(&mut SasClient) -> core::result::Result<T, SasError>,
) -> core::result::Result<T, SasError> {
    let mut client = AUDIO_SAS_CLIENT.lock();
    if client.is_none() {
        *client = Some(SasClient::connect()?);
    }

    let result = operation(client.as_mut().unwrap());
    if let Err(error) = result.as_ref()
        && should_drop_sas_client(*error)
    {
        *client = None;
    }
    result
}

fn load_audio_controls() -> (Vec<OutputInfo>, usize, f32, bool, String) {
    let result = with_sas_client(|client| {
        let state = client.control_state()?;

        let current_path = fixed_sas_str(&state.output_path);
        let mut outputs = match client.list_outputs() {
            Ok(outputs) => outputs,
            Err(error) if should_drop_sas_client(error) => return Err(error),
            Err(_) => Vec::new(),
        };
        if outputs.is_empty() {
            outputs.push(OutputInfo::new(
                state.output_kind,
                OUTPUT_ENTRY_FLAG_CURRENT | OUTPUT_ENTRY_FLAG_COMPATIBLE,
                current_path,
                fixed_sas_str(&state.output_name),
                fixed_sas_str(&state.output_description),
            ));
        }

        let selected = outputs
            .iter()
            .position(|output| {
                output.flags & OUTPUT_ENTRY_FLAG_CURRENT != 0
                    || fixed_sas_str(&output.path) == current_path
            })
            .unwrap_or(0);

        Ok((
            outputs,
            selected,
            q16_to_percent(state.master_volume_q16).min(100) as f32,
            state.flags & CONTROL_FLAG_MUTED != 0,
            audio_status_text(state),
        ))
    });

    match result {
        Ok(controls) => controls,
        Err(SasError::ConnectionFailed) => (
            Vec::new(),
            0,
            25.0,
            false,
            String::from("SAS is not running"),
        ),
        Err(error) => (
            Vec::new(),
            0,
            25.0,
            false,
            format!("Failed to query SAS: {}", error.as_str()),
        ),
    }
}

fn audio_output_labels(outputs: &[OutputInfo]) -> Vec<String> {
    outputs.iter().map(output_label_from_info).collect()
}

fn read_limited_text(path: &str) -> core::result::Result<String, &'static str> {
    let mut file = fs::File::open(path).map_err(|_| "failed to open text file")?;
    let mut content = String::new();
    let mut buffer = [0u8; 1024];

    loop {
        let bytes = file
            .read(&mut buffer)
            .map_err(|_| "failed to read text file")?;
        if bytes == 0 {
            break;
        }
        if content.len().saturating_add(bytes) > MAX_SETTINGS_TEXT_BYTES {
            return Err("text file exceeds settings limit");
        }
        let chunk =
            core::str::from_utf8(&buffer[..bytes]).map_err(|_| "text file is not valid UTF-8")?;
        content.push_str(chunk);
    }

    Ok(content)
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..index],
            _ => {}
        }
    }
    line
}

fn toml_section_name(line: &str) -> Option<&str> {
    if line.len() >= 2 && line.starts_with('[') && line.ends_with(']') {
        Some(line[1..line.len() - 1].trim())
    } else {
        None
    }
}

fn toml_assignment_key(line: &str) -> Option<&str> {
    let equals = line.find('=')?;
    Some(line[..equals].trim())
}

fn trim_toml_string(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].trim()
    } else {
        value
    }
}

fn parse_toml_string_setting(content: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for raw_line in content.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if let Some(line_section) = toml_section_name(line) {
            in_section = line_section == section;
            continue;
        }
        if !in_section || toml_assignment_key(line) != Some(key) {
            continue;
        }
        let equals = line.find('=')?;
        let value = trim_toml_string(&line[equals + 1..]);
        if !value.is_empty() {
            return Some(String::from(value));
        }
    }
    None
}

fn update_toml_assignment(content: &str, section: &str, key: &str, value: &str) -> String {
    let assignment = format!("{} = {}", key, value);
    let mut output = String::new();
    let mut in_target_section = false;
    let mut saw_target_section = false;
    let mut wrote_assignment = false;

    for raw_line in content.lines() {
        let logical_line = strip_toml_comment(raw_line).trim();
        if let Some(line_section) = toml_section_name(logical_line) {
            if in_target_section && !wrote_assignment {
                output.push_str(&assignment);
                output.push('\n');
                wrote_assignment = true;
            }
            in_target_section = line_section == section;
            saw_target_section |= in_target_section;
        }

        if in_target_section && toml_assignment_key(logical_line) == Some(key) {
            if !wrote_assignment {
                output.push_str(&assignment);
                output.push('\n');
                wrote_assignment = true;
            }
            continue;
        }

        output.push_str(raw_line);
        output.push('\n');
    }

    if in_target_section && !wrote_assignment {
        output.push_str(&assignment);
        output.push('\n');
        wrote_assignment = true;
    }

    if !saw_target_section {
        if !output.is_empty() && !output.ends_with("\n\n") {
            output.push('\n');
        }
        output.push('[');
        output.push_str(section);
        output.push_str("]\n");
        output.push_str(&assignment);
        output.push('\n');
        wrote_assignment = true;
    }

    debug_assert!(wrote_assignment);
    output
}

fn persist_sws_assignment(
    section: &str,
    key: &str,
    value: &str,
) -> core::result::Result<(), &'static str> {
    let content = read_limited_text(SWS_CONFIG_PATH).unwrap_or_default();
    let updated = update_toml_assignment(&content, section, key, value);
    let _ = fs::create_directory(SWS_CONFIG_DIR);
    let mut file = fs::File::create(SWS_CONFIG_TEMP_PATH)
        .map_err(|_| "failed to create temporary SWS config")?;
    if file.write_all(updated.as_bytes()).is_err() || file.flush().is_err() {
        let _ = fs::remove_file(SWS_CONFIG_TEMP_PATH);
        return Err("failed to write temporary SWS config");
    }
    drop(file);
    if fs::rename(SWS_CONFIG_TEMP_PATH, SWS_CONFIG_PATH).is_err() {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        let mut final_file = match options.open(SWS_CONFIG_PATH) {
            Ok(file) => file,
            Err(_) => {
                let _ = fs::remove_file(SWS_CONFIG_TEMP_PATH);
                return Err("failed to open final SWS config");
            }
        };
        if final_file.write_all(updated.as_bytes()).is_err() || final_file.flush().is_err() {
            let _ = fs::remove_file(SWS_CONFIG_TEMP_PATH);
            return Err("failed to replace SWS config");
        }
        drop(final_file);
        let _ = fs::remove_file(SWS_CONFIG_TEMP_PATH);
    }
    Ok(())
}

fn save_output_scale_config(scale_milli: u32) {
    let value = if scale_milli == 1000 { "1.0" } else { "2.0" };
    match persist_sws_assignment("output", "scale", value) {
        Ok(()) => println!("[settings] Saved output scale: {}", scale_milli),
        Err(error) => println!("[settings] SWS config error: {}", error),
    }
}

fn cursor_theme_name(content: &str, fallback: &str) -> String {
    parse_toml_string_setting(content, "theme", "name").unwrap_or_else(|| String::from(fallback))
}

fn enumerate_cursor_themes() -> Vec<CursorThemeInfo> {
    let mut themes = Vec::new();
    let Ok(entries) = fs::list_directory(CURSOR_THEME_ROOT) else {
        return themes;
    };

    for entry in entries {
        if !entry.is_directory() || entry.name.starts_with('.') {
            continue;
        }
        let path = format!("{}/{}", CURSOR_THEME_ROOT, entry.name);
        let manifest_path = format!("{}/theme.toml", path);
        let Ok(manifest) = read_limited_text(&manifest_path) else {
            continue;
        };
        themes.push(CursorThemeInfo {
            name: cursor_theme_name(&manifest, &entry.name),
            path,
        });
    }

    themes.sort_by(|left, right| left.name.cmp(&right.name));
    themes
}

fn current_cursor_theme_path() -> String {
    read_limited_text(SWS_CONFIG_PATH)
        .ok()
        .and_then(|content| parse_toml_string_setting(&content, "cursor", "theme"))
        .unwrap_or_else(|| String::from(DEFAULT_CURSOR_THEME_PATH))
}

fn load_cursor_theme_choices() -> (Vec<CursorThemeInfo>, usize) {
    let themes = enumerate_cursor_themes();
    let current_path = current_cursor_theme_path();
    let selected = themes
        .iter()
        .position(|theme| theme.path == current_path)
        .unwrap_or(0);
    (themes, selected)
}

fn cursor_theme_error_message(error: SwsError) -> &'static str {
    match error {
        SwsError::ServerError(sws_protocol::error_codes::INVALID_CURSOR_THEME) => {
            "theme is invalid or unreadable"
        }
        SwsError::ServerError(sws_protocol::error_codes::CURSOR_THEME_PERSIST_FAILED) => {
            "could not save the SWS configuration"
        }
        _ => error.as_str(),
    }
}

fn enumerate_regions() -> Vec<String> {
    let mut regions = Vec::new();
    if let Ok(entries) = fs::list_directory("/usr/share/zoneinfo") {
        for entry in &entries {
            let name = entry.name.as_str();
            if name.starts_with('.') || name.starts_with('+') {
                continue;
            }
            if name == "posix" || name == "right" || name == "tab" {
                continue;
            }
            if entry.is_directory() {
                regions.push(String::from(name));
            }
        }
    }
    regions.sort();
    regions
}

fn enumerate_cities(region: &str) -> Vec<String> {
    let path = format!("/usr/share/zoneinfo/{}", region);
    let mut cities = Vec::new();
    if let Ok(entries) = fs::list_directory(&path) {
        for entry in &entries {
            let name = entry.name.as_str();
            if name.starts_with('.') {
                continue;
            }
            if entry.is_file() {
                cities.push(String::from(name));
            }
        }
    }
    cities.sort();
    cities
}

fn read_current_timezone() -> String {
    match fs::File::open("/etc/timezone") {
        Ok(mut file) => {
            let mut buf = [0u8; 128];
            match file.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let s = core::str::from_utf8(&buf[..n]).unwrap_or("UTC");
                    let trimmed = s.trim();
                    if trimmed.is_empty() {
                        String::from("UTC")
                    } else {
                        String::from(trimmed)
                    }
                }
                _ => String::from("UTC"),
            }
        }
        Err(_) => String::from("UTC"),
    }
}

fn save_timezone(zone: &str) {
    let target = format!("/usr/share/zoneinfo/{}", zone);
    let _ = fs::remove_file("/etc/localtime");
    match fs::create_symlink("/etc/localtime", &target) {
        Ok(()) => {
            if let Ok(mut file) = fs::File::create("/etc/timezone") {
                let _ = file.write(zone.as_bytes());
            }
            println!("[settings] Timezone set to {}", zone);
        }
        Err(e) => println!("[settings] Failed to set timezone: {:?}", e),
    }
}

#[derive(View, Clone)]
struct SettingsApp {
    background_style: State<BackgroundStyle>,
    background_image: State<Option<String>>,
    background_image_label: State<String>,
    picker_request_id: State<Option<String>>,
    red_value: State<f32>,
    green_value: State<f32>,
    blue_value: State<f32>,
    input_methods: State<Vec<InputMethodInfo>>,
    selected_ime_index: State<usize>,
    timezone_regions: State<Vec<String>>,
    timezone_region_index: State<usize>,
    timezone_cities: State<Vec<String>>,
    timezone_city_index: State<usize>,
    audio_outputs: State<Vec<OutputInfo>>,
    audio_output_index: State<usize>,
    audio_volume_percent: State<f32>,
    audio_muted: State<bool>,
    audio_status: State<String>,
    display_brightness_percent: State<f32>,
    display_brightness_last_good: State<f32>,
    display_brightness_status: State<String>,
    navigation_title: State<String>,
    cursor_themes: State<Vec<CursorThemeInfo>>,
    cursor_theme_index: State<usize>,
    cursor_theme_status: State<String>,
    status_preferences: State<StatusPreferences>,
    status_preferences_status: State<String>,
}

impl SettingsApp {
    pub fn new() -> Self {
        let config = scarlet_desktop_config::load_desktop_config();
        let style = config.theme.background_style.unwrap_or(DEFAULT_STYLE);
        let color = config.theme.background.unwrap_or(DEFAULT_BG_PREVIEW);
        let background_image = config.theme.background_image.clone();
        let background_image_label = background_image
            .clone()
            .unwrap_or_else(|| String::from("No image selected (using generated background)"));
        let input_methods = load_input_methods();
        let selected_ime_index = input_methods
            .iter()
            .position(|method| method.active)
            .unwrap_or(0);
        let timezone_regions = enumerate_regions();
        let current_tz = read_current_timezone();
        let (cur_region, cur_city) = current_tz.split_once('/').unwrap_or(("Etc", "UTC"));
        let region_index = timezone_regions
            .iter()
            .position(|r| r == cur_region)
            .unwrap_or(0);
        let timezone_cities = enumerate_cities(
            timezone_regions
                .get(region_index)
                .map(|s| s.as_str())
                .unwrap_or("Etc"),
        );
        let city_index = timezone_cities
            .iter()
            .position(|c| c == cur_city)
            .unwrap_or(0);
        let (audio_outputs, audio_output_index, audio_volume, audio_muted, audio_status) =
            load_audio_controls();
        let (display_brightness, display_brightness_status) = load_display_brightness();
        let (cursor_themes, cursor_theme_index) = load_cursor_theme_choices();
        let cursor_theme_status = cursor_themes
            .get(cursor_theme_index)
            .map(|theme| format!("Active: {}", theme.name))
            .unwrap_or_else(|| String::from("No cursor themes installed"));
        let (status_preferences, status_preferences_status) = load_status_preferences();
        let app = Self {
            background_style: State::new(StateId::new(0), style),
            background_image: State::new(StateId::new(1), background_image),
            background_image_label: State::new(StateId::new(2), background_image_label),
            picker_request_id: State::new(StateId::new(3), None),
            red_value: State::new(StateId::new(4), color[0] as f32),
            green_value: State::new(StateId::new(5), color[1] as f32),
            blue_value: State::new(StateId::new(6), color[2] as f32),
            input_methods: State::new(StateId::new(7), input_methods),
            selected_ime_index: State::new(StateId::new(8), selected_ime_index),
            timezone_regions: State::new(StateId::new(9), timezone_regions),
            timezone_region_index: State::new(StateId::new(10), region_index),
            timezone_cities: State::new(StateId::new(11), timezone_cities),
            timezone_city_index: State::new(StateId::new(12), city_index),
            audio_outputs: State::new(StateId::new(13), audio_outputs),
            audio_output_index: State::new(StateId::new(14), audio_output_index),
            audio_volume_percent: State::new(StateId::new(15), audio_volume),
            audio_muted: State::new(StateId::new(16), audio_muted),
            audio_status: State::new(StateId::new(17), audio_status),
            display_brightness_percent: State::new(StateId::new(18), display_brightness),
            display_brightness_last_good: State::new(StateId::new(19), display_brightness),
            display_brightness_status: State::new(StateId::new(20), display_brightness_status),
            navigation_title: State::new(StateId::new(21), String::from("Appearance")),
            cursor_themes: State::new(StateId::new(22), cursor_themes),
            cursor_theme_index: State::new(StateId::new(23), cursor_theme_index),
            cursor_theme_status: State::new(StateId::new(24), cursor_theme_status),
            status_preferences: State::new(StateId::new(25), status_preferences),
            status_preferences_status: State::new(StateId::new(26), status_preferences_status),
        };
        start_picker_response_listener();
        start_background_autosave(&app);
        app
    }

    fn save_config(&self) {
        let color = self.current_color();
        let style = self.background_style.get();
        let image = self.background_image.get();
        if let Err(error) = save_background_via_service(color, style, image.as_deref()) {
            println!(
                "[settings] failed to save background through settingsd: {:?}",
                error
            );
        }
    }

    fn refresh_status_preferences(&self) {
        match get_status_preferences_via_service() {
            Ok(preferences) => {
                self.status_preferences.set(preferences);
                self.status_preferences_status.set(String::from(
                    "Loaded authoritative preferences from settingsd",
                ));
            }
            Err(error) => {
                self.status_preferences_status
                    .set(format!("Failed to refresh status preferences: {error}"));
                println!(
                    "[settings] failed to refresh status preferences through settingsd: {}",
                    error
                );
            }
        }
    }

    fn save_status_preferences(&self, requested: StatusPreferences, action: &'static str) {
        match save_status_preferences_via_service(&requested) {
            Ok(saved) => {
                self.status_preferences.set(saved);
                self.status_preferences_status
                    .set(format!("{action} saved through settingsd"));
            }
            Err(error) => {
                self.status_preferences_status
                    .set(format!("Failed to save {action}: {error}"));
                println!(
                    "[settings] failed to save status preferences through settingsd: {}",
                    error
                );
            }
        }
    }

    fn set_status_item_visible(&self, item: StatusItemId, visible: bool) {
        let current = self.status_preferences.get();
        if current.is_visible(item) == visible {
            return;
        }
        self.save_status_preferences(
            set_status_item_visibility(&current, item, visible),
            "status item visibility",
        );
    }

    fn move_status_item(&self, item: StatusItemId, offset: isize) {
        let current = self.status_preferences.get();
        if let Some(updated) = move_status_item(&current, item, offset) {
            self.save_status_preferences(updated, "status item order");
        }
    }

    fn set_status_clock_format(&self, clock_format: ClockFormat) {
        let mut requested = self.status_preferences.get();
        if requested.clock_format == clock_format {
            return;
        }
        requested.clock_format = clock_format;
        self.save_status_preferences(requested, "clock format");
    }

    fn reset_status_preferences(&self) {
        match reset_status_preferences_via_service() {
            Ok(preferences) => {
                self.status_preferences.set(preferences);
                self.status_preferences_status
                    .set(String::from("Status preferences reset through settingsd"));
            }
            Err(error) => {
                self.status_preferences_status
                    .set(format!("Failed to reset status preferences: {error}"));
                println!(
                    "[settings] failed to reset status preferences through settingsd: {}",
                    error
                );
            }
        }
    }

    fn current_color(&self) -> [u8; 3] {
        let r = self.red_value.get().max(0.0).min(255.0) as u8;
        let g = self.green_value.get().max(0.0).min(255.0) as u8;
        let b = self.blue_value.get().max(0.0).min(255.0) as u8;
        [r, g, b]
    }

    fn selected_preset_index(&self) -> Option<usize> {
        let current = self.current_color();
        for (i, preset) in PRESET_COLORS.iter().enumerate() {
            if preset.color == current {
                return Some(i);
            }
        }
        None
    }

    fn clear_background_image(&self) {
        self.background_image.set(None);
        self.background_image_label.set(String::from(
            "No image selected (using generated background)",
        ));
        self.save_config();
    }

    fn open_background_picker(&self) {
        start_background_picker_request();
    }

    fn process_picker_ui_events(&self) {
        let events = {
            let mut pending = PICKER_UI_EVENTS.lock();
            core::mem::take(&mut *pending)
        };

        for event in events {
            match event {
                PickerUiEvent::Opened(request_id) => {
                    self.picker_request_id.set(Some(request_id));
                    println!("[settings] file picker opened");
                }
                PickerUiEvent::OpenFailed(error) => {
                    PICKER_REQUEST_IN_FLIGHT.store(false, AtomicOrdering::Release);
                    println!("[settings] failed to open file picker: {error}");
                }
                PickerUiEvent::ServiceGone => {
                    PICKER_REQUEST_IN_FLIGHT.store(false, AtomicOrdering::Release);
                    self.picker_request_id.set(None);
                    println!("[settings] file manager service gone, cancelling picker");
                }
                PickerUiEvent::Response {
                    request_id,
                    accepted,
                    path,
                } => {
                    let active_request_id = self.picker_request_id.get();
                    if active_request_id.as_deref() != Some(request_id.as_str()) {
                        // Files can emit the signal immediately after replying
                        // to OpenFile. If the listener wins that race, retain
                        // the response until the UI has applied Opened and
                        // knows which request it belongs to.
                        if active_request_id.is_none()
                            && PICKER_REQUEST_IN_FLIGHT.load(AtomicOrdering::Acquire)
                        {
                            PICKER_UI_EVENTS.lock().push(PickerUiEvent::Response {
                                request_id,
                                accepted,
                                path,
                            });
                        }
                        continue;
                    }

                    PICKER_REQUEST_IN_FLIGHT.store(false, AtomicOrdering::Release);
                    self.picker_request_id.set(None);
                    if accepted && !path.is_empty() {
                        self.background_image.set(Some(path.clone()));
                        self.background_image_label.set(path);
                        self.save_config();
                    }
                }
            }
        }
    }

    fn select_input_method(&self, index: usize) {
        let methods = self.input_methods.get();
        let Some(method) = methods.get(index) else {
            println!("[settings] Invalid input method index: {}", index);
            return;
        };

        match Connection::connect_default() {
            Ok(connection) => match connection.set_active_input_method(method.ime_id) {
                Ok(()) => {
                    println!(
                        "[settings] Selected input method: {} ({})",
                        method.name, method.ime_id
                    );
                    self.selected_ime_index.set(index);
                    self.input_methods.update(|input_methods| {
                        for item in input_methods {
                            item.active = item.ime_id == method.ime_id;
                        }
                    });
                }
                Err(e) => println!("[settings] Failed to select input method: {:?}", e),
            },
            Err(e) => println!("[settings] Failed to connect to SWS: {:?}", e),
        }
    }

    fn apply_audio_state(&self, state: ControlState) {
        self.audio_volume_percent
            .set(q16_to_percent(state.master_volume_q16).min(100) as f32);
        self.audio_muted.set(state.flags & CONTROL_FLAG_MUTED != 0);
        self.audio_status.set(audio_status_text(state));
    }

    fn refresh_audio(&self) {
        let (outputs, output_index, volume, muted, status) = load_audio_controls();
        self.audio_outputs.set(outputs);
        self.audio_output_index.set(output_index);
        self.audio_volume_percent.set(volume);
        self.audio_muted.set(muted);
        self.audio_status.set(status);
    }

    fn set_audio_muted(&self, muted: bool) {
        match with_sas_client(|client| client.set_master_muted(muted)) {
            Ok(state) => self.apply_audio_state(state),
            Err(error) => self
                .audio_status
                .set(format!("Failed to set mute: {}", error.as_str())),
        }
    }

    fn set_audio_volume_percent(&self, percent: u32) {
        match with_sas_client(|client| client.set_master_volume_q16(percent_to_q16(percent))) {
            Ok(state) => self.apply_audio_state(state),
            Err(error) => self
                .audio_status
                .set(format!("Failed to set volume: {}", error.as_str())),
        }
    }

    fn set_display_brightness_percent(&self, percent: u32) {
        let percent = percent.min(100) as u8;
        let last_good = self.display_brightness_last_good.get();
        if f32_to_percent(last_good) == percent as u32 {
            self.display_brightness_percent.set(last_good);
            return;
        }

        match DisplayControl::open_primary()
            .and_then(|display| display.set_brightness_percent(percent))
        {
            Ok(()) => {
                let applied = percent as f32;
                self.display_brightness_percent.set(applied);
                self.display_brightness_last_good.set(applied);
                self.display_brightness_status
                    .set(format!("Current brightness: {}%", percent));
                println!("[settings] display brightness set to {}%", percent);
            }
            Err(error) => {
                self.display_brightness_percent.set(last_good);
                self.display_brightness_status.set(format!(
                    "Failed to set brightness; keeping {}% ({:?})",
                    f32_to_percent(last_good),
                    error
                ));
                println!("[settings] failed to set display brightness: {:?}", error);
            }
        }
    }

    fn select_audio_output(&self, index: usize) {
        let outputs = self.audio_outputs.get();
        let Some(output) = outputs.get(index) else {
            self.audio_status
                .set(format!("Invalid audio output index: {}", index));
            return;
        };
        let path = fixed_sas_str(&output.path);
        let Some(request) = OutputRequest::new(OUTPUT_PREFERENCE_PATH, path) else {
            self.audio_status
                .set(String::from("Audio output path is too long"));
            return;
        };

        match with_sas_client(|client| client.set_output(request)) {
            Ok(state) => {
                self.audio_output_index.set(index);
                self.apply_audio_state(state);
                self.refresh_audio();
            }
            Err(error) => self
                .audio_status
                .set(format!("Failed to switch output: {}", error.as_str())),
        }
    }

    fn refresh_cursor_themes(&self) {
        let (themes, selected) = load_cursor_theme_choices();
        let status = themes
            .get(selected)
            .map(|theme| format!("Active: {}", theme.name))
            .unwrap_or_else(|| String::from("No cursor themes installed"));
        self.cursor_themes.set(themes);
        self.cursor_theme_index.set(selected);
        self.cursor_theme_status.set(status);
    }

    fn select_cursor_theme(&self, index: usize) {
        let themes = self.cursor_themes.get();
        let Some(theme) = themes.get(index) else {
            self.cursor_theme_status
                .set(format!("Invalid cursor theme index: {}", index));
            return;
        };
        let name = theme.name.clone();
        let path = theme.path.clone();

        match Connection::connect_default()
            .and_then(|connection| connection.set_cursor_theme(&path))
        {
            Ok(()) => {
                self.cursor_theme_index.set(index);
                self.cursor_theme_status.set(format!("Active: {}", name));
                println!("[settings] Cursor theme set to {} ({})", name, path);
            }
            Err(error) => {
                self.refresh_cursor_themes();
                self.cursor_theme_status.set(format!(
                    "Failed to apply {}: {}",
                    name,
                    cursor_theme_error_message(error)
                ));
                println!(
                    "[settings] Failed to set cursor theme {} ({}): {:?}",
                    name, path, error
                );
            }
        }
    }
}

fn load_input_methods() -> Vec<InputMethodInfo> {
    match Connection::connect_default() {
        Ok(connection) => match connection.get_input_methods() {
            Ok(methods) => methods,
            Err(e) => {
                println!("[settings] Failed to get input methods: {:?}", e);
                Vec::new()
            }
        },
        Err(e) => {
            println!("[settings] Failed to connect to SWS: {:?}", e);
            Vec::new()
        }
    }
}

fn input_method_labels(methods: &[InputMethodInfo]) -> Vec<String> {
    methods.iter().map(|method| method.name.clone()).collect()
}

fn appearance_page(
    r0: State<f32>,
    g0: State<f32>,
    b0: State<f32>,
    r1: State<f32>,
    g1: State<f32>,
    b1: State<f32>,
    r2: State<f32>,
    g2: State<f32>,
    b2: State<f32>,
    r3: State<f32>,
    g3: State<f32>,
    b3: State<f32>,
    r4: State<f32>,
    g4: State<f32>,
    b4: State<f32>,
    r5: State<f32>,
    g5: State<f32>,
    b5: State<f32>,
    r6: State<f32>,
    g6: State<f32>,
    b6: State<f32>,
    r7: State<f32>,
    g7: State<f32>,
    b7: State<f32>,
    s0: State<BackgroundStyle>,
    s1: State<BackgroundStyle>,
    s2: State<BackgroundStyle>,
    is0: bool,
    is1: bool,
    is2: bool,
    is3: bool,
    is4: bool,
    is5: bool,
    is6: bool,
    is7: bool,
    style_default: bool,
    style_gradient: bool,
    style_solid: bool,
    highlight: Color,
    _border: Color,
    app: SettingsApp,
) -> impl View {
    vstack! {
        hstack! {
            vstack! {
                Text::new("Style").font_size(14.0),
                zstack! {
                    Rectangle::new()
                        .fill(Color::rgb(48, 48, 56))
                        .frame(190.0, 60.0),
                    zstack! {
                        Rectangle::new()
                            .fill(Color::CLEAR)
                            .border(2.0, if style_default { highlight } else { Color::CLEAR })
                            .frame(198.0, 68.0),
                        Text::new("Gradient + Lines").font_size(12.0).color(Color::WHITE),
                    },
                }
                .on_click(move || { s0.set(BackgroundStyle::GradientLines); }),
                zstack! {
                    Rectangle::new()
                        .fill(Color::rgb(40, 40, 50))
                        .frame(190.0, 60.0),
                    zstack! {
                        Rectangle::new()
                            .fill(Color::CLEAR)
                            .border(2.0, if style_gradient { highlight } else { Color::CLEAR })
                            .frame(198.0, 68.0),
                        Text::new("Gradient").font_size(12.0).color(Color::WHITE),
                    },
                }
                .on_click(move || { s1.set(BackgroundStyle::Gradient); }),
                zstack! {
                    Rectangle::new()
                        .fill(Color::rgb(26, 26, 30))
                        .frame(190.0, 60.0),
                    zstack! {
                        Rectangle::new()
                            .fill(Color::CLEAR)
                            .border(2.0, if style_solid { highlight } else { Color::CLEAR })
                            .frame(198.0, 68.0),
                        Text::new("Solid").font_size(12.0).color(Color::WHITE),
                    },
                }
                .on_click(move || { s2.set(BackgroundStyle::Solid); }),
            }
            .frame(220.0, f32::INFINITY),
            vstack! {
                Text::new("Color").font_size(14.0),
                hstack! {
                    zstack! {
                        Rectangle::new()
                            .fill(Color::rgb(PRESET_COLORS[0].color[0], PRESET_COLORS[0].color[1], PRESET_COLORS[0].color[2]))
                            .frame(80.0, 80.0),
                        Rectangle::new()
                            .fill(Color::CLEAR)
                            .border(2.0, if is0 { highlight } else { Color::CLEAR })
                            .frame(88.0, 88.0),
                    }
                    .alignment(Alignment::Center)
                    .on_click(move || {
                        r0.set(PRESET_COLORS[0].color[0] as f32);
                        g0.set(PRESET_COLORS[0].color[1] as f32);
                        b0.set(PRESET_COLORS[0].color[2] as f32);
                    }),
                    Spacer::new().frame_width(10.0),
                    zstack! {
                        Rectangle::new()
                            .fill(Color::rgb(PRESET_COLORS[1].color[0], PRESET_COLORS[1].color[1], PRESET_COLORS[1].color[2]))
                            .frame(80.0, 80.0),
                        Rectangle::new()
                            .fill(Color::CLEAR)
                            .border(2.0, if is1 { highlight } else { Color::CLEAR })
                            .frame(88.0, 88.0),
                    }
                    .alignment(Alignment::Center)
                    .on_click(move || {
                        r1.set(PRESET_COLORS[1].color[0] as f32);
                        g1.set(PRESET_COLORS[1].color[1] as f32);
                        b1.set(PRESET_COLORS[1].color[2] as f32);
                    }),
                    Spacer::new().frame_width(10.0),
                    zstack! {
                        Rectangle::new()
                            .fill(Color::rgb(PRESET_COLORS[2].color[0], PRESET_COLORS[2].color[1], PRESET_COLORS[2].color[2]))
                            .frame(80.0, 80.0),
                        Rectangle::new()
                            .fill(Color::CLEAR)
                            .border(2.0, if is2 { highlight } else { Color::CLEAR })
                            .frame(88.0, 88.0),
                    }
                    .alignment(Alignment::Center)
                    .on_click(move || {
                        r2.set(PRESET_COLORS[2].color[0] as f32);
                        g2.set(PRESET_COLORS[2].color[1] as f32);
                        b2.set(PRESET_COLORS[2].color[2] as f32);
                    }),
                    Spacer::new().frame_width(10.0),
                    zstack! {
                        Rectangle::new()
                            .fill(Color::rgb(PRESET_COLORS[3].color[0], PRESET_COLORS[3].color[1], PRESET_COLORS[3].color[2]))
                            .frame(80.0, 80.0),
                        Rectangle::new()
                            .fill(Color::CLEAR)
                            .border(2.0, if is3 { highlight } else { Color::CLEAR })
                            .frame(88.0, 88.0),
                    }
                    .alignment(Alignment::Center)
                    .on_click(move || {
                        r3.set(PRESET_COLORS[3].color[0] as f32);
                        g3.set(PRESET_COLORS[3].color[1] as f32);
                        b3.set(PRESET_COLORS[3].color[2] as f32);
                    }),
                },
                Spacer::new().frame_height(10.0),
                hstack! {
                    zstack! {
                        Rectangle::new()
                            .fill(Color::rgb(PRESET_COLORS[4].color[0], PRESET_COLORS[4].color[1], PRESET_COLORS[4].color[2]))
                            .frame(80.0, 80.0),
                        Rectangle::new()
                            .fill(Color::CLEAR)
                            .border(2.0, if is4 { highlight } else { Color::CLEAR })
                            .frame(88.0, 88.0),
                    }
                    .alignment(Alignment::Center)
                    .on_click(move || {
                        r4.set(PRESET_COLORS[4].color[0] as f32);
                        g4.set(PRESET_COLORS[4].color[1] as f32);
                        b4.set(PRESET_COLORS[4].color[2] as f32);
                    }),
                    Spacer::new().frame_width(10.0),
                    zstack! {
                        Rectangle::new()
                            .fill(Color::rgb(PRESET_COLORS[5].color[0], PRESET_COLORS[5].color[1], PRESET_COLORS[5].color[2]))
                            .frame(80.0, 80.0),
                        Rectangle::new()
                            .fill(Color::CLEAR)
                            .border(2.0, if is5 { highlight } else { Color::CLEAR })
                            .frame(88.0, 88.0),
                    }
                    .alignment(Alignment::Center)
                    .on_click(move || {
                        r5.set(PRESET_COLORS[5].color[0] as f32);
                        g5.set(PRESET_COLORS[5].color[1] as f32);
                        b5.set(PRESET_COLORS[5].color[2] as f32);
                    }),
                    Spacer::new().frame_width(10.0),
                    zstack! {
                        Rectangle::new()
                            .fill(Color::rgb(PRESET_COLORS[6].color[0], PRESET_COLORS[6].color[1], PRESET_COLORS[6].color[2]))
                            .frame(80.0, 80.0),
                        Rectangle::new()
                            .fill(Color::CLEAR)
                            .border(2.0, if is6 { highlight } else { Color::CLEAR })
                            .frame(88.0, 88.0),
                    }
                    .alignment(Alignment::Center)
                    .on_click(move || {
                        r6.set(PRESET_COLORS[6].color[0] as f32);
                        g6.set(PRESET_COLORS[6].color[1] as f32);
                        b6.set(PRESET_COLORS[6].color[2] as f32);
                    }),
                    Spacer::new().frame_width(10.0),
                    zstack! {
                        Rectangle::new()
                            .fill(Color::rgb(PRESET_COLORS[7].color[0], PRESET_COLORS[7].color[1], PRESET_COLORS[7].color[2]))
                            .frame(80.0, 80.0),
                        Rectangle::new()
                            .fill(Color::CLEAR)
                            .border(2.0, if is7 { highlight } else { Color::CLEAR })
                            .frame(88.0, 88.0),
                    }
                    .alignment(Alignment::Center)
                    .on_click(move || {
                        r7.set(PRESET_COLORS[7].color[0] as f32);
                        g7.set(PRESET_COLORS[7].color[1] as f32);
                        b7.set(PRESET_COLORS[7].color[2] as f32);
                    }),
                },
            },
        }
        .padding(10.0),

        Divider::new(),

        vstack! {
            Text::new("Custom Color").font_size(14.0),
            hstack! {
                Text::new("R").font_size(12.0).frame_width(20.0),
                Slider::new(app.red_value.clone()).min(0.0).max(255.0).frame(280.0, 20.0),
                Text::new(format!("{}", app.red_value.get().max(0.0).min(255.0) as u32)).font_size(12.0).frame_width(36.0),
            },
            hstack! {
                Text::new("G").font_size(12.0).frame_width(20.0),
                Slider::new(app.green_value.clone()).min(0.0).max(255.0).frame(280.0, 20.0),
                Text::new(format!("{}", app.green_value.get().max(0.0).min(255.0) as u32)).font_size(12.0).frame_width(36.0),
            },
            hstack! {
                Text::new("B").font_size(12.0).frame_width(20.0),
                Slider::new(app.blue_value.clone()).min(0.0).max(255.0).frame(280.0, 20.0),
                Text::new(format!("{}", app.blue_value.get().max(0.0).min(255.0) as u32)).font_size(12.0).frame_width(36.0),
            },
            Text::new("Preview").font_size(14.0),
        Rectangle::new()
                .fill(Color::rgb(
                    app.current_color()[0],
                    app.current_color()[1],
                    app.current_color()[2],
                ))
                .frame(520.0, 70.0)
                .clip_radius(10.0),
        },

        vstack! {
            Text::new("Wallpaper Image").font_size(14.0),
            hstack! {
                IconView::new(Icon::Photo).size(IconSize::Large),
                Text::from_state(app.background_image_label.clone()).font_size(12.0),
                Spacer::new(),
                Button::new("Choose…").on_click({
                    let app = app.clone();
                    move || app.open_background_picker()
                }),
                Button::new("Clear Image").on_click({
                    let app = app.clone();
                    move || app.clear_background_image()
                }),
            }
            .padding(8.0),
            Text::new("The standalone File Manager will open in image-picker mode.")
                .font_size(11.0)
                .color(ColorPalette::default().text_secondary()),
        },

        Divider::new(),

        hstack! {
            Spacer::new(),
            Button::new("Apply").on_click({
                let app = app.clone();
                move || { app.save_config(); println!("[settings] Applied"); }
            }),
            Spacer::new().frame_width(12.0),
            Button::new("Close").on_click(|| { println!("[settings] Close"); }),
        }.padding(10.0)
    }
    .padding(10.0)
    .frame(f32::INFINITY, f32::INFINITY)
}

#[allow(dead_code)]
fn about_page() -> impl View {
    vstack! {
        vstack! {
            Text::new("Scarlet Desktop").font_size(20.0),
            Text::new("Version 0.1.0").font_size(14.0),
            Text::new("").font_size(10.0),
            Text::new("A modern desktop environment for Scarlet OS").font_size(13.0),
            Text::new("Built with Rust and ScarletUI").font_size(13.0),
        }
        .padding(20.0),

        Divider::new(),

        vstack! {
            Text::new("License").font_size(16.0),
            Text::new("MIT License").font_size(13.0),
            Text::new("").font_size(10.0),
            Text::new("Copyright (c) 2025 Scarlet OS Project").font_size(13.0),
        }
        .padding(20.0),
    }
    .padding(10.0)
    .frame(f32::INFINITY, f32::INFINITY)
}

#[allow(dead_code)]
fn network_page() -> impl View {
    vstack! {
        vstack! {
            Text::new("Coming Soon").font_size(20.0),
            Text::new("Network configuration will be available here").font_size(13.0),
        }
        .padding(40.0),
    }
    .padding(10.0)
    .frame(f32::INFINITY, f32::INFINITY)
}

fn display_page(app: SettingsApp) -> impl View {
    let brightness = app.display_brightness_percent.clone();
    let brightness_label = f32_to_percent(app.display_brightness_percent.get());
    let brightness_status = app.display_brightness_status.get();
    let brightness_app = app.clone();

    vstack! {
        vstack! {
            Text::new("Brightness").font_size(14.0),
            hstack! {
                Text::new("Brightness").font_size(13.0).frame_width(100.0),
                Slider::new(brightness)
                    .min(0.0)
                    .max(100.0)
                    .on_changed(move |value| {
                        brightness_app.set_display_brightness_percent(f32_to_percent(value));
                    })
                    .frame(300.0, 20.0),
                Text::new(format!("{}%", brightness_label))
                    .font_size(12.0)
                    .frame_width(48.0),
            }
            .padding(10.0),
            Text::new(brightness_status).font_size(12.0),
        }
        .padding(10.0),

        Divider::new(),

        vstack! {
            Text::new("Scaling").font_size(14.0),
            hstack! {
                Text::new("Scale").font_size(13.0).frame_width(80.0),
                Button::new("x1.0").on_click(|| { save_output_scale_config(1000); }),
                Spacer::new().frame_width(8.0),
                Button::new("x2.0").on_click(|| { save_output_scale_config(2000); }),
            }
            .padding(10.0),
        }
        .padding(10.0),
    }
    .padding(10.0)
    .frame(f32::INFINITY, f32::INFINITY)
}

fn cursor_page(app: SettingsApp) -> impl View {
    let themes = app.cursor_themes.get();
    let labels = themes
        .iter()
        .map(|theme| theme.name.clone())
        .collect::<Vec<_>>();
    let selected = app.cursor_theme_index.clone();
    let status = app.cursor_theme_status.get();
    let select_app = app.clone();
    let refresh_app = app.clone();

    vstack! {
        vstack! {
            Text::new("Cursor Theme").font_size(14.0),
            hstack! {
                Text::new("Theme").font_size(13.0).frame_width(100.0),
                Select::new(labels, selected)
                    .width(380.0)
                    .placeholder("No installed cursor themes")
                    .on_change(move |index| {
                        select_app.select_cursor_theme(index);
                    }),
                Spacer::new().frame_width(8.0),
                Button::new("Refresh").on_click(move || {
                    refresh_app.refresh_cursor_themes();
                }),
            }
            .padding(10.0),
            Text::new(status).font_size(12.0),
            Text::new("Themes are discovered from /share/cursors and applied immediately.")
                .font_size(11.0)
                .color(ColorPalette::default().text_secondary()),
        }
        .padding(10.0),
    }
    .padding(10.0)
    .frame(f32::INFINITY, f32::INFINITY)
}

fn audio_page(app: SettingsApp) -> impl View {
    let outputs = app.audio_outputs.get();
    let labels = audio_output_labels(&outputs);
    let selected_output = app.audio_output_index.clone();
    let volume = app.audio_volume_percent.clone();
    let volume_label = f32_to_percent(app.audio_volume_percent.get());
    let status = app.audio_status.get();
    let muted = app.audio_muted.get();
    let select_app = app.clone();
    let refresh_app = app.clone();
    let volume_app = app.clone();
    let mute_app = app.clone();

    vstack! {
        vstack! {
            Text::new("Output Device").font_size(14.0),
            hstack! {
                Text::new("Output").font_size(13.0).frame_width(100.0),
                Select::new(labels, selected_output)
                    .width(380.0)
                    .placeholder("No SAS output")
                    .on_change(move |index| {
                        select_app.select_audio_output(index);
                    }),
                Spacer::new().frame_width(8.0),
                Button::new("Refresh").on_click(move || {
                    refresh_app.refresh_audio();
                }),
            }
            .padding(10.0),
            Text::new(status).font_size(12.0),
        }
        .padding(10.0),

        Divider::new(),

        vstack! {
            Text::new("Master Volume").font_size(14.0),
            hstack! {
                Text::new("Volume").font_size(13.0).frame_width(100.0),
                Slider::new(volume)
                    .min(0.0)
                    .max(100.0)
                    .on_changed(move |value| {
                        volume_app.set_audio_volume_percent(f32_to_percent(value));
                    })
                    .frame(300.0, 20.0),
                Text::new(format!("{}%", volume_label)).font_size(12.0).frame_width(48.0),
            }
            .padding(10.0),
            hstack! {
                Text::new("Mute").font_size(13.0).frame_width(100.0),
                Button::new(if muted { "Unmute" } else { "Mute" }).on_click(move || {
                    mute_app.set_audio_muted(!muted);
                }),
            }
            .padding(10.0),
        }
        .padding(10.0),
    }
    .padding(10.0)
    .frame(f32::INFINITY, f32::INFINITY)
}

fn input_page(app: SettingsApp) -> impl View {
    let methods = app.input_methods.get();
    let labels = input_method_labels(&methods);
    let selected = app.selected_ime_index.clone();
    let selected_method = methods.get(selected.get());
    let selected_name = selected_method
        .map(|method| method.name.clone())
        .unwrap_or_else(|| String::from("None"));
    let selected_id = selected_method.map(|method| method.ime_id).unwrap_or(0);

    vstack! {
        vstack! {
            Text::new("IME").font_size(14.0),
            hstack! {
                Text::new("Input Method").font_size(13.0).frame_width(120.0),
                Select::new(labels, selected)
                    .width(300.0)
                    .on_change({
                        let app = app.clone();
                        move |index| {
                            app.select_input_method(index);
                        }
                    }),
            }
            .padding(10.0),
            Text::new(format!("Active: {} ({})", selected_name, selected_id)).font_size(12.0),
            Text::new(format!("Registered IMEs: {}", methods.len())).font_size(12.0),
        }
        .padding(10.0),
    }
    .padding(10.0)
    .frame(f32::INFINITY, f32::INFINITY)
}

fn status_item_label(item: StatusItemId) -> &'static str {
    match item {
        StatusItemId::Cpu => "CPU",
        StatusItemId::Audio => "Audio",
    }
}

fn status_item_row(
    app: SettingsApp,
    item: StatusItemId,
    index: usize,
    item_count: usize,
) -> impl View + Clone {
    let preferences = app.status_preferences.get();
    let visible = preferences.is_visible(item);
    let visibility_app = app.clone();
    let up_app = app.clone();
    let down_app = app;

    hstack! {
        Text::new(status_item_label(item)).font_size(13.0).frame_width(110.0),
        Text::new(if visible { "Visible" } else { "Hidden" })
            .font_size(12.0)
            .frame_width(70.0),
        Button::new(if visible { "Hide" } else { "Show" }).on_click(move || {
            visibility_app.set_status_item_visible(item, !visible);
        }),
        Spacer::new().frame_width(8.0),
        Button::new("Move Up").on_click(move || {
            if index > 0 {
                up_app.move_status_item(item, -1);
            }
        }),
        Spacer::new().frame_width(8.0),
        Button::new("Move Down").on_click(move || {
            if index + 1 < item_count {
                down_app.move_status_item(item, 1);
            }
        }),
    }
    .padding(8.0)
}

fn status_page(
    app: SettingsApp,
    regions: Vec<String>,
    region_idx: State<usize>,
    cities_state: State<Vec<String>>,
    city_idx: State<usize>,
) -> impl View {
    let preferences = app.status_preferences.get();
    let first = preferences
        .order
        .first()
        .copied()
        .unwrap_or(StatusItemId::Cpu);
    let second = preferences
        .order
        .get(1)
        .copied()
        .unwrap_or(StatusItemId::Audio);
    let item_count = preferences.order.len();
    let clock_format = preferences.clock_format;
    let refresh_app = app.clone();
    let reset_app = app.clone();
    let twenty_four_hour_app = app.clone();
    let twelve_hour_app = app.clone();
    let cities = cities_state.get();
    let current_region = regions.get(region_idx.get()).cloned().unwrap_or_default();
    let current_city = cities.get(city_idx.get()).cloned().unwrap_or_default();
    let cities_state_for_region = cities_state.clone();
    let city_idx_for_region = city_idx.clone();
    let region_idx_for_city = region_idx.clone();
    let cities_state_for_city = cities_state.clone();
    let city_idx_for_city = city_idx.clone();
    let regions_for_city = regions.clone();

    vstack! {
        vstack! {
            Text::new("Optional Status Items").font_size(14.0),
            Text::new("Choose which items appear and their left-to-right order before the clock.")
                .font_size(11.0)
                .color(ColorPalette::default().text_secondary()),
            status_item_row(app.clone(), first, 0, item_count),
            status_item_row(app.clone(), second, 1, item_count),
        }
        .padding(10.0),

        Divider::new(),

        vstack! {
            Text::new("Date & Time").font_size(14.0),
            hstack! {
                Text::new("Region").font_size(13.0).frame_width(110.0),
                Select::new(regions.clone(), region_idx.clone())
                    .width(250.0)
                    .on_change(move |index| {
                        if let Some(region) = regions.get(index) {
                            cities_state_for_region.set(enumerate_cities(region));
                            city_idx_for_region.set(0);
                        }
                        region_idx.set(index);
                    }),
            }
            .padding(8.0),
            hstack! {
                Text::new("City").font_size(13.0).frame_width(110.0),
                Select::new(cities, city_idx_for_city.clone())
                    .width(250.0)
                    .on_change(move |index| {
                        if let (Some(region), Some(city)) = (
                            regions_for_city.get(region_idx_for_city.get()),
                            cities_state_for_city.get().get(index),
                        ) {
                            save_timezone(&format!("{region}/{city}"));
                        }
                        city_idx_for_city.set(index);
                    }),
            }
            .padding(8.0),
            Text::new(format!("Current: {current_region}/{current_city}")).font_size(12.0),
        }
        .padding(10.0),

        Divider::new(),

        vstack! {
            Text::new("Clock").font_size(14.0),
            hstack! {
                Text::new("Clock").font_size(13.0).frame_width(110.0),
                Text::new("Always visible · Fixed at far right")
                    .font_size(12.0)
                    .color(ColorPalette::default().text_secondary()),
            }
            .padding(8.0),
            hstack! {
                Text::new("Format").font_size(13.0).frame_width(110.0),
                Button::new(if clock_format == ClockFormat::TwentyFourHour {
                    "24-hour (Selected)"
                } else {
                    "24-hour"
                })
                .on_click(move || {
                    twenty_four_hour_app.set_status_clock_format(ClockFormat::TwentyFourHour);
                }),
                Spacer::new().frame_width(8.0),
                Button::new(if clock_format == ClockFormat::TwelveHour {
                    "12-hour (Selected)"
                } else {
                    "12-hour"
                })
                .on_click(move || {
                    twelve_hour_app.set_status_clock_format(ClockFormat::TwelveHour);
                }),
            }
            .padding(8.0),
        }
        .padding(10.0),

        Divider::new(),

        vstack! {
            Text::new(app.status_preferences_status.get()).font_size(12.0),
            hstack! {
                Button::new("Refresh").on_click(move || {
                    refresh_app.refresh_status_preferences();
                }),
                Spacer::new().frame_width(8.0),
                Button::new("Reset to Defaults").on_click(move || {
                    reset_app.reset_status_preferences();
                }),
            }
            .padding(8.0),
        }
        .padding(10.0),
    }
    .padding(10.0)
    .frame(f32::INFINITY, f32::INFINITY)
}

impl Application for SettingsApp {
    fn on_idle(&mut self) {
        self.process_picker_ui_events();
    }

    fn scenes(&self) -> impl Scene {
        let app = self.clone();
        let display_app = self.clone();
        let audio_app = self.clone();
        let input_app = self.clone();
        let cursor_app = self.clone();
        let status_app = self.clone();
        let navigation_title = self.navigation_title.clone();
        let tz_regions = self.timezone_regions.get();
        let tz_region_idx = self.timezone_region_index.clone();
        let tz_cities = self.timezone_cities.clone();
        let tz_city_idx = self.timezone_city_index.clone();
        let r0 = self.red_value.clone();
        let g0 = self.green_value.clone();
        let b0 = self.blue_value.clone();
        let r1 = self.red_value.clone();
        let g1 = self.green_value.clone();
        let b1 = self.blue_value.clone();
        let r2 = self.red_value.clone();
        let g2 = self.green_value.clone();
        let b2 = self.blue_value.clone();
        let r3 = self.red_value.clone();
        let g3 = self.green_value.clone();
        let b3 = self.blue_value.clone();
        let r4 = self.red_value.clone();
        let g4 = self.green_value.clone();
        let b4 = self.blue_value.clone();
        let r5 = self.red_value.clone();
        let g5 = self.green_value.clone();
        let b5 = self.blue_value.clone();
        let r6 = self.red_value.clone();
        let g6 = self.green_value.clone();
        let b6 = self.blue_value.clone();
        let r7 = self.red_value.clone();
        let g7 = self.green_value.clone();
        let b7 = self.blue_value.clone();
        let s0 = self.background_style.clone();
        let s1 = self.background_style.clone();
        let s2 = self.background_style.clone();

        let selected_idx = self.selected_preset_index();
        let style = self.background_style.get();
        let style_default = style == BackgroundStyle::GradientLines;
        let style_gradient = style == BackgroundStyle::Gradient;
        let style_solid = style == BackgroundStyle::Solid;

        let is0 = selected_idx == Some(0);
        let is1 = selected_idx == Some(1);
        let is2 = selected_idx == Some(2);
        let is3 = selected_idx == Some(3);
        let is4 = selected_idx == Some(4);
        let is5 = selected_idx == Some(5);
        let is6 = selected_idx == Some(6);
        let is7 = selected_idx == Some(7);

        let highlight = ColorPalette::light().primary();
        let border = ColorPalette::light().border();

        WindowGroup::new(
            "main",
            Window::new(
                "Settings",
                navigation! {
                    NavigationLink::new("Appearance", move || {
                        appearance_page(
                            r0.clone(), g0.clone(), b0.clone(),
                            r1.clone(), g1.clone(), b1.clone(),
                            r2.clone(), g2.clone(), b2.clone(),
                            r3.clone(), g3.clone(), b3.clone(),
                            r4.clone(), g4.clone(), b4.clone(),
                            r5.clone(), g5.clone(), b5.clone(),
                            r6.clone(), g6.clone(), b6.clone(),
                            r7.clone(), g7.clone(), b7.clone(),
                            s0.clone(), s1.clone(), s2.clone(),
                            is0, is1, is2, is3, is4, is5, is6, is7,
                            style_default, style_gradient, style_solid,
                            highlight, border,
                            app.clone()
                        )
                    })
                    .on_select({
                        let title = navigation_title.clone();
                        move || title.set(String::from("Appearance"))
                    }),
                    NavigationLink::new("Display", move || display_page(display_app.clone())).on_select({
                        let title = navigation_title.clone();
                        move || title.set(String::from("Display"))
                    }),
                    NavigationLink::new("Mouse & Cursor", move || cursor_page(cursor_app.clone())).on_select({
                        let title = navigation_title.clone();
                        move || title.set(String::from("Mouse & Cursor"))
                    }),
                    NavigationLink::new("Audio", move || audio_page(audio_app.clone())).on_select({
                        let title = navigation_title.clone();
                        move || title.set(String::from("Audio"))
                    }),
                    NavigationLink::new("Input", move || input_page(input_app.clone())).on_select({
                        let title = navigation_title.clone();
                        move || title.set(String::from("Input"))
                    }),
                    NavigationLink::new("Shell & Status", move || {
                        status_page(
                            status_app.clone(),
                            tz_regions.clone(),
                            tz_region_idx.clone(),
                            tz_cities.clone(),
                            tz_city_idx.clone(),
                        )
                    }).on_select({
                        let title = navigation_title.clone();
                        let app = self.clone();
                        move || {
                            app.refresh_status_preferences();
                            title.set(String::from("Shell & Status"));
                        }
                    }),
                }
                .header(move || {
                    HeaderBar::new(
                        hstack! {
                            Spacer::new(),
                            Text::from_state(navigation_title.clone()).font_size(20.0),
                            Spacer::new(),
                        }
                        .alignment(Alignment::Center)
                        .padding(10.0),
                    )
                    .height(48.0)
                })
                .sidebar_width(150.0)
                .frame(f32::INFINITY, f32::INFINITY),
            )
            .app_id("org.scarlet-os.desktop.settings")
            .size(Size::new(800.0, 600.0)),
        )
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{move_status_item, parse_status_preferences_response, set_status_item_visibility};
    use sbus::Argument;
    use scarlet_desktop_config::{ClockFormat, StatusItemId, StatusPreferences};
    use std::string::String;
    use std::vec;

    #[test]
    fn parses_complete_strict_status_response() {
        let parsed = parse_status_preferences_response(&[
            Argument::String(String::from("audio,cpu")),
            Argument::String(String::from("audio")),
            Argument::String(String::from("12h")),
        ])
        .unwrap();

        assert_eq!(parsed.order, vec![StatusItemId::Audio, StatusItemId::Cpu]);
        assert_eq!(parsed.visible, vec![StatusItemId::Audio]);
        assert_eq!(parsed.clock_format, ClockFormat::TwelveHour);
    }

    #[test]
    fn rejects_incomplete_or_non_strict_status_response() {
        assert!(
            parse_status_preferences_response(&[
                Argument::String(String::from("cpu,audio")),
                Argument::String(String::from("cpu,audio")),
            ])
            .is_err()
        );
        assert!(
            parse_status_preferences_response(&[
                Argument::String(String::from("cpu,cpu")),
                Argument::String(String::from("cpu")),
                Argument::String(String::from("24h")),
            ])
            .is_err()
        );
    }

    #[test]
    fn reorders_items_and_keeps_visible_items_in_display_order() {
        let preferences = StatusPreferences {
            order: vec![StatusItemId::Cpu, StatusItemId::Audio],
            visible: vec![StatusItemId::Cpu, StatusItemId::Audio],
            clock_format: ClockFormat::TwentyFourHour,
        };
        let moved = move_status_item(&preferences, StatusItemId::Audio, -1).unwrap();

        assert_eq!(moved.order, vec![StatusItemId::Audio, StatusItemId::Cpu]);
        assert_eq!(moved.visible, moved.order);
        assert!(move_status_item(&moved, StatusItemId::Audio, -1).is_none());
    }

    #[test]
    fn visibility_changes_are_ordered_and_leave_clock_outside_optional_state() {
        let preferences = StatusPreferences {
            order: vec![StatusItemId::Audio, StatusItemId::Cpu],
            visible: vec![StatusItemId::Audio],
            clock_format: ClockFormat::TwelveHour,
        };
        let shown = set_status_item_visibility(&preferences, StatusItemId::Cpu, true);
        let hidden = set_status_item_visibility(&shown, StatusItemId::Audio, false);

        assert_eq!(shown.visible, vec![StatusItemId::Audio, StatusItemId::Cpu]);
        assert_eq!(hidden.visible, vec![StatusItemId::Cpu]);
        assert_eq!(hidden.clock_format, ClockFormat::TwelveHour);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("[settings] Starting");
    let mut app = SettingsApp::new();
    match app.run() {
        Ok(_) => println!("[settings] Done"),
        Err(e) => println!("[settings] Error: {}", e),
    }
}
