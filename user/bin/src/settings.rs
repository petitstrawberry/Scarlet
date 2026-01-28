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

// Preset colors - Apple system-style palette
#[derive(Clone, Copy, Debug)]
pub struct PresetColor {
    pub name: &'static str,
    pub color: [u8; 3],
}

const PRESET_COLORS: &[PresetColor] = &[
    PresetColor { name: "Gray", color: [174, 174, 178] },
    PresetColor { name: "Space Gray", color: [120, 120, 128] },
    PresetColor { name: "Blue", color: [0, 122, 255] },
    PresetColor { name: "Green", color: [52, 199, 89] },
    PresetColor { name: "Orange", color: [255, 149, 0] },
    PresetColor { name: "Red", color: [255, 59, 48] },
    PresetColor { name: "Purple", color: [175, 82, 222] },
    PresetColor { name: "Teal", color: [90, 200, 250] },
];

#[derive(View, Clone)]
struct SettingsApp {
    red_value: State<f32>,
    green_value: State<f32>,
    blue_value: State<f32>,
}

impl SettingsApp {
    pub fn new() -> Self {
        let config = scarlet_desktop_config::load_desktop_config();
        let color = config.theme.background.unwrap_or(PRESET_COLORS[0].color);
        Self {
            red_value: State::new(StateId::new(1), color[0] as f32),
            green_value: State::new(StateId::new(2), color[1] as f32),
            blue_value: State::new(StateId::new(3), color[2] as f32),
        }
    }

    fn save_config(&self) {
        let bg_color = self.current_color();

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
}

impl Application for SettingsApp {
    fn body(&self) -> impl View {
        let selected_idx = self.selected_preset_index();

        // Prepare state clones
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

        let c = &PRESET_COLORS;
        let is0 = selected_idx == Some(0);
        let is1 = selected_idx == Some(1);
        let is2 = selected_idx == Some(2);
        let is3 = selected_idx == Some(3);
        let is4 = selected_idx == Some(4);
        let is5 = selected_idx == Some(5);
        let is6 = selected_idx == Some(6);
        let is7 = selected_idx == Some(7);

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
                    Rectangle::new()
                        .fill(Color::rgb(c[0].color[0], c[0].color[1], c[0].color[2]))
                        .border(2.0, if is0 { highlight } else { border })
                        .on_click(move || {
                            r0.set(c[0].color[0] as f32);
                            g0.set(c[0].color[1] as f32);
                            b0.set(c[0].color[2] as f32);
                        })
                        .frame(85.0, 85.0),
                    Spacer::new().frame_width(10.0),
                    Rectangle::new()
                        .fill(Color::rgb(c[1].color[0], c[1].color[1], c[1].color[2]))
                        .border(2.0, if is1 { highlight } else { border })
                        .on_click(move || {
                            r1.set(c[1].color[0] as f32);
                            g1.set(c[1].color[1] as f32);
                            b1.set(c[1].color[2] as f32);
                        })
                        .frame(85.0, 85.0),
                    Spacer::new().frame_width(10.0),
                    Rectangle::new()
                        .fill(Color::rgb(c[2].color[0], c[2].color[1], c[2].color[2]))
                        .border(2.0, if is2 { highlight } else { border })
                        .on_click(move || {
                            r2.set(c[2].color[0] as f32);
                            g2.set(c[2].color[1] as f32);
                            b2.set(c[2].color[2] as f32);
                        })
                        .frame(85.0, 85.0),
                    Spacer::new().frame_width(10.0),
                    Rectangle::new()
                        .fill(Color::rgb(c[3].color[0], c[3].color[1], c[3].color[2]))
                        .border(2.0, if is3 { highlight } else { border })
                        .on_click(move || {
                            r3.set(c[3].color[0] as f32);
                            g3.set(c[3].color[1] as f32);
                            b3.set(c[3].color[2] as f32);
                        })
                        .frame(85.0, 85.0),
                },
                Spacer::new().frame_height(10.0),
                hstack! {
                    Rectangle::new()
                        .fill(Color::rgb(c[4].color[0], c[4].color[1], c[4].color[2]))
                        .border(2.0, if is4 { highlight } else { border })
                        .on_click(move || {
                            r4.set(c[4].color[0] as f32);
                            g4.set(c[4].color[1] as f32);
                            b4.set(c[4].color[2] as f32);
                        })
                        .frame(85.0, 85.0),
                    Spacer::new().frame_width(10.0),
                    Rectangle::new()
                        .fill(Color::rgb(c[5].color[0], c[5].color[1], c[5].color[2]))
                        .border(2.0, if is5 { highlight } else { border })
                        .on_click(move || {
                            r5.set(c[5].color[0] as f32);
                            g5.set(c[5].color[1] as f32);
                            b5.set(c[5].color[2] as f32);
                        })
                        .frame(85.0, 85.0),
                    Spacer::new().frame_width(10.0),
                    Rectangle::new()
                        .fill(Color::rgb(c[6].color[0], c[6].color[1], c[6].color[2]))
                        .border(2.0, if is6 { highlight } else { border })
                        .on_click(move || {
                            r6.set(c[6].color[0] as f32);
                            g6.set(c[6].color[1] as f32);
                            b6.set(c[6].color[2] as f32);
                        })
                        .frame(85.0, 85.0),
                    Spacer::new().frame_width(10.0),
                    Rectangle::new()
                        .fill(Color::rgb(c[7].color[0], c[7].color[1], c[7].color[2]))
                        .border(2.0, if is7 { highlight } else { border })
                        .on_click(move || {
                            r7.set(c[7].color[0] as f32);
                            g7.set(c[7].color[1] as f32);
                            b7.set(c[7].color[2] as f32);
                        })
                        .frame(85.0, 85.0),
                },

                Divider::new(),

                // Custom color sliders + Preview
                vstack! {
                    Text::new("Custom Color").font_size(14.0),
                    hstack! {
                        Text::new("R").font_size(12.0).frame_width(20.0),
                        Slider::new(self.red_value.clone()).min(0.0).max(255.0).frame(280.0, 20.0),
                        Text::new(format!("{}", self.red_value.get().max(0.0).min(255.0) as u32)).font_size(12.0).frame_width(36.0),
                    },
                    hstack! {
                        Text::new("G").font_size(12.0).frame_width(20.0),
                        Slider::new(self.green_value.clone()).min(0.0).max(255.0).frame(280.0, 20.0),
                        Text::new(format!("{}", self.green_value.get().max(0.0).min(255.0) as u32)).font_size(12.0).frame_width(36.0),
                    },
                    hstack! {
                        Text::new("B").font_size(12.0).frame_width(20.0),
                        Slider::new(self.blue_value.clone()).min(0.0).max(255.0).frame(280.0, 20.0),
                        Text::new(format!("{}", self.blue_value.get().max(0.0).min(255.0) as u32)).font_size(12.0).frame_width(36.0),
                    },
                    Text::new("Preview").font_size(14.0),
                    Rectangle::new()
                        .fill(Color::rgb(
                            self.current_color()[0],
                            self.current_color()[1],
                            self.current_color()[2],
                        ))
                        .frame(360.0, 70.0),
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
