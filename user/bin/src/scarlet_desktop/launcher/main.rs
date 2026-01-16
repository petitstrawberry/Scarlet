//! Scarlet Desktop Application Launcher
//!
//! Provides a searchable application grid with keyboard navigation
//! and the ability to launch applications from desktop entry files.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{
    Application, Color, Event, EventKind, Label, Padding, RectView, Size, Spacer, StackAlignment,
    State, TextField, VStack, View, ViewRefreshHandle, Window, design,
};
use std::{format, println, string::String, string::ToString, vec::Vec};

use design::Palette;

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
        use sbus::Argument;
        use sbus_client;

        println!("[launcher] Launching via sbus: {}", app_id);

        let mut conn = match sbus_client::Connection::connect() {
            Ok(c) => c,
            Err(e) => {
                println!("[launcher] Failed to connect to sbus: {:?}", e);
                self.spawn_direct();
                return;
            }
        };

        let app_id_string = String::from(app_id);
        let mut args = Vec::new();
        args.push(Argument::String(app_id_string));

        match conn.call_method(
            "org.scarlet-os.stemd",
            "/org/scarlet/stemd",
            "org.scarlet-os.stemd",
            "LaunchOrFocus",
            args,
        ) {
            Ok(result) => {
                if !result.is_empty() {
                    if let Argument::String(ref s) = result[0] {
                        println!("[launcher] sbus response: {}", s);
                    }
                }
            }
            Err(e) => {
                println!("[launcher] Failed to call LaunchOrFocus: {:?}", e);
                self.spawn_direct();
            }
        }
    }

    /// Fallback: spawn process directly
    fn spawn_direct(&self) {
        use std::task;

        let exec = self.clean_exec();
        let parts: Vec<&str> = exec.split_whitespace().collect();
        if parts.is_empty() {
            return;
        }

        let cmd = parts[0];
        let args = &parts[1..];

        println!("[launcher] Spawning directly: {} {:?}", cmd, args);

        match task::fork() {
            0 => {
                let mut argv: Vec<&str> = Vec::new();
                argv.push(cmd);
                for arg in args {
                    argv.push(arg);
                }
                let envp: &[&str] = &[];
                let _ = task::execve(cmd, &argv, envp);
                task::exit(1);
            }
            pid if pid > 0 => {
                println!("[launcher] Spawned process with PID: {}", pid);
            }
            _ => {
                println!("[launcher] Failed to fork");
            }
        }
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
    let apps_dir = "/system/scarlet/etc/stemd.d/apps";
    let mut entries = Vec::new();

    println!("[launcher] Loading desktop entries from {}", apps_dir);

    let dir_file = match std::fs::File::open(apps_dir) {
        Ok(f) => f,
        Err(e) => {
            println!("[launcher] Failed to open apps directory: {:?}", e);
            return entries;
        }
    };

    let mut dir = dir_file;

    loop {
        match dir.read_dir() {
            Ok(Some(entry)) => {
                let file_name = entry.name.to_string();
                let file_path = format!("{}/{}", apps_dir, file_name);

                if !file_name.ends_with(".desktop") {
                    continue;
                }

                let app_id = file_name
                    .strip_suffix(".desktop")
                    .unwrap_or(&file_name)
                    .to_string();

                let content = match std::fs::File::open(&file_path) {
                    Ok(mut file) => {
                        let mut buffer = Vec::new();
                        let mut temp_buf = [0u8; 4096];
                        loop {
                            match file.read(&mut temp_buf) {
                                Ok(0) => break,
                                Ok(n) => buffer.extend_from_slice(&temp_buf[..n]),
                                Err(e) => {
                                    println!("[launcher] Failed to read {}: {:?}", file_name, e);
                                    continue;
                                }
                            }
                        }
                        match String::from_utf8(buffer) {
                            Ok(s) => s,
                            Err(e) => {
                                println!("[launcher] Invalid UTF-8 in {}: {:?}", file_name, e);
                                continue;
                            }
                        }
                    }
                    Err(e) => {
                        println!("[launcher] Failed to open {}: {:?}", file_name, e);
                        continue;
                    }
                };

                if let Some(mut parsed) = parse_desktop_file(&content, app_id.clone()) {
                    parsed.file_path = app_id;
                    entries.push(parsed);
                }
            }
            Ok(None) => break,
            Err(e) => {
                println!("[launcher] Error reading directory entry: {:?}", e);
                break;
            }
        }
    }

    println!("[launcher] Loaded {} desktop entries", entries.len());
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Filter entries based on search query
fn filter_entries(entries: &[DesktopEntry], query: &str) -> Vec<DesktopEntry> {
    if query.is_empty() {
        return entries.to_vec();
    }

    let query_lower = query.to_lowercase();

    entries
        .iter()
        .filter(|entry| {
            if entry.name.to_lowercase().contains(&query_lower) {
                return true;
            }

            for category in &entry.categories {
                if category.to_lowercase().contains(&query_lower) {
                    return true;
                }
            }

            false
        })
        .cloned()
        .collect()
}

/// App list view that displays applications with buttons
struct AppListView {
    entries_state: State<Vec<DesktopEntry>>,
    needs_redraw: bool,
}

impl AppListView {
    fn new(entries_state: State<Vec<DesktopEntry>>) -> Self {
        Self {
            entries_state,
            needs_redraw: true,
        }
    }
}

impl View for AppListView {
    fn layout(&mut self, available: Size) -> Size {
        let item_height = 52;
        let count = self.entries_state.with(|e| e.len());
        let total_height = count as u32 * item_height + 16;

        Size::new(available.width, total_height.max(available.height))
    }

    fn flex_factor(&self) -> u32 {
        1
    }

    fn draw(&self, canvas: &mut scarlet_ui::Canvas, frame: scarlet_ui::Rect) {
        use scarlet_ui::graphics::measure_text_sized;

        let palette = Palette::current();
        let item_height = 52;
        let y = frame.y + 8;

        let entries = self.entries_state.with(|e| e.clone());

        if entries.is_empty() {
            let text = "No applications found";
            let (w, h) = measure_text_sized(text, 15.0);
            let text_x = frame.x + (frame.width as i32 - w as i32) / 2;
            canvas.draw_text_sized(
                text_x,
                y + item_height as i32 / 2 - h as i32 / 2,
                text,
                palette.text_mute,
                15.0,
            );
            return;
        }

        for (i, entry) in entries.iter().enumerate() {
            let item_y = y + i as i32 * item_height;

            // Draw background card
            canvas.fill_rounded_rect(
                frame.x + 16,
                item_y,
                frame.width - 32,
                44,
                8,
                palette.surface,
            );

            // Draw border
            canvas.draw_rounded_rect(
                frame.x + 16,
                item_y,
                frame.width - 32,
                44,
                8,
                palette.border,
            );

            // Draw icon
            let icon = entry.get_icon_emoji();
            canvas.draw_text_sized(frame.x + 28, item_y + 10, icon, palette.text_main, 20.0);

            // Draw app name
            canvas.draw_text_sized(
                frame.x + 56,
                item_y + 6,
                &entry.name,
                palette.text_main,
                16.0,
            );

            // Draw category
            let category = entry.get_display_category();
            canvas.draw_text_sized(frame.x + 56, item_y + 26, category, palette.text_mute, 12.0);
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: scarlet_ui::Rect) -> bool {
        match event.kind {
            EventKind::MouseDown {
                button: scarlet_ui::MouseButton::Left,
            } => {
                let x = event.x();
                let y = event.y();
                let item_height = 52;

                if x < frame.x + 16 || x >= frame.x + frame.width as i32 - 16 {
                    return false;
                }

                let start_y = frame.y + 8;
                if y < start_y {
                    return false;
                }

                let index = ((y - start_y) as usize) / item_height as usize;

                let entry_to_launch = self.entries_state.with(|entries| {
                    if index < entries.len() {
                        Some(entries[index].clone())
                    } else {
                        None
                    }
                });

                if let Some(entry) = entry_to_launch {
                    entry.launch();
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn needs_draw(&self) -> bool {
        self.needs_redraw
    }

    fn set_needs_draw(&mut self) {
        self.needs_redraw = true;
    }

    fn clear_needs_draw(&mut self) {
        self.needs_redraw = false;
    }
}

/// Main application entry point
#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[launcher] Starting Scarlet Desktop Application Launcher");

    let all_entries = load_desktop_entries();

    if all_entries.is_empty() {
        println!("[launcher] No desktop entries found!");
    }

    let search_query = State::new(String::new());
    let filtered_entries: State<Vec<DesktopEntry>> = State::new(all_entries.clone());

    let all_entries_clone = all_entries.clone();
    let filtered_entries_clone = filtered_entries.clone();
    let search_query_for_callback = search_query.clone();
    search_query.subscribe(move || {
        let query = search_query_for_callback.get();
        let filtered = filter_entries(&all_entries_clone, &query);
        filtered_entries_clone.set(filtered);
    });

    let app_list_handle = ViewRefreshHandle::new();
    filtered_entries.subscribe_view(&app_list_handle);

    let mut app = match Application::new() {
        Ok(mut a) => {
            a.app_id("org.scarlet-os.desktop.launcher");
            a
        }
        Err(_) => {
            println!("[launcher] Failed to connect to SWS");
            return 1;
        }
    };

    let window_width = 700;
    let window_height = 550;

    let palette = Palette::current();
    let window = Window::new("Applications", window_width, window_height).background(palette.bg);

    let app_list = AppListView::new(filtered_entries.clone());

    let content = VStack::new()
        .spacing(0)
        .alignment(StackAlignment::Center)
        .child(
            Padding::new(
                VStack::new()
                    .spacing(16)
                    .alignment(StackAlignment::Center)
                    .child(
                        Label::new("Applications")
                            .color(palette.text_main)
                            .font_size(28),
                    )
                    .child(
                        TextField::new("Search applications...", search_query.clone())
                            .background(palette.surface)
                            .text_color(palette.text_main)
                            .border_color(palette.border)
                            .focused_border_color(palette.primary)
                            .corner_radius(8)
                            .padding(12),
                    ),
            )
            .all(24),
        )
        .child(RectView::new(palette.border).height(1))
        .child(Padding::new(app_list).vertical(8).horizontal(0))
        .child(Spacer::new())
        .child(RectView::new(palette.border).height(1))
        .child(
            Padding::new(
                Label::new("Type to search • Click to launch")
                    .color(palette.text_mute)
                    .font_size(12),
            )
            .all(16),
        );

    let window = window.content(content);

    if let Err(e) = app.add_window(window) {
        println!("[launcher] Failed to add window: {:?}", e);
        return 1;
    }

    println!("[launcher] Running application loop");
    app.run();
}
