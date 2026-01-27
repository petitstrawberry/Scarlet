//! Scarlet Desktop Settings
//!
//! Modern settings application for Scarlet Desktop

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std;
extern crate scarlet_ui_macros;

use scarlet_std::format;
use scarlet_std::println;
use scarlet_std::fs;
use scarlet_ui::{hstack, prelude::*, vstack, State, StateId};
use scarlet_ui_macros::View;

// Modern preset colors - Tailwind CSS inspired palette
#[derive(Clone, Copy, Debug)]
pub struct PresetColor {
    pub name: &'static str,
    pub color: [u8; 3],
}

const PRESET_COLORS: &[PresetColor] = &[
    PresetColor { name: "Midnight", color: [15, 23, 42] },
    PresetColor { name: "Slate", color: [30, 41, 59] },
    PresetColor { name: "Ocean", color: [8, 51, 68] },
    PresetColor { name: "Forest", color: [20, 83, 45] },
    PresetColor { name: "Emerald", color: [6, 95, 70] },
    PresetColor { name: "Violet", color: [76, 29, 149] },
    PresetColor { name: "Rose", color: [88, 28, 135] },
    PresetColor { name: "Charcoal", color: [24, 24, 27] },
];

#[derive(View, Clone)]
struct SettingsApp {
    selected_color_index: State<usize>,
    red_value: State<f32>,
    green_value: State<f32>,
    blue_value: State<f32>,
}

impl SettingsApp {
    pub fn new() -> Self {
        let config = scarlet_desktop_config::load_desktop_config();
        let mut selected_idx = 0;

        if let Some(bg) = config.theme.background {
            for (i, preset) in PRESET_COLORS.iter().enumerate() {
                if preset.color == bg {
                    selected_idx = i;
                    break;
                }
            }
            let color = config.theme.background.unwrap_or(PRESET_COLORS[0].color);
            Self {
                selected_color_index: State::new(StateId::new(0), selected_idx),
                red_value: State::new(StateId::new(1), color[0] as f32),
                green_value: State::new(StateId::new(2), color[1] as f32),
                blue_value: State::new(StateId::new(3), color[2] as f32),
            }
        } else {
            let c = PRESET_COLORS[0].color;
            Self {
                selected_color_index: State::new(StateId::new(0), 0),
                red_value: State::new(StateId::new(1), c[0] as f32),
                green_value: State::new(StateId::new(2), c[1] as f32),
                blue_value: State::new(StateId::new(3), c[2] as f32),
            }
        }
    }

    fn save_config(&self) {
        let bg_color = PRESET_COLORS[self.selected_color_index.get()].color;

        let config_content = format!(
            "[theme]\nbackground = \"#{:02x}{:02x}{:02x}\"\n",
            bg_color[0], bg_color[1], bg_color[2]
        );

        let _ = fs::create_directory("/etc/scarlet-desktop.d");

        match fs::File::create("/etc/scarlet-desktop.d/background.toml") {
            Ok(mut file) => {
                match file.write(config_content.as_bytes()) {
                    Ok(_) => println!("[settings] Saved: {:02x}{:02x}{:02x}", bg_color[0], bg_color[1], bg_color[2]),
                    Err(e) => println!("[settings] Write error: {:?}", e),
                }
            }
            Err(e) => println!("[settings] Create error: {:?}", e),
        }
    }
}

impl Application for SettingsApp {
    fn body(&self) -> impl View {
        let selected_idx = self.selected_color_index.get();

        // Prepare state clones
        let s0 = self.selected_color_index.clone();
        let s1 = self.selected_color_index.clone();
        let s2 = self.selected_color_index.clone();
        let s3 = self.selected_color_index.clone();
        let s4 = self.selected_color_index.clone();
        let s5 = self.selected_color_index.clone();
        let s6 = self.selected_color_index.clone();
        let s7 = self.selected_color_index.clone();

        let c = &PRESET_COLORS;
        let is0 = selected_idx == 0;
        let is1 = selected_idx == 1;
        let is2 = selected_idx == 2;
        let is3 = selected_idx == 3;
        let is4 = selected_idx == 4;
        let is5 = selected_idx == 5;
        let is6 = selected_idx == 6;
        let is7 = selected_idx == 7;

        // Highlight color - modern blue
        let highlight = Color::rgb(59, 130, 246);
        let border = Color::rgb(51, 65, 85);

        Window::new(
            "Settings",
            vstack! {
                // Modern header
                Text::new("Appearance").font_size(28.0),
                Text::new("Desktop Background").font_size(13.0),
                Divider::new(),

                // Color grid - 2 rows of 4
                hstack! {
                    Button::new("").background_color(Color::rgb(c[0].color[0], c[0].color[1], c[0].color[2]))
                        .border_color(if is0 { highlight } else { border }).on_click(move || { s0.set(0); }).frame(85.0, 85.0),
                    Spacer::new().frame_width(10.0),
                    Button::new("").background_color(Color::rgb(c[1].color[0], c[1].color[1], c[1].color[2]))
                        .border_color(if is1 { highlight } else { border }).on_click(move || { s1.set(1); }).frame(85.0, 85.0),
                    Spacer::new().frame_width(10.0),
                    Button::new("").background_color(Color::rgb(c[2].color[0], c[2].color[1], c[2].color[2]))
                        .border_color(if is2 { highlight } else { border }).on_click(move || { s2.set(2); }).frame(85.0, 85.0),
                    Spacer::new().frame_width(10.0),
                    Button::new("").background_color(Color::rgb(c[3].color[0], c[3].color[1], c[3].color[2]))
                        .border_color(if is3 { highlight } else { border }).on_click(move || { s3.set(3); }).frame(85.0, 85.0),
                },
                Spacer::new().frame_height(10.0),
                hstack! {
                    Button::new("").background_color(Color::rgb(c[4].color[0], c[4].color[1], c[4].color[2]))
                        .border_color(if is4 { highlight } else { border }).on_click(move || { s4.set(4); }).frame(85.0, 85.0),
                    Spacer::new().frame_width(10.0),
                    Button::new("").background_color(Color::rgb(c[5].color[0], c[5].color[1], c[5].color[2]))
                        .border_color(if is5 { highlight } else { border }).on_click(move || { s5.set(5); }).frame(85.0, 85.0),
                    Spacer::new().frame_width(10.0),
                    Button::new("").background_color(Color::rgb(c[6].color[0], c[6].color[1], c[6].color[2]))
                        .border_color(if is6 { highlight } else { border }).on_click(move || { s6.set(6); }).frame(85.0, 85.0),
                    Spacer::new().frame_width(10.0),
                    Button::new("").background_color(Color::rgb(c[7].color[0], c[7].color[1], c[7].color[2]))
                        .border_color(if is7 { highlight } else { border }).on_click(move || { s7.set(7); }).frame(85.0, 85.0),
                },

                Divider::new(),

                // Preview
                {
                    let c = PRESET_COLORS[selected_idx].color;
                    vstack! {
                        Text::new("Preview").font_size(14.0),
                        Rectangle::new().fill(Color::rgb(c[0], c[1], c[2])).frame(360.0, 100.0)
                    }
                },

                Divider::new(),

                // Actions
                hstack! {
                    Spacer::new(),
                    Button::new("Apply").on_click({
                        let app = self.clone();
                        move || { app.save_config(); println!("[settings] Applied"); }
                    }),
                    Spacer::new().frame_width(12.0),
                    Button::new("Close").on_click(|| { println!("[settings] Close"); }),
                    Spacer::new(),
                },
            }
            .frame(f32::INFINITY, f32::INFINITY)
        )
        .app_id("org.scarlet-os.desktop.settings")
        .size(Size::new(420.0, 520.0))
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
