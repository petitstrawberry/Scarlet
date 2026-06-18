//! Scarlet Desktop Settings
//!
//! Modern settings application for Scarlet Desktop

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std;
extern crate scarlet_ui_macros;

use core::f32;

use scarlet_desktop_config::BackgroundStyle;
use scarlet_std::format;
use scarlet_std::fs;
use scarlet_std::println;
use scarlet_std::string::String;
use scarlet_std::vec::Vec;
use scarlet_ui::{
    Icon, NavigationLink, State, StateId, hstack, navigation, prelude::*, vstack, zstack,
};
use scarlet_ui_macros::View;
use sws_client::{Connection, InputMethodInfo};

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

fn save_output_scale_config(scale_milli: u32) {
    let config_content = match scale_milli {
        1000 => "[output]\nscale = 1.0\n",
        _ => "[output]\nscale = 2.0\n",
    };

    let _ = fs::create_directory(SWS_CONFIG_DIR);

    match fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(SWS_CONFIG_PATH)
    {
        Ok(mut file) => match file.write(config_content.as_bytes()) {
            Ok(_) => println!("[settings] Saved output scale: {}", scale_milli),
            Err(e) => println!("[settings] Write SWS config error: {:?}", e),
        },
        Err(e) => println!("[settings] Create SWS config error: {:?}", e),
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
    red_value: State<f32>,
    green_value: State<f32>,
    blue_value: State<f32>,
    input_methods: State<Vec<InputMethodInfo>>,
    selected_ime_index: State<usize>,
    timezone_regions: State<Vec<String>>,
    timezone_region_index: State<usize>,
    timezone_cities: State<Vec<String>>,
    timezone_city_index: State<usize>,
}

impl SettingsApp {
    pub fn new() -> Self {
        let config = scarlet_desktop_config::load_desktop_config();
        let style = config.theme.background_style.unwrap_or(DEFAULT_STYLE);
        let color = config.theme.background.unwrap_or(DEFAULT_BG_PREVIEW);
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
        Self {
            background_style: State::new(StateId::new(0), style),
            red_value: State::new(StateId::new(1), color[0] as f32),
            green_value: State::new(StateId::new(2), color[1] as f32),
            blue_value: State::new(StateId::new(3), color[2] as f32),
            input_methods: State::new(StateId::new(4), input_methods),
            selected_ime_index: State::new(StateId::new(5), selected_ime_index),
            timezone_regions: State::new(StateId::new(6), timezone_regions),
            timezone_region_index: State::new(StateId::new(7), region_index),
            timezone_cities: State::new(StateId::new(8), timezone_cities),
            timezone_city_index: State::new(StateId::new(9), city_index),
        }
    }

    fn save_config(&self) {
        let bg_color = self.current_color();
        let style = self.background_style.get();
        if style == DEFAULT_STYLE && bg_color == DEFAULT_BG_PREVIEW {
            let _ = fs::remove_file("/etc/scarlet-desktop.d/background.toml");
            println!("[settings] Reset to default background");
            return;
        }

        let config_content = format!(
            "[theme]\nbackground = \"#{:02x}{:02x}{:02x}\"\nbackground_style = \"{}\"\n",
            bg_color[0],
            bg_color[1],
            bg_color[2],
            style.as_str()
        );

        let _ = fs::create_directory("/etc/scarlet-desktop.d");

        match fs::File::create("/etc/scarlet-desktop.d/background.toml") {
            Ok(mut file) => match file.write(config_content.as_bytes()) {
                Ok(_) => println!(
                    "[settings] Saved: {:02x}{:02x}{:02x}",
                    bg_color[0], bg_color[1], bg_color[2]
                ),
                Err(e) => println!("[settings] Write error: {:?}", e),
            },
            Err(e) => println!("[settings] Create error: {:?}", e),
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

    fn select_input_method(&self, index: usize) {
        let methods = self.input_methods.get();
        let Some(method) = methods.get(index) else {
            println!("[settings] Invalid input method index: {}", index);
            return;
        };

        match Connection::connect_default() {
            Ok(mut connection) => match connection.set_active_input_method(method.ime_id) {
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
}

fn load_input_methods() -> Vec<InputMethodInfo> {
    match Connection::connect_default() {
        Ok(mut connection) => match connection.get_input_methods() {
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
        Text::new("Appearance").font_size(28.0),
        Text::new("Desktop Background").font_size(13.0),
        Divider::new(),

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

fn about_page() -> impl View {
    vstack! {
        Text::new("About").font_size(28.0),
        Text::new("Scarlet Desktop").font_size(13.0),
        Divider::new(),

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

fn network_page() -> impl View {
    vstack! {
        Text::new("Network").font_size(28.0),
        Text::new("Network Settings").font_size(13.0),
        Divider::new(),

        vstack! {
            Text::new("Coming Soon").font_size(20.0),
            Text::new("Network configuration will be available here").font_size(13.0),
        }
        .padding(40.0),
    }
    .padding(10.0)
    .frame(f32::INFINITY, f32::INFINITY)
}

fn display_page() -> impl View {
    vstack! {
        Text::new("Display").font_size(28.0),
        Text::new("Output").font_size(13.0),
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
        Text::new("Input").font_size(28.0),
        Text::new("Input Method").font_size(13.0),
        Divider::new(),

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

fn datetime_page(
    regions: Vec<String>,
    region_idx: State<usize>,
    cities_state: State<Vec<String>>,
    city_idx: State<usize>,
) -> impl View {
    let cities = cities_state.get();
    let current_region = regions.get(region_idx.get()).cloned().unwrap_or_default();
    let current_city = cities.get(city_idx.get()).cloned().unwrap_or_default();
    let cities_state2 = cities_state.clone();
    let city_idx2 = city_idx.clone();
    let region_idx2 = region_idx.clone();
    let cities2 = cities.clone();
    let regions2 = regions.clone();

    vstack! {
        Text::new("Date & Time").font_size(28.0),
        Text::new("Timezone").font_size(13.0),
        Divider::new(),

        vstack! {
            hstack! {
                Text::new("Region").font_size(13.0).frame_width(120.0),
                Select::new(regions.clone(), region_idx.clone())
                    .width(250.0)
                    .on_change(move |index| {
                        if let Some(r) = regions.get(index) {
                            let new_cities = enumerate_cities(r);
                            cities_state.set(new_cities);
                            city_idx.set(0);
                        }
                        region_idx.set(index);
                    }),
            }
            .padding(10.0),
            hstack! {
                Text::new("City").font_size(13.0).frame_width(120.0),
                Select::new(cities2, city_idx2.clone())
                    .width(250.0)
                    .on_change(move |index| {
                        if let (Some(r), Some(c)) = (
                            regions2.get(region_idx2.get()),
                            cities_state2.get().get(index),
                        ) {
                            let zone = format!("{}/{}", r, c);
                            save_timezone(&zone);
                        }
                        city_idx2.set(index);
                    }),
            }
            .padding(10.0),
            Text::new(format!("Current: {}/{}", current_region, current_city)).font_size(12.0),
        }
        .padding(10.0),
    }
    .padding(10.0)
    .frame(f32::INFINITY, f32::INFINITY)
}

impl Application for SettingsApp {
    fn scenes(&self) -> impl Scene {
        let app = self.clone();
        let input_app = self.clone();
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

        WindowGroup::new("main", Window::new(
            "Settings",
            navigation! {
                NavigationLink::new("Appearance", Icon::Home, move || {
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
                }),
                NavigationLink::new("Display", Icon::Search, display_page),
                NavigationLink::new("Input", Icon::Settings, move || input_page(input_app.clone())),
                NavigationLink::new("Date & Time", Icon::Search, move || {
                    datetime_page(
                        tz_regions.clone(),
                        tz_region_idx.clone(),
                        tz_cities.clone(),
                        tz_city_idx.clone(),
                    )
                }),
                NavigationLink::new("About", Icon::Info, about_page),
            }
            .sidebar_width(150.0)
            .frame(f32::INFINITY, f32::INFINITY),
        )
        .app_id("org.scarlet-os.desktop.settings")
        .size(Size::new(800.0, 600.0)))
    }

    fn debug_logging(&self) -> bool {
        false
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
