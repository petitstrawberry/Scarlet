//! Scarlet Desktop Settings
//!
//! GUI settings application for Scarlet Desktop

#![no_std]
#![no_main]

extern crate scarlet_desktop_config;
extern crate alloc;

use scarlet_desktop_config::{DesktopConfig, TaskbarPosition};
use scarlet_ui::{
    Application, Window, WindowBuilder,
    VStack, HStack, Spacer,
    Text, Button,
    View, ViewExt,
    Color,
};
use alloc::{string::String, format, boxed::Box};
use scarlet_std::println;

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[settings] Starting Scarlet Desktop Settings");

    let mut app = match Application::new() {
        Ok(mut a) => {
            a.app_id("org.scarlet-os.desktop.settings");
            a
        }
        Err(e) => {
            println!("[settings] Failed to create application: {}", e);
            return 1;
        }
    };

    // Load current configuration
    let config = scarlet_desktop_config::load_desktop_config();

    // Build display & theme tab
    let display_tab = VStack::new()
        .spacing(24)
        .child(
            Text::new("Display & Theme")
                .font_size(28)
        )
        .child(
            Text::new("Customize the appearance of Scarlet Desktop")
                .font_size(15)
        )
        .child(
            VStack::new()
                .spacing(12)
                .child(
                    Text::new("Theme Colors")
                        .font_size(18)
                )
                .child(
                    HStack::new()
                        .spacing(12)
                        .child(
                            Text::new("Background")
                                .font_size(14)
                        )
                        .child(
                            Text::new("#12161e")
                                .font_size(13)
                        )
                        .child(Spacer::new())
                )
                .child(
                    HStack::new()
                        .spacing(12)
                        .child(
                            Text::new("Taskbar")
                                .font_size(14)
                        )
                        .child(
                            Text::new("#1e1e1e")
                                .font_size(13)
                        )
                        .child(Spacer::new())
                )
                .child(
                    HStack::new()
                        .spacing(12)
                        .child(
                            Text::new("Text")
                                .font_size(14)
                        )
                        .child(
                            Text::new("#1e1e1e")
                                .font_size(13)
                        )
                        .child(Spacer::new())
                )
                .child(
                    HStack::new()
                        .spacing(12)
                        .child(
                            Text::new("Highlight")
                                .font_size(14)
                        )
                        .child(
                            Text::new("#007aff")
                                .font_size(13)
                        )
                        .child(Spacer::new())
                )
        )
        .child(
            VStack::new()
                .spacing(12)
                .child(
                    Text::new("Taskbar Settings")
                        .font_size(18)
                )
                .child(
                    HStack::new()
                        .spacing(12)
                        .child(
                            Text::new("Position")
                                .font_size(13)
                        )
                        .child(
                            Text::new("Top")
                                .font_size(16)
                        )
                        .child(
                            Text::new("Height")
                                .font_size(13)
                        )
                        .child(
                            Text::new("28 px")
                                .font_size(16)
                        )
                        .child(Spacer::new())
                )
        )
        .child(
            HStack::new()
                .spacing(12)
                .child(
                    Button::new("Toggle Theme")
                        .action(|| {
                            println!("[settings] Theme toggled");
                        })
                        .padding(10)
                )
                .child(
                    Button::new("Apply Changes")
                        .action(|| {
                            println!("[settings] Apply changes (stub)");
                        })
                        .padding(10)
                )
                .child(
                    Button::new("Reset to Defaults")
                        .action(|| {
                            println!("[settings] Reset to defaults (stub)");
                        })
                        .padding(10)
                )
        )
        .padding(24);

    // Build menu bar tab
    let menubar_tab = VStack::new()
        .spacing(24)
        .child(
            Text::new("Menu Bar")
                .font_size(28)
        )
        .child(
            Text::new("Configure the menu bar behavior and appearance")
                .font_size(15)
        )
        .child(
            VStack::new()
                .spacing(12)
                .child(
                    Text::new("Menu Bar Settings")
                        .font_size(18)
                )
                .child(
                    HStack::new()
                        .spacing(12)
                        .child(
                            Text::new("Position")
                                .font_size(13)
                        )
                        .child(
                            Text::new("Top")
                                .font_size(16)
                        )
                        .child(
                            Text::new("Height")
                                .font_size(13)
                        )
                        .child(
                            Text::new("28 px")
                                .font_size(16)
                        )
                        .child(Spacer::new())
                )
        )
        .child(
            HStack::new()
                .spacing(12)
                .child(
                    Button::new("Apply Changes")
                        .action(|| {
                            println!("[settings] Apply menu bar changes (stub)");
                        })
                        .padding(10)
                )
                .child(
                    Button::new("Reset to Defaults")
                        .action(|| {
                            println!("[settings] Reset to defaults (stub)");
                        })
                        .padding(10)
                )
        )
        .padding(24);

    // Build system tab
    let system_tab = VStack::new()
        .spacing(24)
        .child(
            Text::new("System")
                .font_size(28)
        )
        .child(
            Text::new("System information and settings")
                .font_size(15)
        )
        .child(
            VStack::new()
                .spacing(12)
                .child(
                    Text::new("Scarlet Desktop")
                        .font_size(18)
                )
                .child(
                    HStack::new()
                        .spacing(12)
                        .child(
                            Text::new("Version")
                                .font_size(13)
                        )
                        .child(
                            Text::new("0.1.0")
                                .font_size(16)
                        )
                        .child(
                            Text::new("Build")
                                .font_size(13)
                        )
                        .child(
                            Text::new("2025.01.15")
                                .font_size(16)
                        )
                        .child(Spacer::new())
                )
        )
        .padding(24);

    // Build the UI with VStack (TabView to be implemented)
    let ui_content = VStack::new()
        .spacing(16)
        .child(
            Text::new("Settings")
                .font_size(24)
        )
        .child(
            Text::new("Display & Theme")
                .font_size(18)
        )
        .child(display_tab)
        .child(
            Text::new("Menu Bar")
                .font_size(18)
        )
        .child(menubar_tab)
        .child(
            Text::new("System")
                .font_size(18)
        )
        .child(system_tab);

    let window = Window::builder()
        .title("Settings")
        .size(900, 600)
        .min_size(800, 500)
        .build()
        .content(ui_content);

    if let Err(e) = app.add_window(window) {
        println!("[settings] Failed to add window: {}", e);
        return 1;
    }

    println!("[settings] Running settings app");
    app.run();
}
