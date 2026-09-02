#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Sender bus name used by Settings for desktop configuration signals.
pub const DESKTOP_SETTINGS_SIGNAL_SENDER: &str = "org.scarlet-os.desktop.settings";

/// Bus name owned by the headless desktop settings service.
pub const DESKTOP_SETTINGS_BUS_NAME: &str = "org.scarlet-os.desktop.settings";

/// Object path used by the desktop settings service.
pub const DESKTOP_SETTINGS_SERVICE_OBJECT_PATH: &str = "/org/scarlet/os/desktop";

/// Interface implemented by the desktop settings service.
pub const DESKTOP_SETTINGS_SERVICE_INTERFACE: &str = "org.scarlet.desktop.Settings";

/// Method used to persist the complete background configuration.
pub const DESKTOP_SETTINGS_SET_BACKGROUND_METHOD: &str = "SetBackground";

/// Method used to restore the generated default background.
pub const DESKTOP_SETTINGS_RESET_BACKGROUND_METHOD: &str = "ResetBackground";

/// User/system desktop configuration directory used by the current Scarlet
/// desktop profile.
pub const DESKTOP_CONFIG_DIR: &str = "/etc/scarlet-desktop.d";

/// Persistent desktop background configuration path.
pub const DESKTOP_BACKGROUND_CONFIG_PATH: &str = "/etc/scarlet-desktop.d/background.toml";

/// Bus name registered by the desktop background listener.
pub const DESKTOP_BACKGROUND_BUS_NAME: &str = "org.scarlet-os.desktop.background";

/// Object path used for desktop configuration signals.
pub const DESKTOP_SETTINGS_OBJECT_PATH: &str = "/org/scarlet/os/desktop";

/// Interface used for desktop configuration signals.
pub const DESKTOP_SETTINGS_INTERFACE: &str = "org.scarlet.desktop.Settings";

/// Signal emitted after the desktop background configuration was saved.
pub const DESKTOP_BACKGROUND_CHANGED_SIGNAL: &str = "BackgroundChanged";

/// Bus name owned by the File Manager and its picker mode.
pub const DESKTOP_FILE_MANAGER_BUS_NAME: &str = "org.scarlet-os.desktop.filemanager";

/// Application ID of the user-facing Files application.
pub const DESKTOP_FILES_APP_ID: &str = "org.scarlet-os.desktop.files";

/// Object path used by the File Manager service.
pub const DESKTOP_FILE_MANAGER_OBJECT_PATH: &str = "/org/scarlet/os/filemanager";

/// Interface implemented by the File Manager service.
pub const DESKTOP_FILE_MANAGER_INTERFACE: &str = "org.scarlet.desktop.FileManager";

/// Method used to open the File Manager in picker mode.
pub const DESKTOP_FILE_MANAGER_OPEN_FILE_METHOD: &str = "OpenFile";

/// Method used to open the File Manager in save-file picker mode.
pub const DESKTOP_FILE_MANAGER_SAVE_FILE_METHOD: &str = "SaveFile";

/// Method used to show the normal File Manager window.
pub const DESKTOP_FILE_MANAGER_SHOW_METHOD: &str = "Show";

/// Signal emitted after a picker request is accepted or cancelled.
pub const DESKTOP_FILE_MANAGER_RESPONSE_SIGNAL: &str = "Response";

/// Bus name owned by the desktop service manager.
pub const DESKTOP_STEMD_BUS_NAME: &str = "org.scarlet-os.stemd";

/// Object path used by the desktop service manager.
pub const DESKTOP_STEMD_OBJECT_PATH: &str = "/org/scarlet/os/stemd";

/// Interface implemented by the desktop service manager.
pub const DESKTOP_STEMD_INTERFACE: &str = "org.scarlet-os.stemd";

/// Method used to open a local filesystem path with its default application.
pub const DESKTOP_STEMD_OPEN_PATH_METHOD: &str = "OpenPath";

/// Method used to list applications registered from desktop entries.
pub const DESKTOP_STEMD_LIST_APPLICATIONS_METHOD: &str = "ListApplications";

/// Method used to launch an application or focus its existing window.
pub const DESKTOP_STEMD_LAUNCH_OR_FOCUS_METHOD: &str = "LaunchOrFocus";

/// Persistent desktop status-preferences configuration path.
pub const DESKTOP_STATUS_CONFIG_PATH: &str = "/etc/scarlet-desktop.d/status.toml";

/// Current on-disk schema version for [`StatusPreferences`].
pub const STATUS_PREFERENCES_CONFIG_VERSION: u32 = 1;

/// Method used to retrieve the complete status-preferences configuration.
pub const DESKTOP_SETTINGS_GET_STATUS_PREFERENCES_METHOD: &str = "GetStatusPreferences";

/// Method used to persist the complete status-preferences configuration.
pub const DESKTOP_SETTINGS_SET_STATUS_PREFERENCES_METHOD: &str = "SetStatusPreferences";

/// Method used to restore the default status-preferences configuration.
pub const DESKTOP_SETTINGS_RESET_STATUS_PREFERENCES_METHOD: &str = "ResetStatusPreferences";

/// Signal emitted after status preferences were saved or reset.
pub const DESKTOP_STATUS_PREFERENCES_CHANGED_SIGNAL: &str = "StatusPreferencesChanged";

/// An optional item displayed immediately before the fixed trailing clock.
///
/// The clock deliberately is not represented by this type: it is always visible
/// and always remains the far-right status item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusItemId {
    /// Display processor utilization in the status area.
    Cpu,
    /// Display the audio status and volume in the status area.
    Audio,
}

impl StatusItemId {
    /// Returns all optional status items in their deterministic default order.
    ///
    /// # Returns
    ///
    /// The complete optional-item set. The fixed clock is intentionally excluded.
    pub const fn all() -> [Self; 2] {
        [Self::Cpu, Self::Audio]
    }

    /// Parses the stable, lower-case identifier used by IPC and configuration.
    ///
    /// # Arguments
    ///
    /// * `value` - Identifier to parse.
    ///
    /// # Returns
    ///
    /// The corresponding item, or `None` when the identifier is unknown.
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "cpu" => Some(Self::Cpu),
            "audio" => Some(Self::Audio),
            _ => None,
        }
    }

    /// Returns the stable, lower-case identifier used by IPC and configuration.
    ///
    /// # Returns
    ///
    /// The identifier that is safe to persist or transmit over IPC.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Audio => "audio",
        }
    }
}

/// The presentation format for the fixed trailing clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClockFormat {
    /// Render time using a 24-hour clock.
    #[default]
    TwentyFourHour,
    /// Render time using a 12-hour clock.
    TwelveHour,
}

impl ClockFormat {
    /// Parses the stable format identifier used by IPC and configuration.
    ///
    /// # Arguments
    ///
    /// * `value` - Format identifier to parse.
    ///
    /// # Returns
    ///
    /// The corresponding format, or `None` when the identifier is invalid.
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "24h" => Some(Self::TwentyFourHour),
            "12h" => Some(Self::TwelveHour),
            _ => None,
        }
    }

    /// Returns the stable format identifier used by IPC and configuration.
    ///
    /// # Returns
    ///
    /// The identifier that is safe to persist or transmit over IPC.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TwentyFourHour => "24h",
            Self::TwelveHour => "12h",
        }
    }
}

/// User-configurable optional status items and the fixed clock's format.
///
/// The [`Self::order`] and [`Self::visible`] fields contain optional items only.
/// The clock is intentionally excluded because it is always visible and fixed as
/// the trailing, far-right status item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusPreferences {
    /// Ordered optional items, with each known item appearing exactly once after normalization.
    pub order: Vec<StatusItemId>,
    /// Optional items which should be displayed, in their configured order.
    pub visible: Vec<StatusItemId>,
    /// Presentation format for the fixed trailing clock.
    pub clock_format: ClockFormat,
}

impl Default for StatusPreferences {
    fn default() -> Self {
        Self {
            order: StatusItemId::all().to_vec(),
            visible: StatusItemId::all().to_vec(),
            clock_format: ClockFormat::default(),
        }
    }
}

impl StatusPreferences {
    /// Normalizes disk-derived preferences without permitting duplicate or missing controls.
    ///
    /// Unknown disk strings are discarded by [`Self::from_disk_values`]. Duplicate
    /// entries are removed, missing known items are appended to [`Self::order`] in
    /// default order, and [`Self::visible`] is limited to known unique items.
    ///
    /// # Returns
    ///
    /// A safe status-preferences value. The fixed clock remains outside this model.
    pub fn normalize(mut self) -> Self {
        self.order = Self::normalize_order(&self.order);
        self.visible = Self::normalize_visible(&self.visible);
        self
    }

    /// Parses disk-derived CSV values and safely normalizes malformed item lists.
    ///
    /// # Arguments
    ///
    /// * `order` - Comma-separated optional status-item identifiers.
    /// * `visible` - Comma-separated visible optional status-item identifiers.
    /// * `clock_format` - Clock-format identifier.
    ///
    /// # Returns
    ///
    /// Safe normalized preferences. Unknown items and an invalid clock format use
    /// the deterministic default behavior rather than causing a configuration load failure.
    pub fn from_disk_values(order: &str, visible: &str, clock_format: &str) -> Self {
        Self {
            order: Self::parse_csv_lossy(order),
            visible: Self::parse_csv_lossy(visible),
            clock_format: ClockFormat::from_str(clock_format).unwrap_or_default(),
        }
        .normalize()
    }

    /// Strictly parses IPC values for a status-preferences update.
    ///
    /// # Arguments
    ///
    /// * `order` - Comma-separated optional status-item identifiers.
    /// * `visible` - Comma-separated visible optional status-item identifiers.
    /// * `clock_format` - Clock-format identifier.
    ///
    /// # Returns
    ///
    /// Valid preferences when `order` contains every known item exactly once and
    /// both lists contain only known, non-duplicate identifiers. Invalid IPC input
    /// is rejected rather than normalized.
    pub fn from_ipc_values(
        order: &str,
        visible: &str,
        clock_format: &str,
    ) -> Result<Self, &'static str> {
        let order = Self::parse_csv_strict(order, false)?;
        if order.len() != StatusItemId::all().len()
            || !StatusItemId::all().iter().all(|item| order.contains(item))
        {
            return Err("Status preference order must contain each item exactly once");
        }

        Ok(Self {
            order,
            visible: Self::parse_csv_strict(visible, true)?,
            clock_format: ClockFormat::from_str(clock_format)
                .ok_or("Invalid status preference clock format")?,
        })
    }

    /// Serializes the optional-item order as stable comma-separated values.
    ///
    /// # Returns
    ///
    /// A deterministic CSV string suitable for IPC and configuration persistence.
    pub fn order_csv(&self) -> String {
        Self::serialize_csv(&self.order)
    }

    /// Serializes visible optional items as stable comma-separated values.
    ///
    /// # Returns
    ///
    /// A deterministic CSV string suitable for IPC and configuration persistence.
    pub fn visible_csv(&self) -> String {
        Self::serialize_csv(&self.visible)
    }

    /// Serializes this value as a complete versioned TOML `[status]` section.
    ///
    /// The serialization is deterministic and suitable for atomic persistence by
    /// the desktop settings service.
    ///
    /// # Returns
    ///
    /// A complete TOML section with [`STATUS_PREFERENCES_CONFIG_VERSION`].
    pub fn to_toml_section(&self) -> String {
        let normalized = self.clone().normalize();
        format!(
            "[status]\nconfig_version = {}\norder = \"{}\"\nvisible = \"{}\"\nclock_format = \"{}\"\n",
            STATUS_PREFERENCES_CONFIG_VERSION,
            normalized.order_csv(),
            normalized.visible_csv(),
            normalized.clock_format.as_str(),
        )
    }

    /// Returns whether an optional item is currently visible.
    ///
    /// # Arguments
    ///
    /// * `item` - Optional status item to query.
    ///
    /// # Returns
    ///
    /// `true` when the item is visible. The fixed clock is always visible and is
    /// deliberately not queryable through this optional-item API.
    pub fn is_visible(&self, item: StatusItemId) -> bool {
        self.visible.contains(&item)
    }

    fn parse_csv_lossy(value: &str) -> Vec<StatusItemId> {
        value
            .split(',')
            .filter_map(|item| StatusItemId::from_str(item.trim()))
            .collect()
    }

    fn parse_csv_strict(value: &str, allow_empty: bool) -> Result<Vec<StatusItemId>, &'static str> {
        if value.trim().is_empty() {
            return if allow_empty {
                Ok(Vec::new())
            } else {
                Err("Status preference order is missing")
            };
        }

        let mut items = Vec::new();
        for value in value.split(',') {
            let item =
                StatusItemId::from_str(value.trim()).ok_or("Unknown status preference item")?;
            if items.contains(&item) {
                return Err("Duplicate status preference item");
            }
            items.push(item);
        }
        Ok(items)
    }

    fn normalize_order(items: &[StatusItemId]) -> Vec<StatusItemId> {
        let mut normalized = Self::normalize_visible(items);
        for item in StatusItemId::all() {
            if !normalized.contains(&item) {
                normalized.push(item);
            }
        }
        normalized
    }

    fn normalize_visible(items: &[StatusItemId]) -> Vec<StatusItemId> {
        let mut normalized = Vec::new();
        for item in items {
            if !normalized.contains(item) {
                normalized.push(*item);
            }
        }
        normalized
    }

    fn serialize_csv(items: &[StatusItemId]) -> String {
        let mut serialized = String::new();
        for (index, item) in items.iter().enumerate() {
            if index != 0 {
                serialized.push(',');
            }
            serialized.push_str(item.as_str());
        }
        serialized
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThemeColors {
    /// Generated background color.
    pub background: Option<[u8; 3]>,
    /// Generated background style.
    pub background_style: Option<BackgroundStyle>,
    /// Optional local image path used as the desktop background.
    pub background_image: Option<String>,
    pub taskbar: Option<[u8; 3]>,
    pub text: Option<[u8; 3]>,
    pub highlight: Option<[u8; 3]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackgroundStyle {
    #[default]
    GradientLines,
    Gradient,
    Solid,
}

impl BackgroundStyle {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "gradient_lines" | "gradient-lines" | "lines" => Some(BackgroundStyle::GradientLines),
            "gradient" => Some(BackgroundStyle::Gradient),
            "solid" => Some(BackgroundStyle::Solid),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            BackgroundStyle::GradientLines => "gradient_lines",
            BackgroundStyle::Gradient => "gradient",
            BackgroundStyle::Solid => "solid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskbarConfig {
    pub height: Option<u32>,
    pub position: Option<TaskbarPosition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarPosition {
    Top,
    Bottom,
}

impl TaskbarPosition {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "top" => Some(TaskbarPosition::Top),
            "bottom" => Some(TaskbarPosition::Bottom),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DesktopConfig {
    pub theme: ThemeColors,
    pub taskbar: TaskbarConfig,
    /// Preferences for optional status items and the fixed trailing clock.
    pub status: StatusPreferences,
}

pub struct ConfigParser {
    content: String,
}

impl ConfigParser {
    pub fn new(content: String) -> Self {
        Self { content }
    }

    pub fn parse(&self) -> DesktopConfig {
        let mut config = DesktopConfig::default();
        let lines: Vec<&str> = self.content.lines().collect();
        let mut i = 0;

        while i < lines.len() {
            let line = lines[i].trim();

            if line.starts_with("[theme]") {
                self.parse_theme(&lines, &mut i, &mut config.theme);
            } else if line.starts_with("[taskbar]") {
                self.parse_taskbar(&lines, &mut i, &mut config.taskbar);
            } else if line.starts_with("[status]") {
                self.parse_status(&lines, &mut i, &mut config.status);
            }

            i += 1;
        }

        config
    }

    fn parse_theme(&self, lines: &[&str], i: &mut usize, theme: &mut ThemeColors) {
        *i += 1;
        while *i < lines.len() {
            let line = lines[*i].trim();

            if line.is_empty() || line.starts_with('[') {
                break;
            }

            if line.starts_with('#') {
                *i += 1;
                continue;
            }

            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim();
                let value = line[eq_pos + 1..].trim();

                if let Some(color) = Self::parse_color(value) {
                    match key {
                        "background" => theme.background = Some(color),
                        "taskbar" => theme.taskbar = Some(color),
                        "text" => theme.text = Some(color),
                        "highlight" => theme.highlight = Some(color),
                        _ => {}
                    }
                } else {
                    let value = Self::unquote(value);
                    if key == "background_style" {
                        if let Some(style) = BackgroundStyle::from_str(&value) {
                            theme.background_style = Some(style);
                        }
                    } else if key == "background_image" && !value.is_empty() {
                        theme.background_image = Some(value);
                    }
                }
            }

            *i += 1;
        }
    }

    fn parse_taskbar(&self, lines: &[&str], i: &mut usize, taskbar: &mut TaskbarConfig) {
        *i += 1;
        while *i < lines.len() {
            let line = lines[*i].trim();

            if line.is_empty() || line.starts_with('[') {
                break;
            }

            if line.starts_with('#') {
                *i += 1;
                continue;
            }

            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim();
                let value = line[eq_pos + 1..].trim();
                let value = Self::unquote(value);

                match key {
                    "height" => {
                        if let Some(h) = Self::parse_u32(&value) {
                            taskbar.height = Some(h);
                        }
                    }
                    "position" => {
                        if let Some(pos) = TaskbarPosition::from_str(&value) {
                            taskbar.position = Some(pos);
                        }
                    }
                    _ => {}
                }
            }

            *i += 1;
        }
    }

    fn parse_status(&self, lines: &[&str], i: &mut usize, status: &mut StatusPreferences) {
        let mut config_version = None;
        let mut order = None;
        let mut visible = None;
        let mut clock_format = None;

        *i += 1;
        while *i < lines.len() {
            let line = lines[*i].trim();

            if line.is_empty() || line.starts_with('[') {
                break;
            }

            if line.starts_with('#') {
                *i += 1;
                continue;
            }

            if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim();
                let value = Self::unquote(line[eq_pos + 1..].trim());

                match key {
                    "config_version" => config_version = Self::parse_u32(&value),
                    "order" => order = Some(value),
                    "visible" => visible = Some(value),
                    "clock_format" => clock_format = Some(value),
                    _ => {}
                }
            }

            *i += 1;
        }

        if config_version != Some(STATUS_PREFERENCES_CONFIG_VERSION) {
            return;
        }

        let defaults = StatusPreferences::default();
        *status = StatusPreferences::from_disk_values(
            order.as_deref().unwrap_or(&defaults.order_csv()),
            visible.as_deref().unwrap_or(&defaults.visible_csv()),
            clock_format
                .as_deref()
                .unwrap_or(defaults.clock_format.as_str()),
        );
    }

    fn unquote(s: &str) -> String {
        let s = s.trim();
        if ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
            && s.len() >= 2
        {
            return s[1..s.len() - 1].to_string();
        }
        s.to_string()
    }

    fn parse_color(s: &str) -> Option<[u8; 3]> {
        let unquoted = Self::unquote(s);
        let s = unquoted.trim();

        if s.starts_with('#') && s.len() == 7 {
            let r = u8::from_str_radix(&s[1..3], 16).ok()?;
            let g = u8::from_str_radix(&s[3..5], 16).ok()?;
            let b = u8::from_str_radix(&s[5..7], 16).ok()?;
            return Some([r, g, b]);
        }

        if s.starts_with('#') && s.len() == 4 {
            let r = u8::from_str_radix(&s[1..2], 16).ok()? * 17;
            let g = u8::from_str_radix(&s[2..3], 16).ok()? * 17;
            let b = u8::from_str_radix(&s[3..4], 16).ok()? * 17;
            return Some([r, g, b]);
        }

        None
    }

    fn parse_u32(s: &str) -> Option<u32> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }

        let mut acc: u32 = 0;
        for ch in s.bytes() {
            if !ch.is_ascii_digit() {
                return None;
            }
            let digit = (ch - b'0') as u32;
            acc = acc.saturating_mul(10).saturating_add(digit);
        }

        Some(acc)
    }
}

pub fn read_config(path: &str) -> Result<String, &'static str> {
    #[cfg(feature = "std")]
    {
        return std::fs::read_to_string(path).map_err(|_| "Failed to read config file");
    }

    #[cfg(not(feature = "std"))]
    use scarlet_std::fs::File;
    #[cfg(not(feature = "std"))]
    {
        let mut file = File::open(path).map_err(|_| "Failed to open config file")?;

        let mut content = String::new();
        let mut buf = [0u8; 4096];

        loop {
            let n = file
                .read(&mut buf)
                .map_err(|_| "Failed to read config file")?;
            if n == 0 {
                break;
            }
            let chunk =
                core::str::from_utf8(&buf[..n]).map_err(|_| "Config file is not valid UTF-8")?;
            content.push_str(chunk);
        }

        Ok(content)
    }
}

#[cfg(feature = "std")]
fn config_filenames(dir_path: &str) -> Result<Vec<String>, &'static str> {
    let entries = std::fs::read_dir(dir_path).map_err(|_| "Failed to read config directory")?;
    let mut filenames = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| "Failed to read config directory entry")?;
        let file_type = entry
            .file_type()
            .map_err(|_| "Failed to read config file type")?;
        let filename = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_file() && filename.ends_with(".toml") {
            filenames.push(filename);
        }
    }
    Ok(filenames)
}

#[cfg(not(feature = "std"))]
fn config_filenames(dir_path: &str) -> Result<Vec<String>, &'static str> {
    use scarlet_std::fs;

    let entries = fs::list_directory(dir_path).map_err(|_| "Failed to read config directory")?;
    let mut filenames = Vec::new();
    for entry in entries {
        if entry.name == "." || entry.name == ".." {
            continue;
        }
        if entry.is_file() && entry.name.ends_with(".toml") {
            filenames.push(entry.name);
        }
    }
    Ok(filenames)
}

pub fn read_config_dir(dir_path: &str) -> Result<String, &'static str> {
    let mut combined_content = String::new();
    let mut toml_files = config_filenames(dir_path)?;
    toml_files.sort();

    for filename in toml_files {
        let file_path = format!("{}/{}", dir_path, filename);
        let content = read_config(&file_path).map_err(|_| "Failed to read config file")?;
        combined_content.push_str(&content);
        combined_content.push('\n');
    }

    Ok(combined_content)
}

pub fn load_desktop_config() -> DesktopConfig {
    let config_dirs = [
        "/etc/scarlet-desktop.d",
        "/system/scarlet/etc/scarlet-desktop.d",
    ];

    for dir in &config_dirs {
        if let Ok(content) = read_config_dir(dir) {
            let parser = ConfigParser::new(content);
            return parser.parse();
        }
    }

    DesktopConfig::default()
}

#[cfg(test)]
mod tests {
    use super::{
        ClockFormat, ConfigParser, STATUS_PREFERENCES_CONFIG_VERSION, StatusItemId,
        StatusPreferences,
    };

    #[test]
    fn status_preferences_default_to_cpu_audio_and_24_hour_clock() {
        assert_eq!(
            StatusPreferences::default(),
            StatusPreferences {
                order: vec![StatusItemId::Cpu, StatusItemId::Audio],
                visible: vec![StatusItemId::Cpu, StatusItemId::Audio],
                clock_format: ClockFormat::TwentyFourHour,
            }
        );
    }

    #[test]
    fn disk_values_discard_unknown_items_and_normalize_missing_items() {
        let preferences = StatusPreferences::from_disk_values(
            "audio,unknown,audio",
            "unknown,audio,audio",
            "not-a-clock",
        );

        assert_eq!(
            preferences.order,
            vec![StatusItemId::Audio, StatusItemId::Cpu]
        );
        assert_eq!(preferences.visible, vec![StatusItemId::Audio]);
        assert_eq!(preferences.clock_format, ClockFormat::TwentyFourHour);
    }

    #[test]
    fn strict_ipc_values_reject_invalid_order_and_clock_format() {
        assert!(StatusPreferences::from_ipc_values("cpu,cpu", "cpu", "24h").is_err());
        assert!(StatusPreferences::from_ipc_values("cpu", "cpu", "24h").is_err());
        assert!(StatusPreferences::from_ipc_values("cpu,network", "cpu", "24h").is_err());
        assert!(StatusPreferences::from_ipc_values("cpu,audio", "cpu,cpu", "24h").is_err());
        assert!(StatusPreferences::from_ipc_values("cpu,audio", "cpu", "invalid").is_err());
    }

    #[test]
    fn strict_ipc_round_trip_preserves_all_supported_values() {
        let preferences = StatusPreferences::from_ipc_values("audio,cpu", "audio", "12h").unwrap();

        assert_eq!(preferences.order_csv(), "audio,cpu");
        assert_eq!(preferences.visible_csv(), "audio");
        assert_eq!(preferences.clock_format, ClockFormat::TwelveHour);
        assert_eq!(
            preferences.to_toml_section(),
            format!(
                "[status]\nconfig_version = {}\norder = \"audio,cpu\"\nvisible = \"audio\"\nclock_format = \"12h\"\n",
                STATUS_PREFERENCES_CONFIG_VERSION
            )
        );
    }

    #[test]
    fn config_parser_reads_the_versioned_status_section() {
        let config = ConfigParser::new(
            "[status]\nconfig_version = 1\norder = \"audio,cpu\"\nvisible = \"cpu\"\nclock_format = \"12h\"\n"
                .into(),
        )
        .parse();

        assert_eq!(
            config.status.order,
            vec![StatusItemId::Audio, StatusItemId::Cpu]
        );
        assert_eq!(config.status.visible, vec![StatusItemId::Cpu]);
        assert_eq!(config.status.clock_format, ClockFormat::TwelveHour);
    }

    #[test]
    fn config_parser_defaults_status_for_missing_or_unknown_versions() {
        let missing = ConfigParser::new("[theme]\nbackground = \"#112233\"\n".into()).parse();
        let unknown = ConfigParser::new(
            "[status]\nconfig_version = 2\norder = \"audio,cpu\"\nvisible = \"audio\"\nclock_format = \"12h\"\n"
                .into(),
        )
        .parse();

        assert_eq!(missing.status, StatusPreferences::default());
        assert_eq!(unknown.status, StatusPreferences::default());
    }

    #[test]
    fn config_parser_normalizes_malformed_status_values_safely() {
        let config = ConfigParser::new(
            "[status]\nconfig_version = 1\norder = \"audio,audio,unknown\"\nvisible = \"unknown,audio,audio\"\nclock_format = \"invalid\"\n"
                .into(),
        )
        .parse();

        assert_eq!(
            config.status.order,
            vec![StatusItemId::Audio, StatusItemId::Cpu]
        );
        assert_eq!(config.status.visible, vec![StatusItemId::Audio]);
        assert_eq!(config.status.clock_format, ClockFormat::TwentyFourHour);
    }
}
