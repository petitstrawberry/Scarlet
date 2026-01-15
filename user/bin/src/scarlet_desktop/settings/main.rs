//! Scarlet Desktop Settings
//!
//! GUI settings application for Scarlet Desktop
//

#![no_std]
#![no_main]

extern crate scarlet_desktop_config;
extern crate scarlet_std as std;

use scarlet_desktop_config::{DesktopConfig, TaskbarPosition};
use scarlet_ui::{
    Application, Button, Color, HStack, Label, NavigationItem, NavigationView, Padding, RectView,
    Spacer, StackAlignment, State, VStack, Window, WindowKind,
};
use std::{boxed::Box, format, println, string::String, vec::Vec};

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

/// Build the Display & Theme page content
fn build_display_page(config: &DesktopConfig) -> Box<dyn scarlet_ui::View> {
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

    Box::new(
        Padding::new(
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
                        .child(Label::new("Theme Colors").color(text_color).font_size(18))
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
                                            Label::new("Taskbar").color(text_color).font_size(14),
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
                                        .child(Label::new("Text").color(text_color).font_size(14))
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
                                            Label::new("Highlight").color(text_color).font_size(14),
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
                                        .child(Label::new("Position").color(text_dim).font_size(13))
                                        .child(
                                            Label::new(position_to_string(&taskbar_position))
                                                .color(text_color)
                                                .font_size(16),
                                        ),
                                )
                                .child(
                                    VStack::new()
                                        .spacing(2)
                                        .alignment(StackAlignment::Start)
                                        .child(Label::new("Height").color(text_dim).font_size(13))
                                        .child(
                                            Label::new(&format!("{} px", taskbar_height))
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
        )
        .all(24),
    )
}

/// Build the Menu Bar page content
fn build_menubar_page(config: &DesktopConfig) -> Box<dyn scarlet_ui::View> {
    let text_color = Color::rgb(224, 224, 224);
    let text_dim = Color::rgb(136, 136, 136);
    let accent_color = Color::rgb(0, 122, 255);

    let taskbar_height = config.taskbar.height.unwrap_or(28);
    let taskbar_position = config.taskbar.position.unwrap_or(TaskbarPosition::Top);

    Box::new(
        Padding::new(
            VStack::new()
                .spacing(24)
                .alignment(StackAlignment::Start)
                .child(Label::new("Menu Bar").color(text_color).font_size(28))
                .child(
                    Label::new("Configure the menu bar behavior and appearance")
                        .color(text_dim)
                        .font_size(15),
                )
                .child(
                    VStack::new()
                        .spacing(12)
                        .alignment(StackAlignment::Start)
                        .child(
                            Label::new("Menu Bar Settings")
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
                                        .child(Label::new("Position").color(text_dim).font_size(13))
                                        .child(
                                            Label::new(position_to_string(&taskbar_position))
                                                .color(text_color)
                                                .font_size(16),
                                        ),
                                )
                                .child(
                                    VStack::new()
                                        .spacing(2)
                                        .alignment(StackAlignment::Start)
                                        .child(Label::new("Height").color(text_dim).font_size(13))
                                        .child(
                                            Label::new(&format!("{} px", taskbar_height))
                                                .color(text_color)
                                                .font_size(16),
                                        ),
                                )
                                .child(Spacer::new()),
                        ),
                )
                .child(
                    HStack::new()
                        .spacing(12)
                        .alignment(StackAlignment::Start)
                        .child(
                            Button::new("Apply Changes", || {
                                println!("[settings] Apply menu bar changes (stub)");
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
        )
        .all(24),
    )
}

/// Build the System page content
fn build_system_page() -> Box<dyn scarlet_ui::View> {
    let text_color = Color::rgb(224, 224, 224);
    let text_dim = Color::rgb(136, 136, 136);

    Box::new(
        Padding::new(
            VStack::new()
                .spacing(24)
                .alignment(StackAlignment::Start)
                .child(Label::new("System").color(text_color).font_size(28))
                .child(
                    Label::new("System information and settings")
                        .color(text_dim)
                        .font_size(15),
                )
                .child(
                    VStack::new()
                        .spacing(12)
                        .alignment(StackAlignment::Start)
                        .child(
                            Label::new("Scarlet Desktop")
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
                                        .child(Label::new("Version").color(text_dim).font_size(13))
                                        .child(Label::new("0.1.0").color(text_color).font_size(16)),
                                )
                                .child(
                                    VStack::new()
                                        .spacing(2)
                                        .alignment(StackAlignment::Start)
                                        .child(Label::new("Build").color(text_dim).font_size(13))
                                        .child(
                                            Label::new("2025.01.15")
                                                .color(text_color)
                                                .font_size(16),
                                        ),
                                )
                                .child(Spacer::new()),
                        ),
                )
                .child(Spacer::new()),
        )
        .all(24),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[settings] Starting Scarlet Desktop Settings");

    let mut app = match Application::new() {
        Ok(mut app) => {
            app.app_id("org.scarlet-os.desktop.settings");
            app
        }
        Err(e) => {
            println!("[settings] Failed to connect to SWS: {}", e);
            return 1;
        }
    };

    // Load current configuration
    let config = scarlet_desktop_config::load_desktop_config();

    // Set up navigation state
    let selected_page = State::new(String::from("display"));

    // Build navigation items
    let mut nav_items = Vec::new();
    nav_items.push(NavigationItem::new("display", "Display & Theme"));
    nav_items.push(NavigationItem::new("menubar", "Menu Bar"));
    nav_items.push(NavigationItem::new("system", "System"));

    // Create content builder closure
    let config_clone = config.clone();
    let content_builder = move |page_id: &str| -> Box<dyn scarlet_ui::View> {
        match page_id {
            "display" => build_display_page(&config_clone),
            "menubar" => build_menubar_page(&config_clone),
            "system" => build_system_page(),
            _ => build_display_page(&config_clone),
        }
    };

    // Settings window with NavigationView
    let window = Window::new("Settings", 900, 600)
        .min_size(800, 500)
        .background(Color::rgb(30, 30, 30))
        .window_type(WindowKind::Normal)
        .main_window()
        .content(
            NavigationView::new(selected_page)
                .sidebar_width(220)
                .items(&nav_items)
                .content(content_builder),
        );

    if let Err(e) = app.add_window(window) {
        println!("[settings] Failed to add window: {}", e);
        return 1;
    }

    println!("[settings] Running settings app");
    app.run();
}
