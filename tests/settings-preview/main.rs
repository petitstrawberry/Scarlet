//! Winit visual fixture for the production Settings pages. No native services,
//! filesystem writes, audio, display, desktop or time-zone changes are reachable.
#[cfg(target_os = "scarlet")]
compile_error!(
    "Settings visual fixtures are host-only; use the production Settings app on Scarlet"
);

use scarlet_desktop_config::{BackgroundStyle, ClockFormat, StatusItemId, StatusPreferences};
use scarlet_ui::{HeaderBar, NavigationLink, State, hstack, navigation, prelude::*, vstack};
use scarlet_ui_macros::View;
#[path = "../../user/bin/src/settings_views.rs"]
mod settings_views;
use settings_views::*;
#[derive(Clone, Debug)]
struct CursorThemeInfo {
    name: String,
}
#[derive(Clone, Debug)]
struct InputMethodInfo {
    name: String,
}
type OutputInfo = String;
#[derive(Clone, Copy)]
struct PresetColor {
    name: &'static str,
    color: [u8; 3],
}
const PRESET_COLORS: &[PresetColor] = &[
    PresetColor {
        name: "Default",
        color: [40, 40, 50],
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
fn f32_to_percent(v: f32) -> u32 {
    v.clamp(0.0, 100.0).round() as u32
}
fn audio_output_labels(outputs: &[OutputInfo]) -> Vec<String> {
    outputs.to_vec()
}
fn input_method_labels(methods: &[InputMethodInfo]) -> Vec<String> {
    methods.iter().map(|m| m.name.clone()).collect()
}
fn enumerate_cities(region: &str) -> Vec<String> {
    match region {
        "Asia" => vec!["Tokyo".into(), "Singapore".into()],
        _ => vec!["UTC".into()],
    }
}
fn system_description() -> (String, String, String, String) {
    (
        "Scarlet — fixture data".into(),
        "Preview version".into(),
        "Scarlet fixture kernel".into(),
        "aarch64-unknown-scarlet".into(),
    )
}
#[allow(dead_code)]
#[derive(Clone, View)]
struct SettingsApp {
    background_style_index: State<usize>,
    background_preset_index: State<usize>,
    background_status: State<String>,
    background_retry: State<u64>,
    output_scale_index: State<usize>,
    output_scale_last_good: State<usize>,
    output_scale_status: State<String>,
    timezone_active: State<String>,
    timezone_status: State<String>,
    clock_format_index: State<usize>,
    cpu_visible: State<bool>,
    audio_visible: State<bool>,
    audio_last_good: State<(f32, bool, usize)>,
    input_status: State<String>,

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
    fn fixture(empty: bool) -> Self {
        let app = Self::default();
        app.red_value.set(40.0);
        app.green_value.set(40.0);
        app.blue_value.set(50.0);
        app.background_image_label
            .set("No wallpaper selected".into());
        app.background_status
            .set("Fixture: changes stay in this preview".into());
        app.display_brightness_percent.set(72.0);
        app.display_brightness_status
            .set("Fixture brightness: 72%".into());
        app.output_scale_status
            .set("Changes apply after restarting the desktop".into());
        app.timezone_active.set("Asia/Tokyo".into());
        app.timezone_regions.set(vec!["Asia".into(), "Etc".into()]);
        app.timezone_cities.set(enumerate_cities("Asia"));
        app.audio_volume_percent.set(65.0);
        app.navigation_title.set("Appearance".into());
        app.cpu_visible.set(true);
        app.audio_visible.set(true);
        app.cursor_theme_status.set("Fixture cursor theme".into());
        app.audio_status.set("Fixture output connected".into());
        if empty {
            app.audio_status
                .set("Audio service unavailable. Check the connection and refresh.".into());
            app.cursor_theme_status
                .set("No cursor themes found. Install a theme and refresh.".into());
            app.input_status.set("No input methods registered".into());
            app.background_status.set(
                "Could not save desktop background. Retry when the settings service is available."
                    .into(),
            );
        } else {
            app.cursor_themes.set(vec![CursorThemeInfo {
                name: "Default".into(),
            }]);
            app.audio_outputs
                .set(vec!["Built-in audio".into(), "External speakers".into()]);
            app.input_methods.set(vec![
                InputMethodInfo {
                    name: "Direct input".into(),
                },
                InputMethodInfo {
                    name: "Japanese".into(),
                },
            ]);
        }
        app
    }
    fn current_color(&self) -> [u8; 3] {
        [
            self.red_value.get() as u8,
            self.green_value.get() as u8,
            self.blue_value.get() as u8,
        ]
    }
    fn sync_background_preset(&self) {
        self.background_preset_index.set(
            PRESET_COLORS
                .iter()
                .position(|p| p.color == self.current_color())
                .unwrap_or(PRESET_COLORS.len()),
        );
    }
    fn open_background_picker(&self) {
        self.background_status
            .set("Fixture: image picker is unavailable; no image was opened".into());
    }
    fn clear_background_image(&self) {
        self.background_image.set(None);
        self.background_image_label
            .set("No wallpaper selected".into());
    }
    fn save_config(&self) {
        self.background_status
            .set("Fixture: background change kept in memory only".into());
    }
    fn set_display_brightness_percent(&self, value: u32) {
        self.display_brightness_status
            .set(format!("Fixture brightness: {value}%"));
    }
    fn set_output_scale(&self, _: usize) {
        self.output_scale_status
            .set("Fixture: no display configuration was changed".into());
    }
    fn select_cursor_theme(&self, _: usize) {
        self.cursor_theme_status
            .set("Fixture: cursor selection stays in this preview".into());
    }
    fn refresh_cursor_themes(&self) {
        self.cursor_theme_status
            .set("Fixture: no installed themes were read".into());
    }
    fn select_audio_output(&self, _: usize) {
        self.audio_status
            .set("Fixture: output selection stays in this preview".into());
    }
    fn refresh_audio(&self) {
        self.audio_status
            .set("Fixture: no audio service was contacted".into());
    }
    fn set_audio_volume_percent(&self, v: u32) {
        self.audio_status.set(format!("Fixture volume: {v}%"));
    }
    fn set_audio_muted(&self, m: bool) {
        self.audio_status.set(format!("Fixture mute: {m}"));
    }
    fn select_input_method(&self, _: usize) {
        self.input_status
            .set("Fixture: no input service was contacted".into());
    }
    fn set_status_item_visible(&self, item: StatusItemId, visible: bool) {
        let mut p = self.status_preferences.get();
        p.visible.retain(|x| *x != item);
        if visible {
            p.visible.push(item);
        }
        self.status_preferences.set(p);
    }
    fn move_status_item(&self, item: StatusItemId, delta: isize) {
        let mut p = self.status_preferences.get();
        if let Some(i) = p.order.iter().position(|x| *x == item) {
            let j = (i as isize + delta).clamp(0, p.order.len() as isize - 1) as usize;
            p.order.swap(i, j);
            self.status_preferences.set(p);
        }
    }
    fn apply_timezone(&self) {
        self.timezone_status
            .set("Fixture: system time zone was not changed".into());
    }
    fn set_status_clock_format(&self, format: ClockFormat) {
        let mut p = self.status_preferences.get();
        p.clock_format = format;
        self.status_preferences.set(p);
    }
    fn refresh_status_preferences(&self) {
        self.status_preferences_status
            .set("Fixture: status preferences stay in this preview".into());
    }
    fn reset_status_preferences(&self) {
        self.status_preferences.set(StatusPreferences::default());
        self.cpu_visible.set(true);
        self.audio_visible.set(true);
        self.clock_format_index.set(0);
    }
}
impl Application for SettingsApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new("main", Window::new("Settings — fixture preview", vstack! {
            Text::new("FIXTURE PREVIEW · Winit · no system settings are changed").font_size(14.0).padding(8.0).frame_width(f32::INFINITY).background(Color::rgb(255,245,220)),
            settings_navigation(self.clone()).frame(f32::INFINITY,f32::INFINITY),
        }.spacing(0.0).frame(f32::INFINITY,f32::INFINITY)).size(Size::new(800.0,634.0)).min_size(Size::new(800.0,574.0)).resizable(true))
    }
    fn debug_logging(&self) -> bool {
        false
    }
}
fn main() {
    let mut app = SettingsApp::fixture(std::env::args().any(|a| a == "--empty"));
    app.run().unwrap();
}
