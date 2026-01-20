//! Scarlet Desktop Application Launcher
//!
//! Provides a searchable application grid with keyboard navigation
//! and the ability to launch applications from desktop entry files.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::{boxed::Box, string::String, string::ToString, vec, vec::Vec};
use scarlet_std::println;
use scarlet_ui::{
    Application, Button, HStack, Spacer, Text, TextField, VStack, View, ViewExt, Window,
    WindowBuilder,
};

/// Desktop entry file information
#[derive(Debug, Clone)]
struct DesktopEntry {
    name: String,
    exec: String,
    icon: String,
    categories: Vec<String>,
    file_path: String,
}

impl DesktopEntry {
    /// Clean the Exec command by removing field codes
    fn clean_exec(&self) -> String {
        let mut exec = self.exec.clone();

        exec = exec.replace("%f", "");
        exec = exec.replace("%F", "");
        exec = exec.replace("%u", "");
        exec = exec.replace("%U", "");
        exec = exec.replace("%d", "");
        exec = exec.replace("%D", "");
        exec = exec.replace("%n", "");
        exec = exec.replace("%N", "");
        exec = exec.replace("%k", "");
        exec = exec.replace("%v", "");
        exec = exec.replace("%%", "%");

        exec.trim().to_string()
    }

    /// Launch the application
    fn launch(&self) {
        let exec = self.clean_exec();
        println!("[launcher] Launching: {} ({})", self.name, exec);
        self.launch_via_sbus(&self.file_path);
    }

    /// Launch application via sbus (stemd)
    fn launch_via_sbus(&self, app_id: &str) {
        use alloc::sync::Arc;
        use sbus::Argument;

        println!("[launcher] Launching via sbus: {}", app_id);

        // For now, just stub this - full sbus implementation would go here
        println!("[launcher] Would launch: {}", app_id);
    }

    /// Get an emoji icon based on the icon name or category
    fn get_icon_emoji(&self) -> &str {
        if self.icon.contains("terminal") || self.icon.contains("console") {
            return "[TERM]";
        }
        if self.icon.contains("text") || self.icon.contains("editor") {
            return "[EDIT]";
        }
        if self.icon.contains("file") || self.icon.contains("folder") {
            return "[FILE]";
        }
        if self.icon.contains("setting") || self.icon.contains("preference") {
            return "[SET]";
        }
        if self.icon.contains("network") || self.icon.contains("wifi") {
            return "[NET]";
        }
        if self.icon.contains("audio") || self.icon.contains("sound") {
            return "[AUDIO]";
        }
        if self.icon.contains("video") || self.icon.contains("camera") {
            return "[VIDEO]";
        }
        if self.icon.contains("image") || self.icon.contains("photo") {
            return "[IMG]";
        }
        if self.icon.contains("calc") || self.icon.contains("math") {
            return "[CALC]";
        }
        if self.icon.contains("clock") || self.icon.contains("time") {
            return "[TIME]";
        }
        if self.icon.contains("game") {
            return "[GAME]";
        }

        if self.categories.iter().any(|c| c.contains("System")) {
            return "[SYS]";
        }
        if self.categories.iter().any(|c| c.contains("Utility")) {
            return "[UTIL]";
        }
        if self.categories.iter().any(|c| c.contains("Development")) {
            return "[DEV]";
        }
        if self.categories.iter().any(|c| c.contains("Network")) {
            return "[NET]";
        }
        if self.categories.iter().any(|c| c.contains("Audio")) {
            return "[AUDIO]";
        }
        if self.categories.iter().any(|c| c.contains("Video")) {
            return "[VIDEO]";
        }
        if self.categories.iter().any(|c| c.contains("Game")) {
            return "[GAME]";
        }

        "[APP]"
    }

    /// Get category name for display
    fn get_display_category(&self) -> &str {
        if self.categories.is_empty() {
            return "Application";
        }

        for category in &self.categories {
            match category.as_str() {
                "System" => return "System",
                "Utility" => return "Utility",
                "Development" => return "Development",
                "Network" => return "Network",
                "Audio" => return "Audio",
                "Video" => return "Video",
                "Graphics" => return "Graphics",
                "Office" => return "Office",
                "Game" => return "Games",
                "Education" => return "Education",
                _ => continue,
            }
        }

        "Application"
    }
}

/// Parse a .desktop file from the filesystem
fn parse_desktop_file(content: &str, file_path: String) -> Option<DesktopEntry> {
    let mut name = String::new();
    let mut exec = String::new();
    let mut icon = String::new();
    let mut categories = Vec::new();
    let mut in_desktop_entry = false;

    for line in content.lines() {
        let line = line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line == "[Desktop Entry]" {
            in_desktop_entry = true;
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            if in_desktop_entry {
                break;
            }
            continue;
        }

        if let Some(eq_pos) = line.find('=') {
            let key = &line[..eq_pos];
            let value = &line[eq_pos + 1..];

            match key {
                "Name" => name = value.to_string(),
                "Exec" => exec = value.to_string(),
                "Icon" => icon = value.to_string(),
                "Categories" => {
                    categories = value
                        .split(';')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .collect();
                }
                _ => {}
            }
        }
    }

    if name.is_empty() || exec.is_empty() {
        println!(
            "[launcher] Invalid desktop file {}: missing Name or Exec",
            file_path
        );
        return None;
    }

    Some(DesktopEntry {
        name,
        exec,
        icon,
        categories,
        file_path,
    })
}

/// Load all desktop entries from the apps directory
fn load_desktop_entries() -> Vec<DesktopEntry> {
    // For now, return a hardcoded list
    // In a real implementation, this would read from /system/scarlet/etc/stemd.d/apps
    vec![
        DesktopEntry {
            name: String::from("Notepad"),
            exec: String::from("scarlet_desktop_notepad"),
            icon: String::from("text-editor"),
            categories: vec![String::from("Utility")],
            file_path: String::from("scarlet_desktop_notepad"),
        },
        DesktopEntry {
            name: String::from("Settings"),
            exec: String::from("scarlet_desktop_settings"),
            icon: String::from("preferences-system"),
            categories: vec![String::from("System")],
            file_path: String::from("scarlet_desktop_settings"),
        },
        DesktopEntry {
            name: String::from("Filer"),
            exec: String::from("scarlet_desktop_filer"),
            icon: String::from("file-manager"),
            categories: vec![String::from("System")],
            file_path: String::from("scarlet_desktop_filer"),
        },
        DesktopEntry {
            name: String::from("Terminal"),
            exec: String::from("scarlet_desktop_terminal"),
            icon: String::from("terminal"),
            categories: vec![String::from("System")],
            file_path: String::from("scarlet_desktop_terminal"),
        },
    ]
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[launcher] Starting Scarlet Desktop Application Launcher");

    let all_entries = load_desktop_entries();

    if all_entries.is_empty() {
        println!("[launcher] No desktop entries found!");
    }

    let mut app = match Application::new() {
        Ok(mut a) => {
            a.app_id("org.scarlet-os.desktop.launcher");
            a
        }
        Err(e) => {
            println!("[launcher] Failed to create application: {}", e);
            return 1;
        }
    };

    // Build app list UI
    let mut app_list_ui = VStack::new().spacing(0);

    for entry in &all_entries {
        let entry_clone = entry.clone();
        let entry_item = HStack::new()
            .spacing(16)
            .child(Text::new(entry.get_icon_emoji()).font_size(20))
            .child(
                VStack::new()
                    .spacing(2)
                    .child(Text::new(&entry.name).font_size(16))
                    .child(Text::new(entry.get_display_category()).font_size(12)),
            )
            .child(Spacer::new())
            .background(scarlet_ui::Color::rgb(50, 50, 50))
            .padding(13);
        app_list_ui = app_list_ui.child(entry_item);
    }

    let ui_content = VStack::new()
        .spacing(0)
        .child(
            VStack::new()
                .spacing(16)
                .child(Text::new("Applications").font_size(28))
                .child(
                    TextField::new()
                        .placeholder("Search applications...")
                        .padding(12),
                )
                .padding(24),
        )
        .child(
            HStack::new()
                .background(scarlet_ui::Color::rgb(200, 200, 200))
                .frame(1, 1),
        )
        .child(app_list_ui.padding(8))
        .child(Spacer::new())
        .child(
            HStack::new()
                .background(scarlet_ui::Color::rgb(200, 200, 200))
                .frame(1, 1),
        )
        .child(
            Text::new("Type to search • Click to launch")
                .font_size(12)
                .padding(16),
        );

    let window = Window::builder()
        .title("Applications")
        .size(700, 550)
        .min_size(600, 400)
        .build()
        .content(ui_content);

    if let Err(e) = app.add_window(window) {
        println!("[launcher] Failed to add window: {:?}", e);
        return 1;
    }

    println!("[launcher] Running application loop");
    app.run();
}
