//! Scarlet Desktop Settings
//!
//! GUI settings application for Scarlet Desktop
//

#![no_std]
#![no_main]

extern crate scarlet_desktop_config;
extern crate scarlet_std as std;

use scarlet_desktop_config::TaskbarPosition;
use scarlet_ui::{
    Application, Button, Color, HStack, Label, Padding, RectView, Spacer, StackAlignment, VStack,
    Window, WindowKind,
};
use std::{format, println, string::String};

fn rgb_to_string(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

fn color_from_rgb(rgb: [u8; 3]) -> Color {
    Color::rgb(rgb[0], rgb[1], rgb[2])
}

fn position_to_string(pos: &TaskbarPosition) -> &'static str {
    match pos {
        TaskbarPosition::Top => "Top",
        TaskbarPosition::Bottom => "Bottom",
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[settings] Starting Scarlet Desktop Settings");

    let mut app = match Application::new() {
        Ok(app) => app,
        Err(e) => {
            println!("[settings] Failed to connect to SWS: {}", e);
            return 1;
        }
    };

    // Load current configuration
    let config = scarlet_desktop_config::load_desktop_config();

    let page_bg = Color::rgb(30, 30, 30);
    let text_color = Color::rgb(224, 224, 224);
    let text_dim = Color::rgb(136, 136, 136);
    let accent_color = Color::rgb(0, 122, 255);

    // Extract theme colors
    let bg_color = config.theme.background.unwrap_or([18, 22, 30]);
    let taskbar_color = config.theme.taskbar.unwrap_or([30, 30, 30]);
    let text_color_rgb = config.theme.text.unwrap_or([224, 224, 224]);
    let highlight_color = config.theme.highlight.unwrap_or([0, 122, 255]);

    // Extract taskbar settings
    let taskbar_height = config.taskbar.height.unwrap_or(28);
    let taskbar_position = config.taskbar.position.unwrap_or(TaskbarPosition::Top);

    // Settings window
    let window = Window::new("Scarlet Settings", 900, 600)
        .min_size(800, 500)
        .background(page_bg)
        .window_type(WindowKind::Normal)
        .content(
            Padding::new(
                HStack::new()
                    .spacing(0)
                    // Left sidebar
                    .child(
                        VStack::new()
                            .spacing(8)
                            .child(
                                // Header
                                Label::new("Settings").color(text_color).font_size(22),
                            )
                            .child(
                                // Selected: Display/Theme
                                HStack::new()
                                    .spacing(10)
                                    .alignment(StackAlignment::Center)
                                    .child(
                                        RectView::new(accent_color)
                                            .width(4)
                                            .height(26)
                                            .corner_radius(2),
                                    )
                                    .child(
                                        Label::new("Display & Theme")
                                            .color(text_color)
                                            .font_size(15),
                                    ),
                            )
                            .child(
                                // Unselected: Menu Bar
                                Label::new("Menu Bar").color(text_dim).font_size(15),
                            )
                            .child(
                                // Unselected: System
                                Label::new("System").color(text_dim).font_size(15),
                            )
                            .child(Spacer::new())
                            .child(
                                // Footer info
                                Label::new("Scarlet Desktop v0.1")
                                    .color(text_dim)
                                    .font_size(12),
                            ),
                    )
                    // Right content - Display & Theme page
                    .child(
                        VStack::new()
                            .spacing(24)
                            .alignment(StackAlignment::Start)
                            .child(
                                Label::new("Display & Theme")
                                    .color(text_color)
                                    .font_size(28),
                            )
                            .child(
                                Label::new("Customize the appearance of Scarlet Desktop")
                                    .color(text_dim)
                                    .font_size(15),
                            )
                            // Theme Colors section
                            .child(
                                VStack::new()
                                    .spacing(12)
                                    .alignment(StackAlignment::Start)
                                    .child(
                                        Label::new("Theme Colors").color(text_color).font_size(18),
                                    )
                                    .child(
                                        // Background color
                                        HStack::new()
                                            .spacing(12)
                                            .alignment(StackAlignment::Center)
                                            .child(
                                                RectView::new(color_from_rgb(bg_color))
                                                    .width(48)
                                                    .height(32)
                                                    .corner_radius(6),
                                            )
                                            .child(
                                                VStack::new()
                                                    .spacing(2)
                                                    .alignment(StackAlignment::Start)
                                                    .child(
                                                        Label::new("Background")
                                                            .color(text_color)
                                                            .font_size(14),
                                                    )
                                                    .child(
                                                        Label::new(&rgb_to_string(bg_color))
                                                            .color(text_dim)
                                                            .font_size(13),
                                                    ),
                                            )
                                            .child(Spacer::new()),
                                    )
                                    .child(
                                        // Taskbar color
                                        HStack::new()
                                            .spacing(12)
                                            .alignment(StackAlignment::Center)
                                            .child(
                                                RectView::new(color_from_rgb(taskbar_color))
                                                    .width(48)
                                                    .height(32)
                                                    .corner_radius(6),
                                            )
                                            .child(
                                                VStack::new()
                                                    .spacing(2)
                                                    .alignment(StackAlignment::Start)
                                                    .child(
                                                        Label::new("Taskbar")
                                                            .color(text_color)
                                                            .font_size(14),
                                                    )
                                                    .child(
                                                        Label::new(&rgb_to_string(taskbar_color))
                                                            .color(text_dim)
                                                            .font_size(13),
                                                    ),
                                            )
                                            .child(Spacer::new()),
                                    )
                                    .child(
                                        // Text color
                                        HStack::new()
                                            .spacing(12)
                                            .alignment(StackAlignment::Center)
                                            .child(
                                                RectView::new(color_from_rgb(text_color_rgb))
                                                    .width(48)
                                                    .height(32)
                                                    .corner_radius(6),
                                            )
                                            .child(
                                                VStack::new()
                                                    .spacing(2)
                                                    .alignment(StackAlignment::Start)
                                                    .child(
                                                        Label::new("Text")
                                                            .color(text_color)
                                                            .font_size(14),
                                                    )
                                                    .child(
                                                        Label::new(&rgb_to_string(text_color_rgb))
                                                            .color(text_dim)
                                                            .font_size(13),
                                                    ),
                                            )
                                            .child(Spacer::new()),
                                    )
                                    .child(
                                        // Highlight color
                                        HStack::new()
                                            .spacing(12)
                                            .alignment(StackAlignment::Center)
                                            .child(
                                                RectView::new(color_from_rgb(highlight_color))
                                                    .width(48)
                                                    .height(32)
                                                    .corner_radius(6),
                                            )
                                            .child(
                                                VStack::new()
                                                    .spacing(2)
                                                    .alignment(StackAlignment::Start)
                                                    .child(
                                                        Label::new("Highlight")
                                                            .color(text_color)
                                                            .font_size(14),
                                                    )
                                                    .child(
                                                        Label::new(&rgb_to_string(highlight_color))
                                                            .color(text_dim)
                                                            .font_size(13),
                                                    ),
                                            )
                                            .child(Spacer::new()),
                                    ),
                            )
                            // Taskbar Settings section
                            .child(
                                VStack::new()
                                    .spacing(12)
                                    .alignment(StackAlignment::Start)
                                    .child(
                                        Label::new("Taskbar Settings")
                                            .color(text_color)
                                            .font_size(18),
                                    )
                                    .child(
                                        HStack::new()
                                            .spacing(12)
                                            .alignment(StackAlignment::Center)
                                            .child(
                                                VStack::new()
                                                    .spacing(2)
                                                    .alignment(StackAlignment::Start)
                                                    .child(
                                                        Label::new("Position")
                                                            .color(text_dim)
                                                            .font_size(13),
                                                    )
                                                    .child(
                                                        Label::new(position_to_string(
                                                            &taskbar_position,
                                                        ))
                                                        .color(text_color)
                                                        .font_size(16),
                                                    ),
                                            )
                                            .child(
                                                VStack::new()
                                                    .spacing(2)
                                                    .alignment(StackAlignment::Start)
                                                    .child(
                                                        Label::new("Height")
                                                            .color(text_dim)
                                                            .font_size(13),
                                                    )
                                                    .child(
                                                        Label::new(&format!(
                                                            "{} px",
                                                            taskbar_height
                                                        ))
                                                        .color(text_color)
                                                        .font_size(16),
                                                    ),
                                            )
                                            .child(Spacer::new()),
                                    ),
                            )
                            // Action buttons
                            .child(
                                HStack::new()
                                    .spacing(12)
                                    .alignment(StackAlignment::Start)
                                    .child(
                                        Button::new("Apply Changes", || {
                                            println!("[settings] Apply changes (stub)");
                                        })
                                        .background(accent_color)
                                        .text_color(Color::WHITE)
                                        .corner_radius(8),
                                    )
                                    .child(
                                        Button::new("Reset to Defaults", || {
                                            println!("[settings] Reset to defaults (stub)");
                                        })
                                        .background(Color::rgb(80, 80, 80))
                                        .text_color(Color::WHITE)
                                        .corner_radius(8),
                                    ),
                            )
                            .child(Spacer::new()),
                    ),
            )
            .all(24),
        );

    if let Err(e) = app.add_window(window) {
        println!("[settings] Failed to add window: {}", e);
        return 1;
    }

    println!("[settings] Running settings app");
    app.run();
}
