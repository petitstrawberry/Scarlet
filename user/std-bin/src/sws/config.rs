//! Persistent configuration support for the Scarlet Window Server.

use std::format;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::string::String;
use std::sync::Mutex;
use sws_protocol::outputs::{
    self as protocol, OutputConfiguration, OutputDescriptor, OutputExperience, OutputMode,
    OutputRole, OutputSnapshot,
};

/// System-wide SWS configuration path inside the Scarlet bundle namespace.
pub(super) const SWS_CONFIG_PATH: &str = "/etc/sws/config.toml";

const SWS_CONFIG_TEMP_PATH: &str = "/tmp/sws-config.tmp";
const INPUT_METHOD_SECTION: &str = "input_method";
const ACTIVE_INPUT_METHOD_KEY: &str = "active";
const CURSOR_SECTION: &str = "cursor";
const CURSOR_THEME_KEY: &str = "theme";

static CONFIG_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Read the current SWS configuration.
///
/// # Returns
///
/// The UTF-8 configuration contents, or an error when the file cannot be read.
pub(super) fn read_sws_config() -> Result<String, &'static str> {
    let mut file = File::open(SWS_CONFIG_PATH).map_err(|_| "Failed to open SWS config")?;
    let mut content = String::new();
    let mut buffer = [0u8; 1024];

    loop {
        let bytes = file
            .read(&mut buffer)
            .map_err(|_| "Failed to read SWS config")?;
        if bytes == 0 {
            break;
        }
        let chunk =
            core::str::from_utf8(&buffer[..bytes]).map_err(|_| "SWS config is not valid UTF-8")?;
        content.push_str(chunk);
    }

    Ok(content)
}

/// Parse the stable name of the preferred input method.
///
/// # Arguments
///
/// * `content` - SWS TOML configuration contents.
///
/// # Returns
///
/// The configured name from `[input_method].active`, if present and non-empty.
pub(super) fn parse_active_input_method(content: &str) -> Option<String> {
    let mut accepts_input_method = false;

    for raw_line in content.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(section) = section_name(line) {
            accepts_input_method = section == INPUT_METHOD_SECTION;
            continue;
        }

        if !accepts_input_method {
            continue;
        }

        let Some(eq_pos) = line.find('=') else {
            continue;
        };
        if line[..eq_pos].trim() != ACTIVE_INPUT_METHOD_KEY {
            continue;
        }

        let name = trim_toml_string(line[eq_pos + 1..].trim());
        if !name.is_empty() {
            return Some(String::from(name));
        }
    }

    None
}

/// Persist the preferred input method while preserving unrelated SWS settings.
///
/// # Arguments
///
/// * `name` - Stable input method service name to store.
///
/// # Returns
///
/// `Ok(())` after the staged update has replaced the old config, or an error
/// when the configuration cannot be written.
pub(super) fn persist_active_input_method(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("Input method name must not be empty");
    }

    persist_string_setting(INPUT_METHOD_SECTION, ACTIVE_INPUT_METHOD_KEY, name)
}

/// Persist the active cursor theme while preserving unrelated SWS settings.
///
/// # Arguments
///
/// * `theme_path` - Validated installed cursor theme directory.
///
/// # Returns
///
/// `Ok(())` after the staged update has replaced the old config, or an error
/// when the configuration cannot be written.
pub(super) fn persist_cursor_theme(theme_path: &str) -> Result<(), &'static str> {
    if theme_path.is_empty() {
        return Err("Cursor theme path must not be empty");
    }

    persist_string_setting(CURSOR_SECTION, CURSOR_THEME_KEY, theme_path)
}

fn persist_string_setting(section: &str, key: &str, value: &str) -> Result<(), &'static str> {
    let _guard = CONFIG_WRITE_LOCK.lock().expect("SWS mutex poisoned");
    let content = read_sws_config()?;
    let updated = update_string_assignment(&content, section, key, value);

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = options
        .open(SWS_CONFIG_TEMP_PATH)
        .map_err(|_| "Failed to create temporary SWS config")?;
    if file.write_all(updated.as_bytes()).is_err() || file.flush().is_err() {
        let _ = fs::remove_file(SWS_CONFIG_TEMP_PATH);
        return Err("Failed to write temporary SWS config");
    }
    drop(file);

    if fs::rename(SWS_CONFIG_TEMP_PATH, SWS_CONFIG_PATH).is_err() {
        // `/tmp` may be a separate filesystem. Keep staging there, then fall
        // back to a complete write when a cross-filesystem rename is not
        // available.
        let mut final_options = OpenOptions::new();
        final_options.write(true).create(true).truncate(true);
        let mut final_file = match final_options.open(SWS_CONFIG_PATH) {
            Ok(file) => file,
            Err(_) => {
                let _ = fs::remove_file(SWS_CONFIG_TEMP_PATH);
                return Err("Failed to open final SWS config");
            }
        };
        if final_file.write_all(updated.as_bytes()).is_err() || final_file.flush().is_err() {
            let _ = fs::remove_file(SWS_CONFIG_TEMP_PATH);
            return Err("Failed to replace SWS config");
        }
        drop(final_file);
        let _ = fs::remove_file(SWS_CONFIG_TEMP_PATH);
    }

    Ok(())
}

#[cfg(test)]
fn update_active_input_method(content: &str, name: &str) -> String {
    update_string_assignment(content, INPUT_METHOD_SECTION, ACTIVE_INPUT_METHOD_KEY, name)
}

fn update_string_assignment(content: &str, section: &str, key: &str, value: &str) -> String {
    let assignment = format!("{} = \"{}\"", key, escape_toml_basic_string(value));
    let mut output = String::new();
    let mut in_target_section = false;
    let mut saw_target_section = false;
    let mut wrote_assignment = false;

    for raw_line in content.lines() {
        let logical_line = strip_toml_comment(raw_line).trim();
        if let Some(line_section) = section_name(logical_line) {
            if in_target_section && !wrote_assignment {
                output.push_str(&assignment);
                output.push('\n');
                wrote_assignment = true;
            }
            in_target_section = line_section == section;
            saw_target_section |= in_target_section;
        }

        if in_target_section && assignment_key(logical_line) == Some(key) {
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

fn section_name(line: &str) -> Option<&str> {
    if line.len() >= 2 && line.starts_with('[') && line.ends_with(']') {
        Some(line[1..line.len() - 1].trim())
    } else {
        None
    }
}

fn assignment_key(line: &str) -> Option<&str> {
    let eq_pos = line.find('=')?;
    Some(line[..eq_pos].trim())
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '#' if !in_string => return &line[..index],
            _ => {}
        }
    }
    line
}

fn trim_toml_string(value: &str) -> &str {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].trim()
    } else {
        value
    }
}

fn escape_toml_basic_string(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "[output]\nscale = 2.0\n\n[cursor]\ntheme = \"/share/cursors/default\"\n\n[input_method]\nactive = \"scarlet-mozc\"\n";

    #[test]
    fn cursor_theme_update_preserves_other_sections() {
        let updated =
            update_string_assignment(CONFIG, "cursor", "theme", "/share/cursors/shigure-ui-a");

        assert!(updated.contains("[output]\nscale = 2.0"));
        assert!(updated.contains("theme = \"/share/cursors/shigure-ui-a\""));
        assert!(updated.contains("[input_method]\nactive = \"scarlet-mozc\""));
        assert!(!updated.contains("theme = \"/share/cursors/default\""));
    }

    #[test]
    fn missing_cursor_section_is_appended() {
        let updated = update_string_assignment(
            "[output]\nscale = 1.0\n",
            "cursor",
            "theme",
            "/share/cursors/default",
        );

        assert!(updated.contains("[output]\nscale = 1.0"));
        assert!(updated.ends_with("[cursor]\ntheme = \"/share/cursors/default\"\n"));
    }

    #[test]
    fn input_method_update_escapes_and_preserves_cursor() {
        let updated = update_active_input_method(CONFIG, "quoted\\\"ime");

        assert!(updated.contains("active = \"quoted\\\\\\\"ime\""));
        assert!(updated.contains("theme = \"/share/cursors/default\""));
    }

    fn displays() -> OutputSnapshot {
        let outputs = ["panel", "projector", "monitor"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| OutputDescriptor {
                id: index as u32,
                connector_key: format!("port-{index}"),
                monitor_key: name.into(),
                identity_is_unique: true,
                name: name.into(),
                primary: index == 0,
                internal: index == 0,
                mode: OutputMode {
                    pixel_width: 100,
                    pixel_height: 100,
                    refresh_millihz: 60_000,
                },
                physical_width_mm: 0,
                physical_height_mm: 0,
            })
            .collect();
        let configuration = [
            (0, OutputRole::Extend),
            (-100, OutputRole::Mirror),
            (100, OutputRole::Disabled),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (x, role))| OutputConfiguration {
            id: index as u32,
            x,
            y: 0,
            scale_milli: 1000,
            experience: OutputExperience::Auto,
            role,
        })
        .collect();
        OutputSnapshot {
            generation: 1,
            outputs,
            configuration,
        }
    }

    fn saved_document(snapshot: &OutputSnapshot) -> toml::Value {
        let mut document = CONFIG.parse().unwrap();
        let key = protocol::topology_key(&snapshot.outputs).unwrap();
        update_output_layout(&mut document, snapshot, key).unwrap();
        toml::to_string(&document).unwrap().parse().unwrap()
    }

    #[test]
    fn roles_and_parked_positions_follow_monitors_across_ports_and_ids() {
        let original = displays();
        let document = saved_document(&original);
        let mut outputs = original.outputs.clone();
        outputs.reverse();
        for (index, output) in outputs.iter_mut().enumerate() {
            output.id = 10 + index as u32;
            output.connector_key = format!("new-port-{index}");
        }
        let configuration = output_layout_from(&document, &outputs).unwrap();
        for (output, config) in outputs.iter().zip(configuration) {
            let old = original
                .outputs
                .iter()
                .position(|old| old.monitor_key == output.monitor_key)
                .unwrap();
            assert_eq!(
                config,
                OutputConfiguration {
                    id: output.id,
                    ..original.configuration[old]
                }
            );
        }
        assert_eq!(
            document["cursor"]["theme"].as_str(),
            Some("/share/cursors/default")
        );
        assert!(output_layout_from(&document, &outputs[..2]).is_none());
        outputs[1].identity_is_unique = false;
        assert!(output_layout_from(&document, &outputs).is_none());
        outputs[1].identity_is_unique = true;
        outputs[1].monitor_key = outputs[0].monitor_key.clone();
        assert!(output_layout_from(&document, &outputs).is_none());
    }

    #[test]
    fn omitted_roles_restore_as_extended_and_unknown_roles_reject_the_layout() {
        let original = displays();
        let mut document = saved_document(&original);
        for entry in document["display_layouts"][0]["configuration"]
            .as_array_mut()
            .unwrap()
        {
            entry.as_table_mut().unwrap().remove("role");
        }
        let configuration = output_layout_from(&document, &original.outputs).unwrap();
        for (config, old) in configuration.iter().zip(original.configuration) {
            assert_eq!(
                *config,
                OutputConfiguration {
                    role: OutputRole::Extend,
                    ..old
                }
            );
        }
        document["display_layouts"][0]["configuration"][1]
            .as_table_mut()
            .unwrap()
            .insert("role".into(), "typo".into());
        assert!(output_layout_from(&document, &original.outputs).is_none());
    }
}

/// Save one display combination atomically. Profiles are matched by monitor
/// identity, never by a port, runtime output ID or enumeration order.
pub(super) fn persist_outputs(snapshot: &OutputSnapshot) -> Result<(), &'static str> {
    let Some(key) = protocol::topology_key(&snapshot.outputs) else {
        return Ok(());
    };
    let _guard = CONFIG_WRITE_LOCK.lock().expect("SWS config mutex");
    let content = read_sws_config()?;
    let mut document = content
        .parse::<toml::Value>()
        .map_err(|_| "Invalid SWS configuration")?;
    update_output_layout(&mut document, snapshot, key)?;
    let updated = toml::to_string(&document).map_err(|_| "Cannot encode display layouts")?;
    let temporary = "/etc/sws/.config-output.tmp";
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(temporary)
        .map_err(|_| "Cannot stage display layout")?;
    if file.write_all(updated.as_bytes()).is_err() || file.flush().is_err() {
        let _ = fs::remove_file(temporary);
        return Err("Cannot write display layout");
    }
    drop(file);
    fs::rename(temporary, SWS_CONFIG_PATH).map_err(|_| "Cannot publish display layout")
}

fn update_output_layout(
    document: &mut toml::Value,
    snapshot: &OutputSnapshot,
    key: Vec<String>,
) -> Result<(), &'static str> {
    let table = document.as_table_mut().ok_or("Invalid SWS configuration")?;
    let layouts = table
        .entry("display_layouts")
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or("Invalid display layouts")?;
    let key_value = toml::Value::Array(key.into_iter().map(toml::Value::String).collect());
    let mut profile = toml::map::Map::new();
    profile.insert("monitors".into(), key_value.clone());
    profile.insert(
        "primary_monitor".into(),
        toml::Value::String(
            snapshot
                .outputs
                .iter()
                .find(|output| output.primary)
                .ok_or("Missing primary output")?
                .monitor_key
                .clone(),
        ),
    );
    let mut entries = Vec::new();
    for output in &snapshot.outputs {
        let config = snapshot
            .configuration
            .iter()
            .find(|config| config.id == output.id)
            .ok_or("Missing output configuration")?;
        let mut entry = toml::map::Map::new();
        entry.insert(
            "monitor".into(),
            toml::Value::String(output.monitor_key.clone()),
        );
        entry.insert(
            "role".into(),
            toml::Value::String(
                match config.role {
                    OutputRole::Extend => "extend",
                    OutputRole::Mirror => "mirror",
                    OutputRole::Disabled => "disabled",
                }
                .into(),
            ),
        );
        for (name, value) in [
            ("x", i64::from(config.x)),
            ("y", i64::from(config.y)),
            ("scale_milli", i64::from(config.scale_milli)),
            ("experience", config.experience as i64),
        ] {
            entry.insert(name.into(), toml::Value::Integer(value));
        }
        for (name, value) in [
            ("mode_width", output.mode.pixel_width),
            ("mode_height", output.mode.pixel_height),
            ("mode_refresh_millihz", output.mode.refresh_millihz),
        ] {
            entry.insert(name.into(), toml::Value::Integer(i64::from(value)));
        }
        entries.push(toml::Value::Table(entry));
    }
    profile.insert("configuration".into(), toml::Value::Array(entries));
    if let Some(index) = layouts
        .iter()
        .position(|layout| layout.get("monitors") == Some(&key_value))
    {
        layouts.remove(index);
    }
    layouts.push(toml::Value::Table(profile));
    if layouts.len() > 64 {
        layouts.remove(0);
    }
    Ok(())
}

fn saved_entries<'a>(
    document: &'a toml::Value,
    outputs: &[OutputDescriptor],
) -> Option<&'a [toml::Value]> {
    let key = protocol::topology_key(outputs)?;
    let monitors = toml::Value::Array(key.into_iter().map(toml::Value::String).collect());
    let layout = document
        .get("display_layouts")?
        .as_array()?
        .iter()
        .find(|layout| layout.get("monitors") == Some(&monitors))?;
    Some(layout.get("configuration")?.as_array()?.as_slice())
}

fn monitor_entry<'a>(
    entries: &'a [toml::Value],
    output: &OutputDescriptor,
) -> Option<&'a toml::Value> {
    entries.iter().find(|entry| {
        entry.get("monitor").and_then(toml::Value::as_str) == Some(output.monitor_key.as_str())
    })
}

pub(super) fn saved_output_layout(
    outputs: &[OutputDescriptor],
) -> Option<Vec<OutputConfiguration>> {
    let document = read_sws_config().ok()?.parse().ok()?;
    output_layout_from(&document, outputs)
}

fn output_layout_from(
    document: &toml::Value,
    outputs: &[OutputDescriptor],
) -> Option<Vec<OutputConfiguration>> {
    let entries = saved_entries(document, outputs)?;
    let mut configuration = Vec::with_capacity(outputs.len());
    for output in outputs {
        let entry = monitor_entry(entries, output)?;
        configuration.push(OutputConfiguration {
            id: output.id,
            x: entry.get("x")?.as_integer()?.try_into().ok()?,
            y: entry.get("y")?.as_integer()?.try_into().ok()?,
            scale_milli: entry.get("scale_milli")?.as_integer()?.try_into().ok()?,
            experience: match entry.get("experience")?.as_integer()? {
                0 => OutputExperience::Auto,
                1 => OutputExperience::Desktop,
                2 => OutputExperience::Tablet,
                _ => return None,
            },
            role: match entry.get("role") {
                None => OutputRole::Extend, // Profiles written before roles were introduced.
                Some(value) => match value.as_str()? {
                    "extend" => OutputRole::Extend,
                    "mirror" => OutputRole::Mirror,
                    "disabled" => OutputRole::Disabled,
                    _ => return None,
                },
            },
        });
    }
    protocol::canonical_layout(outputs, &configuration).ok()
}

/// Mode preferences use the same monitor-combination identity as arrangement.
pub(super) fn saved_output_modes(outputs: &[OutputDescriptor]) -> Vec<(u32, OutputMode)> {
    let Some(document) = read_sws_config()
        .ok()
        .and_then(|config| config.parse().ok())
    else {
        return Vec::new();
    };
    let Some(entries) = saved_entries(&document, outputs) else {
        return Vec::new();
    };
    outputs
        .iter()
        .filter_map(|output| {
            let entry = monitor_entry(entries, output)?;
            Some((
                output.id,
                OutputMode {
                    pixel_width: entry.get("mode_width")?.as_integer()?.try_into().ok()?,
                    pixel_height: entry.get("mode_height")?.as_integer()?.try_into().ok()?,
                    refresh_millihz: entry
                        .get("mode_refresh_millihz")?
                        .as_integer()?
                        .try_into()
                        .ok()?,
                },
            ))
        })
        .collect()
}
