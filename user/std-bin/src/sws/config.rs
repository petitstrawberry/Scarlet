//! Persistent configuration support for the Scarlet Window Server.

use std::format;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::string::String;
use std::sync::Mutex;

/// System-wide SWS configuration path inside the Scarlet bundle namespace.
pub(super) const SWS_CONFIG_PATH: &str = "/etc/sws/config.toml";

const SWS_CONFIG_TEMP_PATH: &str = "/etc/sws/config.toml.tmp";
const INPUT_METHOD_SECTION: &str = "input_method";
const ACTIVE_INPUT_METHOD_KEY: &str = "active";

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
/// `Ok(())` after the updated file has atomically replaced the old config, or
/// an error when the configuration cannot be written.
pub(super) fn persist_active_input_method(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("Input method name must not be empty");
    }

    let _guard = CONFIG_WRITE_LOCK.lock().expect("SWS mutex poisoned");
    let content = read_sws_config()?;
    let updated = update_active_input_method(&content, name);

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
        let _ = fs::remove_file(SWS_CONFIG_TEMP_PATH);
        return Err("Failed to replace SWS config");
    }

    Ok(())
}

fn update_active_input_method(content: &str, name: &str) -> String {
    let assignment = format!(
        "{} = \"{}\"",
        ACTIVE_INPUT_METHOD_KEY,
        escape_toml_basic_string(name)
    );
    let mut output = String::new();
    let mut in_input_method = false;
    let mut saw_input_method = false;
    let mut wrote_active = false;

    for raw_line in content.lines() {
        let logical_line = strip_toml_comment(raw_line).trim();
        if let Some(section) = section_name(logical_line) {
            if in_input_method && !wrote_active {
                output.push_str(&assignment);
                output.push('\n');
                wrote_active = true;
            }
            in_input_method = section == INPUT_METHOD_SECTION;
            saw_input_method |= in_input_method;
        }

        if in_input_method && assignment_key(logical_line) == Some(ACTIVE_INPUT_METHOD_KEY) {
            if !wrote_active {
                output.push_str(&assignment);
                output.push('\n');
                wrote_active = true;
            }
            continue;
        }

        output.push_str(raw_line);
        output.push('\n');
    }

    if in_input_method && !wrote_active {
        output.push_str(&assignment);
        output.push('\n');
        wrote_active = true;
    }

    if !saw_input_method {
        if !output.is_empty() && !output.ends_with("\n\n") {
            output.push('\n');
        }
        output.push_str("[input_method]\n");
        output.push_str(&assignment);
        output.push('\n');
        wrote_active = true;
    }

    debug_assert!(wrote_active);
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
