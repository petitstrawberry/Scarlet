//! Scarlet Desktop Application Launcher
//!
//! Provides a searchable application grid with keyboard navigation
//! and the ability to launch applications from desktop entry files.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{
    Application, Color, Label, Padding, RectView, Size, Spacer, StackAlignment,
    State, TextField, VStack, View, Window, Event, EventKind, ViewRefreshHandle,
};
use std::{format, println, string::String, string::ToString, vec::Vec};

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

        // Remove common field codes: %f, %F, %u, %U, %d, %D, %n, %N, %k, %v
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

        // Use sbus to launch or focus the application
        self.launch_via_sbus(&self.file_path);
    }

    /// Launch application via sbus (stemd)
    fn launch_via_sbus(&self, app_id: &str) {
        use sbus::Argument;
        use sbus_client;

        println!("[launcher] Launching via sbus: {}", app_id);

        // Connect to sbus
        let mut conn = match sbus_client::Connection::connect() {
            Ok(c) => c,
            Err(e) => {
                println!("[launcher] Failed to connect to sbus: {:?}", e);
                // Fall back to direct spawn only on connection failure
                self.spawn_direct();
                return;
            }
        };

        // Call stemd's LaunchOrFocus method
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
                // Successfully called the method - check response
                if !result.is_empty() {
                    if let Argument::String(ref s) = result[0] {
                        println!("[launcher] sbus response: {}", s);
                        // Whether successful or not, stemd handled it
                        // Don't fall back to direct spawn - stemd will manage the launch
                    }
                }
            }
            Err(e) => {
                println!("[launcher] Failed to call LaunchOrFocus: {:?}", e);
                // Fall back to direct spawn only on call failure
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
                // Child process
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
        // Match icon name
        if self.icon.contains("terminal") || self.icon.contains("console") {
            return "⌘";
        }
        if self.icon.contains("text") || self.icon.contains("editor") {
            return "📝";
        }
        if self.icon.contains("file") || self.icon.contains("folder") {
            return "📁";
        }
        if self.icon.contains("setting") || self.icon.contains("preference") {
            return "⚙️";
        }
        if self.icon.contains("network") || self.icon.contains("wifi") {
            return "🌐";
        }
        if self.icon.contains("audio") || self.icon.contains("sound") {
            return "🔊";
        }
        if self.icon.contains("video") || self.icon.contains("camera") {
            return "📹";
        }
        if self.icon.contains("image") || self.icon.contains("photo") {
            return "🖼️";
        }
        if self.icon.contains("calc") || self.icon.contains("math") {
            return "🧮";
        }
        if self.icon.contains("clock") || self.icon.contains("time") {
            return "🕐";
        }
        if self.icon.contains("game") {
            return "🎮";
        }

        // Match category
        if self.categories.iter().any(|c| c.contains("System")) {
            return "💻";
        }
        if self.categories.iter().any(|c| c.contains("Utility")) {
            return "🔧";
        }
        if self.categories.iter().any(|c| c.contains("Development")) {
            return "👨‍💻";
        }
        if self.categories.iter().any(|c| c.contains("Network")) {
            return "🌐";
        }
        if self.categories.iter().any(|c| c.contains("Audio")) {
            return "🎵";
        }
        if self.categories.iter().any(|c| c.contains("Video")) {
            return "🎬";
        }
        if self.categories.iter().any(|c| c.contains("Game")) {
            return "🎮";
        }

        // Default icon
        "📦"
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

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Check for [Desktop Entry] header
        if line == "[Desktop Entry]" {
            in_desktop_entry = true;
            continue;
        }

        // Check for another group header
        if line.starts_with('[') && line.ends_with(']') {
            if in_desktop_entry {
                // We've left the Desktop Entry section
                break;
            }
            continue;
        }

        // Parse key=value pairs
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

    // Validate that we have the minimum required fields
    if name.is_empty() || exec.is_empty() {
        println!("[launcher] Invalid desktop file {}: missing Name or Exec", file_path);
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

    // Open directory using File::open
    let dir_file = match std::fs::File::open(apps_dir) {
        Ok(f) => f,
        Err(e) => {
            println!("[launcher] Failed to open apps directory: {:?}", e);
            return entries;
        }
    };

    // Read directory entries using File::read_dir
    let mut dir = dir_file;

    loop {
        match dir.read_dir() {
            Ok(Some(entry)) => {
                let file_name = entry.name.to_string();
                let file_path = format!("{}/{}", apps_dir, file_name);

                // Only process .desktop files
                if !file_name.ends_with(".desktop") {
                    continue;
                }

                // Extract app_id from filename (remove .desktop extension)
                let app_id = file_name
                    .strip_suffix(".desktop")
                    .unwrap_or(&file_name)
                    .to_string();

                println!("[launcher] Loading: {}", file_name);

                // Read file content
                let content = match std::fs::File::open(&file_path) {
                    Ok(mut file) => {
                        let mut buffer = Vec::new();
                        let mut temp_buf = [0u8; 4096];
                        loop {
                            match file.read(&mut temp_buf) {
                                Ok(0) => break, // EOF
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

                // Parse desktop file
                if let Some(mut parsed) = parse_desktop_file(&content, app_id.clone()) {
                    // Ensure file_path is set to app_id for sbus
                    parsed.file_path = app_id;
                    entries.push(parsed);
                }
            }
            Ok(None) => break, // EOF - no more entries
            Err(e) => {
                println!("[launcher] Error reading directory entry: {:?}", e);
                break;
            }
        }
    }

    println!("[launcher] Loaded {} desktop entries", entries.len());

    // Sort entries alphabetically by name
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
            // Search in name
            if entry.name.to_lowercase().contains(&query_lower) {
                return true;
            }

            // Search in categories
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
        // Calculate total height needed
        let item_height = 48;
        let count = self.entries_state.with(|e| e.len());
        let total_height = count as u32 * item_height + 16;

        Size::new(available.width, total_height.max(available.height))
    }

    fn flex_factor(&self) -> u32 {
        1
    }

    fn draw(&self, canvas: &mut scarlet_ui::Canvas, frame: scarlet_ui::Rect) {
        use scarlet_ui::graphics::{measure_text_sized};

        let item_height = 48;
        let y = frame.y + 8;

        // Get entries
        let entries = self.entries_state.with(|e| e.clone());

        // Draw "No apps found" message if empty
        if entries.is_empty() {
            let text = "No applications found";
            let (w, h) = measure_text_sized(text, 15.0);
            let text_x = frame.x + (frame.width as i32 - w as i32) / 2;
            canvas.draw_text_sized(
                text_x,
                y + item_height as i32 / 2 - h as i32 / 2,
                text,
                Color::rgb(150, 150, 150),
                15.0,
            );
            return;
        }

        // Draw each app entry
        for (i, entry) in entries.iter().enumerate() {
            let item_y = y + i as i32 * item_height;

            // Draw background
            canvas.fill_rounded_rect(
                frame.x + 20,
                item_y,
                frame.width - 40,
                40,
                6,
                Color::rgb(45, 45, 45),
            );

            // Draw border
            canvas.draw_rounded_rect(
                frame.x + 20,
                item_y,
                frame.width - 40,
                40,
                6,
                Color::rgb(70, 70, 70),
            );

            // Draw icon
            let icon = entry.get_icon_emoji();
            canvas.draw_text_sized(
                frame.x + 32,
                item_y + 12,
                icon,
                Color::rgb(220, 220, 220),
                18.0,
            );

            // Draw app name
            canvas.draw_text_sized(
                frame.x + 60,
                item_y + 6,
                &entry.name,
                Color::rgb(235, 235, 235),
                16.0,
            );

            // Draw category
            let category = entry.get_display_category();
            canvas.draw_text_sized(
                frame.x + 60,
                item_y + 24,
                category,
                Color::rgb(140, 140, 140),
                12.0,
            );
        }
    }

    fn on_event(&mut self, event: &mut Event, frame: scarlet_ui::Rect) -> bool {
        match event.kind {
            EventKind::MouseDown { button: scarlet_ui::MouseButton::Left } => {
                let x = event.x();
                let y = event.y();
                let item_height = 48;

                // Check if click is within bounds
                if x < frame.x + 20 || x >= frame.x + frame.width as i32 - 20 {
                    return false;
                }

                let start_y = frame.y + 8;
                if y < start_y {
                    return false;
                }

                let index = ((y - start_y) as usize) / item_height as usize;

                // Check if index is valid and launch
                let entry_to_launch = self.entries_state.with(|entries| {
                    if index < entries.len() {
                        Some(entries[index].clone())
                    } else {
                        None
                    }
                });

                // Launch after returning from event handler
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

    // Load desktop entries
    let all_entries = load_desktop_entries();

    if all_entries.is_empty() {
        println!("[launcher] No desktop entries found!");
    }

    // Create search query state
    let search_query = State::new(String::new());

    // Create filtered entries state
    let filtered_entries: State<Vec<DesktopEntry>> = State::new(all_entries.clone());

    // Subscribe to search query changes to update filtered entries
    let all_entries_clone = all_entries.clone();
    let filtered_entries_clone = filtered_entries.clone();
    let search_query_for_callback = search_query.clone();
    search_query.subscribe(move || {
        let query = search_query_for_callback.get();
        let filtered = filter_entries(&all_entries_clone, &query);
        filtered_entries_clone.set(filtered);
    });

    // Subscribe to filtered entries changes to redraw view
    let app_list_handle = ViewRefreshHandle::new();
    filtered_entries.subscribe_view(&app_list_handle);

    // Create application
    let mut app = match Application::new() {
        Ok(mut a) => {
            // Set app_id to prevent duplicate launches
            a.app_id("org.scarlet-os.desktop.launcher");
            a
        }
        Err(_) => {
            println!("[launcher] Failed to connect to SWS");
            return 1;
        }
    };

    // Create main window
    let window_width = 700;
    let window_height = 550;

    let window = Window::new("Applications", window_width, window_height)
        .background(Color::rgb(25, 25, 25));

    // Create the app list view with filtered entries state
    let app_list = AppListView::new(filtered_entries.clone());

    // Build the UI
    let content = VStack::new()
        .spacing(0)
        .alignment(StackAlignment::Center)
        .child(
            // Header with title and search
            Padding::new(
                VStack::new()
                    .spacing(16)
                    .alignment(StackAlignment::Center)
                    .child(
                        Label::new("Applications")
                            .color(Color::rgb(235, 235, 235))
                            .font_size(28),
                    )
                    .child(
                        // Search field
                        TextField::new("Search applications...", search_query.clone())
                            .background(Color::rgb(45, 45, 45))
                            .text_color(Color::rgb(220, 220, 220))
                            .border_color(Color::rgb(70, 70, 70))
                            .focused_border_color(Color::rgb(100, 150, 220))
                            .corner_radius(8)
                            .padding(12)
                    ),
            )
            .all(24)
        )
        .child(
            // Divider line
            RectView::new(Color::rgb(50, 50, 50))
                .height(1)
        )
        .child(
            // Application list
            Padding::new(
                app_list,
            )
            .vertical(8)
            .horizontal(0)
        )
        .child(
            // Spacer to push footer to bottom
            Spacer::new()
        )
        .child(
            // Divider line
            RectView::new(Color::rgb(50, 50, 50))
                .height(1)
        )
        .child(
            // Footer with hint
            Padding::new(
                Label::new("Type to search • Click to launch")
                    .color(Color::rgb(120, 120, 120))
                    .font_size(12),
            )
            .all(16)
        );

    let window = window.content(content);

    // Add window to application
    if let Err(e) = app.add_window(window) {
        println!("[launcher] Failed to add window: {:?}", e);
        return 1;
    }

    println!("[launcher] Running application loop");

    // Run the application (blocks forever)
    app.run();
}
