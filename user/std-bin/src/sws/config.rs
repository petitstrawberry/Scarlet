//! Persistent configuration support for the Scarlet Window Server.

use std::format;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::string::String;
use std::sync::Mutex;

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
    use super::{update_active_input_method, update_string_assignment};

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
}
