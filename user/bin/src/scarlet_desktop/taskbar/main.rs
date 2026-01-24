//! Scarlet Desktop Menu Bar.
//!
//! Provides a macOS-style menu bar as a regular SWS client.
//!
//! - Window type: TASKBAR
//! - Height: 28px (optimized for menu bar usage)
//! - Positions at top of screen
//! - Left side: Application menus
//! - Right side: System status (clock, CPU, memory)
//! - Sends workarea notification to SWS
//!
#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_desktop_config;
extern crate scarlet_std as std;

use core::time::Duration;
use sbus_client;
use scarlet_desktop_config::{TaskbarConfig, TaskbarPosition};
use scarlet_ui::graphics;
use scarlet_ui::Color;
use std::string::{String, ToString};
use std::thread;
use std::vec;
use std::vec::Vec;
use std::{format, println};
use sws_client::event::Event as SwsEvent;
use sws_client::{Connection, InputEvent, WindowSizeLimits};
use sws_protocol::window_types;

/// stemd IPC protocol constants
mod stemd_protocol {
    pub const LAUNCH_OR_FOCUS: u8 = 0x01;
}

/// Dropdown menu state
struct DropdownState {
    surface_id: Option<u32>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    menu_index: Option<usize>,
    items: Vec<DropdownItem>,
}

impl DropdownState {
    fn new() -> Self {
        Self {
            surface_id: None,
            x: 0,
            y: 0,
            width: 200,
            height: 0,
            menu_index: None,
            items: Vec::new(),
        }
    }

    fn is_open(&self) -> bool {
        self.surface_id.is_some()
    }

    fn contains(&self, x: i32, y: i32) -> bool {
        if !self.is_open() {
            return false;
        }
        x >= self.x
            && x < self.x + self.width as i32
            && y >= self.y
            && y < self.y + self.height as i32
    }

    fn close(&mut self) {
        self.surface_id = None;
        self.menu_index = None;
        self.items.clear();
    }
}

/// Dropdown menu item type
enum DropdownItemType {
    Window { window_id: u32 },
    Action { action: DropdownAction },
}

/// Dropdown menu action
enum DropdownAction {
    LaunchSettings,
    FocusApp { app_id: String },
    // Add more actions here as needed
}

/// Dropdown menu item
struct DropdownItem {
    label: String,
    item_type: DropdownItemType,
    y: i32,
    height: u32,
}

impl DropdownItem {
    fn contains(&self, x: i32, y: i32, dropdown_x: i32, dropdown_width: u32) -> bool {
        x >= dropdown_x
            && x < dropdown_x + dropdown_width as i32
            && y >= self.y
            && y < self.y + self.height as i32
    }
}

/// Menu bar item (left side)
struct MenuItem {
    label: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl MenuItem {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && x < self.x + self.width as i32
            && y >= self.y
            && y < self.y + self.height as i32
    }
}

/// System status item (right side)
struct StatusItem {
    label: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl StatusItem {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && x < self.x + self.width as i32
            && y >= self.y
            && y < self.y + self.height as i32
    }
}

fn load_config() -> TaskbarConfig {
    scarlet_desktop_config::load_desktop_config().taskbar
}

/// Spawn an application process
fn spawn_application(path: &str, args: &[&str]) -> bool {
    use std::task;

    println!("[menu_bar] Spawning: {} {:?}", path, args);

    match unsafe { task::fork() } {
        0 => {
            // Child process - exec the application
            let mut argv: Vec<&str> = Vec::new();
            argv.push(path);
            for arg in args {
                argv.push(arg);
            }
            let envp: &[&str] = &[];

            let _ = task::execve(path, &argv, envp);
            // If exec fails, exit child
            task::exit(1);
        }
        pid if pid > 0 => {
            // Parent process - child spawned successfully
            println!("[menu_bar] Spawned process with PID: {}", pid);
            true
        }
        _ => {
            // Fork failed
            println!("[menu_bar] Failed to fork for application spawn");
            false
        }
    }
}

/// Launch or focus an application via sbus
/// Calls stemd's LaunchOrFocus method through the sbus
fn launch_or_focus_app(app_id: &str) -> bool {
    use alloc::string::ToString;
    use sbus::Argument;

    println!("[menu_bar] Launching app via sbus: {}", app_id);

    // Connect to sbus
    let mut conn = match sbus_client::Connection::connect() {
        Ok(c) => c,
        Err(e) => {
            println!("[menu_bar] Failed to connect to sbus: {:?}", e);
            return false;
        }
    };

    // Call stemd's LaunchOrFocus method
    let mut args = Vec::new();
    args.push(Argument::String(app_id.to_string()));

    match conn.call_method(
        "org.scarlet-os.stemd", // destination
        "/org/scarlet/stemd",   // path
        "org.scarlet-os.stemd", // interface
        "LaunchOrFocus",        // method
        args,
    ) {
        Ok(result) => {
            if !result.is_empty() {
                if let Argument::String(ref s) = result[0] {
                    let success = s.starts_with("OK")
                        || s.starts_with("Focused")
                        || s.starts_with("Launched");
                    println!("[menu_bar] sbus response: {}", s);
                    return success;
                }
            }
            println!("[menu_bar] sbus returned empty or unexpected result");
            false
        }
        Err(e) => {
            println!("[menu_bar] Failed to call LaunchOrFocus: {:?}", e);
            false
        }
    }
}

/// Query running applications from stemd
/// Returns a list of (app_id, app_name) tuples
fn get_running_apps_from_stemd() -> Result<Vec<(String, String)>, &'static str> {
    use alloc::string::ToString;
    use sbus::Argument;

    println!("[menu_bar] Querying running apps from stemd");

    // Connect to sbus
    let mut conn = match sbus_client::Connection::connect() {
        Ok(c) => c,
        Err(e) => {
            println!("[menu_bar] Failed to connect to sbus: {:?}", e);
            return Err("Failed to connect to sbus");
        }
    };

    // Call stemd's GetRunningApps method (no arguments)
    let args = Vec::new();

    match conn.call_method(
        "org.scarlet-os.stemd",
        "/org/scarlet/stemd",
        "org.scarlet-os.stemd",
        "GetRunningApps",
        args,
    ) {
        Ok(result) => {
            let mut apps = Vec::new();
            for arg in result.iter() {
                if let Argument::String(s) = arg {
                    // Parse "app_id|name" format
                    if let Some(pipe_pos) = s.find('|') {
                        let app_id = s[..pipe_pos].to_string();
                        let app_name = s[pipe_pos + 1..].to_string();
                        apps.push((app_id, app_name));
                    } else {
                        // Fallback: use the whole string as both app_id and name
                        apps.push((s.clone(), s.clone()));
                    }
                }
            }
            println!("[menu_bar] Got {} running apps from stemd", apps.len());
            Ok(apps)
        }
        Err(e) => {
            println!("[menu_bar] Failed to call GetRunningApps: {:?}", e);
            Err("Failed to call GetRunningApps")
        }
    }
}

/// Query the currently active app from stemd
/// Returns (app_id, app_name) or None if no app is focused
fn get_active_app_from_stemd() -> Option<(String, String)> {
    use alloc::string::ToString;
    use sbus::Argument;

    // Connect to sbus
    let mut conn = match sbus_client::Connection::connect() {
        Ok(c) => c,
        Err(e) => {
            // Silent failure - don't log on every poll
            return None;
        }
    };

    // Call stemd's GetActiveApp method (no arguments)
    let args = Vec::new();

    match conn.call_method(
        "org.scarlet-os.stemd",
        "/org/scarlet/stemd",
        "org.scarlet-os.stemd",
        "GetActiveApp",
        args,
    ) {
        Ok(result) => {
            if !result.is_empty() {
                if let Argument::String(s) = &result[0] {
                    if s.is_empty() {
                        return None;
                    }
                    // Parse "app_id|name" format
                    if let Some(pipe_pos) = s.find('|') {
                        let app_id = s[..pipe_pos].to_string();
                        let app_name = s[pipe_pos + 1..].to_string();
                        return Some((app_id, app_name));
                    }
                }
            }
            None
        }
        Err(e) => {
            // Silent failure - don't log on every poll
            None
        }
    }
}

/// Build menu items based on the active application
/// - First menu is always "Scarlet" (fixed)
/// - When an app is focused, show app name + app-specific menus after Scarlet
/// - When no app is focused, show only Scarlet
fn build_menu_items(
    active_app: &Option<(String, String, String, String)>,
    bar_height: u32,
) -> Vec<MenuItem> {
    let mut menu_items = Vec::new();
    let mut x_offset = 0i32;

    // Always add Scarlet menu as the first item (fixed)
    let (tw, _): (u32, u32) = graphics::measure_text_sized("Scarlet", 13.0);
    let width: u32 = tw.saturating_add(16).min(100);
    menu_items.push(MenuItem {
        label: String::from("Scarlet"),
        x: x_offset,
        y: 0,
        width,
        height: bar_height,
    });
    x_offset += width as i32;

    match active_app {
        Some((_app_id, app_name, _title, menu_titles)) => {
            // App is focused: show app name followed by app-specific menus
            // Parse menu_titles from the event (format: "menu1|menu2|menu3")
            let menus = parse_menu_titles(menu_titles);

            // Add app name as a menu item
            if !app_name.is_empty() {
                let (tw, _): (u32, u32) = graphics::measure_text_sized(&app_name, 13.0);
                let width: u32 = tw.saturating_add(16).min(100);
                menu_items.push(MenuItem {
                    label: app_name.clone(),
                    x: x_offset,
                    y: 0,
                    width,
                    height: bar_height,
                });
                x_offset += width as i32;
            }

            for label in &menus {
                let (tw, _): (u32, u32) = graphics::measure_text_sized(&label, 13.0);
                let width: u32 = tw.saturating_add(16).min(100);
                menu_items.push(MenuItem {
                    label: label.clone(),
                    x: x_offset,
                    y: 0,
                    width,
                    height: bar_height,
                });
                x_offset += width as i32;
            }
        }
        None => {
            // No app focused: only Scarlet menu is shown
        }
    }

    menu_items
}

/// Parse menu titles from the format "menu1|menu2|menu3"
fn parse_menu_titles(menu_titles: &str) -> Vec<String> {
    if menu_titles.is_empty() {
        return Vec::new();
    }
    menu_titles.split('|').map(|s| s.to_string()).collect()
}

/// Query app menu titles from stemd via sbus
/// NOTE: This function is deprecated and should be removed once all apps use menu registration
fn query_app_menus_from_stemd(app_id: &str) -> Result<Vec<String>, &'static str> {
    use alloc::string::ToString;
    use sbus::Argument;

    println!("[menu_bar] Querying menus from stemd for app: {}", app_id);

    // Connect to sbus
    let mut conn = match sbus_client::Connection::connect() {
        Ok(c) => c,
        Err(e) => {
            println!("[menu_bar] Failed to connect to sbus: {:?}", e);
            return Err("Failed to connect to sbus");
        }
    };

    // Call stemd's GetAppMenus method
    let mut args = Vec::new();
    args.push(Argument::String(app_id.to_string()));

    match conn.call_method(
        "org.scarlet-os.stemd",
        "/org/scarlet/stemd",
        "org.scarlet-os.stemd",
        "GetAppMenus",
        args,
    ) {
        Ok(result) => {
            let mut menu_titles = Vec::new();
            for arg in result.iter() {
                if let Argument::String(s) = arg {
                    // Parse format: "menu1|menu2|menu3"
                    if !s.is_empty() {
                        for title in s.split('|') {
                            menu_titles.push(title.to_string());
                        }
                    }
                }
            }
            println!(
                "[menu_bar] Got {} menu titles from stemd",
                menu_titles.len()
            );
            Ok(menu_titles)
        }
        Err(e) => {
            println!("[menu_bar] Failed to call GetAppMenus: {:?}", e);
            Err("Failed to call GetAppMenus")
        }
    }
}

fn handle_input(
    ev: InputEvent,
    cursor_x: &mut i32,
    cursor_y: &mut i32,
    left_down: &mut bool,
    pressed_items: &mut Vec<usize>,
    hovered_items: &mut Vec<usize>,
    menu_items: &[MenuItem],
) -> bool {
    match ev.type_ {
        0x03 => {
            // EV_ABS - cursor movement
            match ev.code {
                0x00 => {
                    *cursor_x = ev.value; // ABS_X
                    // Update hovered items
                    hovered_items.clear();
                    for (i, item) in menu_items.iter().enumerate() {
                        if item.contains(*cursor_x, *cursor_y) {
                            hovered_items.push(i);
                        }
                    }
                    true // Redraw on hover change
                }
                0x01 => {
                    *cursor_y = ev.value; // ABS_Y
                    // Update hovered items
                    hovered_items.clear();
                    for (i, item) in menu_items.iter().enumerate() {
                        if item.contains(*cursor_x, *cursor_y) {
                            hovered_items.push(i);
                        }
                    }
                    true // Redraw on hover change
                }
                _ => false,
            }
        }
        0x01 => {
            // EV_KEY
            // BTN_LEFT = 0x110
            if ev.code == 0x110 {
                if ev.value != 0 {
                    // Mouse down
                    *left_down = true;
                    pressed_items.clear();
                    for (i, item) in menu_items.iter().enumerate() {
                        if item.contains(*cursor_x, *cursor_y) {
                            pressed_items.push(i);
                        }
                    }
                    true
                } else {
                    // Mouse up
                    *left_down = false;
                    let clicked = pressed_items.clone();
                    pressed_items.clear();

                    // Check if mouse is still over the same items
                    for &idx in &clicked {
                        if idx < menu_items.len() && menu_items[idx].contains(*cursor_x, *cursor_y)
                        {
                            // Menu item clicked
                            let label = &menu_items[idx].label;
                            println!("[menu_bar] Menu clicked: {}", label);
                            return true;
                        }
                    }
                    true
                }
            } else {
                false
            }
        }
        _ => false,
    }
}

fn draw_menu_bar(
    conn: &mut Connection,
    surface_id: u32,
    seconds: u32,
    cpu_usage: u8,
    memory_usage: u8,
    pressed_items: &[usize],
    hovered_items: &[usize],
    menu_items: &[MenuItem],
    status_items: &[StatusItem],
) {
    let Some(surface) = conn.surface_mut(surface_id) else {
        return;
    };

    let w = surface.width();
    let h = surface.height();

    // Use theme colors
    let bg_color = Color::rgb(0.922, 0.922, 0.933); // sidebar_bg
    let text_color = Color::rgb(0.078, 0.078, 0.094); // text_main
    let text_dim = Color::rgb(0.471, 0.471, 0.471); // text_sub
    let border_color = Color::rgb(0.392, 0.392, 0.412); // border
    let hover_bg = Color::rgb(0.824, 0.824, 0.839); // hover
    let active_bg = Color::rgb(0.706, 0.706, 0.745); // primary (pressed)

    surface.with_buffer(|buf, width, height| {
        // Bar background
        fill_rect(buf, width, h, 0, 0, w, h, bg_color);
        draw_hline(buf, width, h, 0, (h - 1) as i32, w, border_color);

        // Draw menu items (left side)
        let font_size = 13.0;
        for (i, item) in menu_items.iter().enumerate() {
            let is_pressed = pressed_items.contains(&i);
            let is_hovered = hovered_items.contains(&i);

            // Draw hover/active background
            if is_pressed {
                fill_rect(
                    buf,
                    width,
                    h,
                    item.x,
                    item.y + 1,
                    item.width,
                    item.height.saturating_sub(2),
                    active_bg,
                );
            } else if is_hovered {
                fill_rect(
                    buf,
                    width,
                    h,
                    item.x,
                    item.y + 1,
                    item.width,
                    item.height.saturating_sub(2),
                    hover_bg,
                );
            }

            // Draw text
            let text_x = item.x + 8;
            let text_y = item.y + ((item.height as i32 - 16) / 2).max(0);

            draw_text_helper(buf, width, h, text_x, text_y, &item.label, text_color, font_size);
        }

        // Draw status items (right side)
        for item in status_items {
            let text_x = item.x + 6;
            let text_y = item.y + ((item.height as i32 - 16) / 2).max(0);

            draw_text_helper(buf, width, h, text_x, text_y, &item.label, text_dim, font_size);
        }
    });

    let _ = conn.commit(surface_id);
}

/// Fill a rectangle with solid color
fn fill_rect(
    buf: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    color: Color,
) {
    let bgra = color.to_bgra();
    let x_start = x.max(0) as u32;
    let y_start = y.max(0) as u32;
    let x_end = (x as u32 + w).min(width);
    let y_end = (y as u32 + h).min(height);

    for py in y_start..y_end {
        for px in x_start..x_end {
            let idx = (py as usize * width as usize + px as usize) * 4;
            if idx + 3 < buf.len() {
                buf[idx] = (bgra & 0xFF) as u8;
                buf[idx + 1] = ((bgra >> 8) & 0xFF) as u8;
                buf[idx + 2] = ((bgra >> 16) & 0xFF) as u8;
                buf[idx + 3] = ((bgra >> 24) & 0xFF) as u8;
            }
        }
    }
}

/// Draw horizontal line
fn draw_hline(buf: &mut [u8], width: u32, height: u32, x: i32, y: i32, w: u32, color: Color) {
    fill_rect(buf, width, height, x, y, w, 1, color);
}

/// Show window list dropdown menu
/// Shows running applications from stemd, grouped by app_id
fn show_window_list_dropdown(
    conn: &mut Connection,
    dropdown: &mut DropdownState,
    menu_item: &MenuItem,
    menu_index: usize,
) {
    println!("[menu_bar] Showing window list dropdown");

    // Close any existing dropdown
    if let Some(old_id) = dropdown.surface_id {
        let _ = conn.destroy_surface(old_id);
    }

    // Query running apps from stemd
    let running_apps = match get_running_apps_from_stemd() {
        Ok(apps) => apps,
        Err(e) => {
            println!("[menu_bar] Failed to get running apps from stemd: {:?}", e);
            // Fallback to SWS window list
            show_legacy_window_list(conn, dropdown, menu_item, menu_index);
            return;
        }
    };

    if running_apps.is_empty() {
        println!("[menu_bar] No running apps to show");
        return;
    }

    // Calculate dropdown size
    const ITEM_HEIGHT: u32 = 28;
    const DROPDOWN_WIDTH: u32 = 250;

    let item_count = running_apps.len().min(15); // Max 15 items visible
    let dropdown_height = (item_count as u32) * ITEM_HEIGHT + 4; // +4 for border

    // Create dropdown surface (AlwaysOnTop for popup)
    let surface_id = match conn.create_surface_with_type(
        "org.scarlet-os.desktop.taskbar",
        "Taskbar",
        "",
        DROPDOWN_WIDTH,
        dropdown_height,
        window_types::ALWAYS_ON_TOP,
    ) {
        Ok(id) => id,
        Err(_) => {
            println!("[menu_bar] Failed to create dropdown surface");
            return;
        }
    };

    let _ = conn.set_window_type(surface_id, window_types::ALWAYS_ON_TOP);

    // Position below the menu item
    let x = menu_item.x;
    let y = menu_item.height as i32; // Below menu bar
    let _ = conn.move_window(surface_id, x, y);

    // Build dropdown items from running apps
    dropdown.items.clear();
    let mut current_y = 2i32; // Start with 2px top padding

    for (app_id, app_name) in &running_apps {
        // Use the app name from the registry
        let label = app_name.clone();

        dropdown.items.push(DropdownItem {
            label,
            item_type: DropdownItemType::Action {
                action: DropdownAction::FocusApp {
                    app_id: app_id.clone(),
                },
            },
            y: current_y,
            height: ITEM_HEIGHT,
        });

        current_y += ITEM_HEIGHT as i32;
    }

    // Update dropdown state
    dropdown.surface_id = Some(surface_id);
    dropdown.x = x;
    dropdown.y = y;
    dropdown.width = DROPDOWN_WIDTH;
    dropdown.height = dropdown_height;
    dropdown.menu_index = Some(menu_index);

    // Draw dropdown
    draw_dropdown(conn, surface_id, dropdown);

    println!(
        "[menu_bar] Dropdown shown: {} running apps at ({}, {})",
        running_apps.len(),
        x,
        y
    );
}

/// Legacy fallback: show window list directly from SWS
fn show_legacy_window_list(
    conn: &mut Connection,
    dropdown: &mut DropdownState,
    menu_item: &MenuItem,
    menu_index: usize,
) {
    println!("[menu_bar] Using legacy window list from SWS");

    // Fetch window list
    let windows = match conn.get_window_list() {
        Ok(w) => w,
        Err(e) => {
            println!("[menu_bar] Failed to get window list: {:?}", e);
            return;
        }
    };

    if windows.is_empty() {
        println!("[menu_bar] No windows to show");
        return;
    }

    // Calculate dropdown size
    const ITEM_HEIGHT: u32 = 28;
    const DROPDOWN_WIDTH: u32 = 250;

    let item_count = windows.len().min(15);
    let dropdown_height = (item_count as u32) * ITEM_HEIGHT + 4;

    // Create dropdown surface
    let surface_id = match conn.create_surface(
        "org.scarlet-os.desktop.taskbar",
        "Taskbar",
        "",
        DROPDOWN_WIDTH,
        dropdown_height,
    ) {
        Ok(id) => id,
        Err(_) => {
            println!("[menu_bar] Failed to create dropdown surface");
            return;
        }
    };

    let _ = conn.set_window_type(surface_id, window_types::ALWAYS_ON_TOP);
    let x = menu_item.x;
    let y = menu_item.height as i32;
    let _ = conn.move_window(surface_id, x, y);

    // Build dropdown items
    dropdown.items.clear();
    let mut current_y = 2i32;

    for window in &windows {
        let label = if window.title.is_empty() {
            String::from("Untitled")
        } else {
            window.title.clone()
        };

        dropdown.items.push(DropdownItem {
            label,
            item_type: DropdownItemType::Window {
                window_id: window.window_id,
            },
            y: current_y,
            height: ITEM_HEIGHT,
        });

        current_y += ITEM_HEIGHT as i32;
    }

    dropdown.surface_id = Some(surface_id);
    dropdown.x = x;
    dropdown.y = y;
    dropdown.width = DROPDOWN_WIDTH;
    dropdown.height = dropdown_height;
    dropdown.menu_index = Some(menu_index);

    draw_dropdown(conn, surface_id, dropdown);
}

/// Show application menu dropdown
/// Displays different menu items based on the menu label
fn show_app_menu_dropdown(
    conn: &mut Connection,
    dropdown: &mut DropdownState,
    menu_item: &MenuItem,
    menu_index: usize,
) {
    println!("[menu_bar] Showing app menu dropdown: {}", menu_item.label);

    // Close any existing dropdown
    if let Some(old_id) = dropdown.surface_id {
        let _ = conn.destroy_surface(old_id);
    }

    // Calculate dropdown size based on menu items
    const ITEM_HEIGHT: u32 = 28;
    const DROPDOWN_WIDTH: u32 = 200;

    // Determine menu items based on label
    let menu_items = get_menu_items_for_label(&menu_item.label);
    let num_items = menu_items.len();
    let dropdown_height = num_items as u32 * ITEM_HEIGHT + 4;

    // Create dropdown surface (AlwaysOnTop for popup)
    let surface_id = match conn.create_surface_with_type(
        "org.scarlet-os.desktop.taskbar",
        "Taskbar",
        "",
        DROPDOWN_WIDTH,
        dropdown_height,
        window_types::ALWAYS_ON_TOP,
    ) {
        Ok(id) => id,
        Err(_) => {
            println!("[menu_bar] Failed to create dropdown surface");
            return;
        }
    };

    let _ = conn.set_window_type(surface_id, window_types::ALWAYS_ON_TOP);

    // Position below the menu item
    let x = menu_item.x;
    let y = menu_item.height as i32; // Below menu bar
    let _ = conn.move_window(surface_id, x, y);

    // Build dropdown items
    dropdown.items.clear();
    let mut current_y = 2i32; // Start with 2px top padding

    for (label, action) in menu_items {
        dropdown.items.push(DropdownItem {
            label,
            item_type: DropdownItemType::Action { action },
            y: current_y,
            height: ITEM_HEIGHT,
        });
        current_y += ITEM_HEIGHT as i32;
    }

    // Update dropdown state
    dropdown.surface_id = Some(surface_id);
    dropdown.x = x;
    dropdown.y = y;
    dropdown.width = DROPDOWN_WIDTH;
    dropdown.height = dropdown_height;
    dropdown.menu_index = Some(menu_index);

    // Draw dropdown
    draw_dropdown(conn, surface_id, dropdown);
}

/// Get menu items for a given menu label
fn get_menu_items_for_label(label: &str) -> Vec<(String, DropdownAction)> {
    match label {
        "Scarlet" => vec![
            (String::from("Settings..."), DropdownAction::LaunchSettings),
            (
                String::from("About Scarlet"),
                DropdownAction::LaunchSettings,
            ),
        ],
        "File" => vec![
            (String::from("New"), DropdownAction::LaunchSettings),
            (String::from("Open"), DropdownAction::LaunchSettings),
            (String::from("-"), DropdownAction::LaunchSettings),
            (String::from("Save"), DropdownAction::LaunchSettings),
            (String::from("Save As..."), DropdownAction::LaunchSettings),
        ],
        "Edit" => vec![
            (String::from("Cut"), DropdownAction::LaunchSettings),
            (String::from("Copy"), DropdownAction::LaunchSettings),
            (String::from("Paste"), DropdownAction::LaunchSettings),
            (String::from("-"), DropdownAction::LaunchSettings),
            (String::from("Select All"), DropdownAction::LaunchSettings),
        ],
        "View" => vec![
            (String::from("Zoom In"), DropdownAction::LaunchSettings),
            (String::from("Zoom Out"), DropdownAction::LaunchSettings),
            (String::from("-"), DropdownAction::LaunchSettings),
            (String::from("Full Screen"), DropdownAction::LaunchSettings),
        ],
        "Window" => vec![
            (String::from("Minimize"), DropdownAction::LaunchSettings),
            (String::from("Maximize"), DropdownAction::LaunchSettings),
            (String::from("-"), DropdownAction::LaunchSettings),
            (String::from("Close"), DropdownAction::LaunchSettings),
        ],
        "Help" => vec![
            (
                String::from("Documentation"),
                DropdownAction::LaunchSettings,
            ),
            (String::from("-"), DropdownAction::LaunchSettings),
            (String::from("About"), DropdownAction::LaunchSettings),
        ],
        _ => vec![
            // Default: show placeholder items
            (String::from("Item 1"), DropdownAction::LaunchSettings),
            (String::from("Item 2"), DropdownAction::LaunchSettings),
        ],
    }
}

/// Draw dropdown menu
fn draw_dropdown(conn: &mut Connection, surface_id: u32, dropdown: &DropdownState) {
    let Some(surface) = conn.surface_mut(surface_id) else {
        return;
    };

    let w = surface.width();
    let h = surface.height();

    surface.with_buffer(|buf, width, height| {
        // Use theme colors for dropdown
        let bg_color = Color::rgb(1.0, 1.0, 1.0); // surface
        let border_color = Color::rgb(0.392, 0.392, 0.412); // border
        let text_color = Color::rgb(0.078, 0.078, 0.094); // text_main
        let hover_color = Color::rgb(0.706, 0.706, 0.745); // primary
        let separator_color = Color::rgb(0.784, 0.784, 0.784); // separator

        fill_rect(buf, width, h, 0, 0, width, h, bg_color);

        // Draw border
        draw_rect(buf, width, h, 0, 0, width, h, border_color);

        // Draw items
        const FONT_SIZE: f32 = 13.0;
        for item in &dropdown.items {
            if item.label == "-" {
                // Separator line
                let sep_y = item.y + item.height as i32 / 2;
                draw_line(
                    buf,
                    width,
                    h,
                    4,
                    sep_y,
                    (width - 4) as i32,
                    sep_y,
                    separator_color,
                );
            } else {
                // Item background (hover effect would go here)
                fill_rect(
                    buf,
                    width,
                    h,
                    2,
                    item.y,
                    width - 4,
                    item.height,
                    hover_color,
                );

                // Draw text
                let text_x = 8;
                let text_y = item.y + ((item.height as i32 - 16) / 2).max(0);
                draw_text_helper(buf, width, h, text_x, text_y, &item.label, text_color, FONT_SIZE);
            }
        }
    });

    let _ = conn.commit(surface_id);
}

/// Draw rectangle outline
fn draw_rect(
    buf: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    color: Color,
) {
    draw_hline(buf, width, height, x, y, w, color);
    draw_hline(buf, width, height, x, y + h as i32 - 1, w, color);
    draw_vline(buf, width, height, x, y, h, color);
    draw_vline(buf, width, height, x + w as i32 - 1, y, h, color);
}

/// Draw vertical line
fn draw_vline(buf: &mut [u8], width: u32, height: u32, x: i32, y: i32, h: u32, color: Color) {
    fill_rect(buf, width, height, x, y, 1, h, color);
}

/// Helper function to draw text using Canvas
fn draw_text_helper(buf: &mut [u8], width: u32, height: u32, x: i32, y: i32, text: &str, color: Color, font_size: f32) {
    use scarlet_ui::graphics::Canvas;
    let mut canvas = Canvas::new(buf, width, height);
    canvas.draw_text_sized(x, y, text, color, font_size);
}

/// Draw line
fn draw_line(
    buf: &mut [u8],
    width: u32,
    height: u32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    color: Color,
) {
    // Simple Bresenham's line algorithm for horizontal/vertical lines
    if y1 == y2 {
        // Horizontal line
        let x_start = x1.min(x2);
        let x_end = x1.max(x2);
        draw_hline(
            buf,
            width,
            height,
            x_start,
            y1,
            (x_end - x_start + 1) as u32,
            color,
        );
    } else if x1 == x2 {
        // Vertical line
        let y_start = y1.min(y2);
        let y_end = y1.max(y2);
        draw_vline(
            buf,
            width,
            height,
            x1,
            y_start,
            (y_end - y_start + 1) as u32,
            color,
        );
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[menu_bar] Starting Scarlet Desktop Menu Bar");

    let config = load_config();

    // Use 28px height for menu bar (macOS-inspired)
    let bar_height: u32 = 28;
    let position: TaskbarPosition = TaskbarPosition::Top; // Always top for menu bar

    let mut conn = match Connection::connect("/tmp/sws.sock") {
        Ok(c) => c,
        Err(_) => {
            println!("[menu_bar] Failed to connect to SWS");
            return 1;
        }
    };

    // Get screen size first (no surface needed for SCREEN_SIZE)
    let (screen_width, screen_height) = match conn.get_screen_size() {
        Ok(size) => {
            println!("[menu_bar] Screen size: {}x{}", size.0, size.1);
            size
        }
        Err(_) => {
            println!("[menu_bar] Failed to get screen size, using defaults");
            (1920, 1080)
        }
    };

    // Create taskbar surface directly with TASKBAR type
    let surface_id = match conn.create_surface_with_type(
        "org.scarlet-os.desktop.taskbar",
        "Taskbar",
        "",
        screen_width,
        bar_height,
        window_types::TASKBAR,
    ) {
        Ok(id) => id,
        Err(_) => {
            println!("[menu_bar] Failed to create surface");
            return 1;
        }
    };

    // Disable interactive resize
    let _ = conn.set_window_resizable(surface_id, false);

    // Set window size limits
    let _ = conn.set_window_size_limits(
        surface_id,
        WindowSizeLimits {
            min_width: screen_width,
            min_height: bar_height,
            max_width: screen_width,
            max_height: bar_height,
        },
    );

    // Position at top
    let _ = conn.move_window(surface_id, 0, 0);

    // Send workarea notification (exclude menu bar from top)
    let workarea_y = bar_height as i32;
    let workarea_width = screen_width;
    let workarea_height = screen_height.saturating_sub(bar_height);
    let _ = conn.set_workarea(0, workarea_y, workarea_width, workarea_height);
    println!(
        "[menu_bar] Workarea: x=0, y={}, width={}, height={}",
        workarea_y, workarea_width, workarea_height
    );

    let mut cursor_x: i32 = 0;
    let mut cursor_y: i32 = 0;
    let mut left_down: bool = false;
    let mut pressed_items: Vec<usize> = Vec::new();
    let mut hovered_items: Vec<usize> = Vec::new();
    let mut dropdown = DropdownState::new();

    let mut seconds: u32 = 0;
    let mut tick_ms: u32 = 0;

    // Simulated system stats (placeholder for real data)
    let mut cpu_usage: u8 = 15;
    let mut memory_usage: u8 = 42;

    // Track active app for menu switching (updated via FocusChanged events)
    // Format: (app_id, app_name, title, menu_titles)
    let mut active_app: Option<(String, String, String, String)> = None;

    // Define initial menu items (will be updated dynamically)
    let mut menu_items: Vec<MenuItem> = build_menu_items(&active_app, bar_height);

    // Define status items (right side)
    let mut status_items: Vec<StatusItem> = Vec::new();
    let mut x_offset_right = screen_width as i32;

    // Clock
    let clock_text = format!("00:00");
    let (cw, _): (u32, u32) = graphics::measure_text_sized(&clock_text, 13.0);
    let clock_width: u32 = cw.saturating_add(12);
    x_offset_right -= clock_width as i32;
    status_items.push(StatusItem {
        label: String::from("uptime 0s"),
        x: x_offset_right,
        y: 0,
        width: clock_width,
        height: bar_height,
    });

    // CPU usage
    let cpu_text = format!("CPU {}%", cpu_usage);
    let (cpu_w, _): (u32, u32) = graphics::measure_text_sized(&cpu_text, 13.0);
    let cpu_width: u32 = cpu_w.saturating_add(12);
    x_offset_right -= cpu_width as i32;
    status_items.push(StatusItem {
        label: String::from("CPU 15%"),
        x: x_offset_right,
        y: 0,
        width: cpu_width,
        height: bar_height,
    });

    // Memory usage
    let mem_text = format!("Mem {}%", memory_usage);
    let (mem_w, _): (u32, u32) = graphics::measure_text_sized(&mem_text, 13.0);
    let mem_width: u32 = mem_w.saturating_add(12);
    x_offset_right -= mem_width as i32;
    status_items.push(StatusItem {
        label: String::from("Mem 42%"),
        x: x_offset_right,
        y: 0,
        width: mem_width,
        height: bar_height,
    });

    // Initial draw
    draw_menu_bar(
        &mut conn,
        surface_id,
        seconds,
        cpu_usage,
        memory_usage,
        &pressed_items,
        &hovered_items,
        &menu_items,
        &status_items,
    );

    loop {
        let _ = conn.dispatch();
        let mut needs_redraw = false;
        let mut menu_clicked: Option<usize> = None;

        while let Some(ev) = conn.poll_event() {
            match ev {
                SwsEvent::FocusChanged {
                    window_id: _,
                    app_id,
                    app_name,
                    title,
                    menu_titles,
                } => {
                    println!(
                        "[menu_bar] Focus changed event: app_id={}, app_name={}, title={}, menu_titles={}",
                        app_id, app_name, title, menu_titles
                    );
                    // Update active app based on focus change
                    // Store (app_id, app_name, title, menu_titles) for menu building
                    active_app = Some((
                        app_id.clone(),
                        app_name.clone(),
                        title.clone(),
                        menu_titles.clone(),
                    ));
                    menu_items = build_menu_items(&active_app, bar_height);
                    needs_redraw = true;
                }
                SwsEvent::Input(input) if input.surface_id == surface_id => {
                    // Check if menu was clicked (only on mouse up)
                    if input.type_ == 0x01 && input.code == 0x110 && input.value == 0 {
                        // Mouse up - check if menu item was clicked
                        for (i, item) in menu_items.iter().enumerate() {
                            if item.contains(cursor_x, cursor_y) && !pressed_items.is_empty() {
                                menu_clicked = Some(i);
                                break;
                            }
                        }
                    }

                    // Handle menu bar input
                    if handle_input(
                        input,
                        &mut cursor_x,
                        &mut cursor_y,
                        &mut left_down,
                        &mut pressed_items,
                        &mut hovered_items,
                        &menu_items,
                    ) {
                        needs_redraw = true;
                    }

                    // Close dropdown if clicking outside it
                    if dropdown.is_open()
                        && input.type_ == 0x01
                        && input.code == 0x110
                        && input.value == 0
                    {
                        // Check if click is outside both menu bar and dropdown
                        let on_menu_bar = cursor_y < bar_height as i32;
                        let on_dropdown = dropdown.contains(cursor_x, cursor_y);

                        println!(
                            "[menu_bar] Click at ({}, {}): on_menu_bar={}, on_dropdown={}, dropdown_bounds=({},{},{},{})",
                            cursor_x,
                            cursor_y,
                            on_menu_bar,
                            on_dropdown,
                            dropdown.x,
                            dropdown.y,
                            dropdown.width,
                            dropdown.height
                        );

                        if !on_menu_bar && !on_dropdown {
                            println!("[menu_bar] Closing dropdown (clicked outside)");
                            if let Some(old_id) = dropdown.surface_id {
                                let _ = conn.destroy_surface(old_id);
                            }
                            dropdown.close();
                        }
                    }
                }
                SwsEvent::Input(input) if dropdown.surface_id == Some(input.surface_id) => {
                    // Handle dropdown input
                    println!(
                        "[menu_bar] Dropdown input: type={} code={} value={} cursor=({},{})",
                        input.type_, input.code, input.value, cursor_x, cursor_y
                    );
                    if input.type_ == 0x01 && input.code == 0x110 && input.value == 0 {
                        // Mouse up on dropdown - check for item clicks
                        let mut close_dropdown = true;
                        let mut handle_launch_settings = false;
                        let mut focus_app_id: Option<String> = None;

                        // First pass: check what action to take
                        for item in &dropdown.items {
                            let contains =
                                item.contains(cursor_x, cursor_y, dropdown.x, dropdown.width);
                            println!(
                                "[menu_bar] Checking item '{}': contains={}, item_pos=({},{} {}x{}) cursor=({},{})",
                                item.label,
                                contains,
                                dropdown.x,
                                dropdown.y + item.y,
                                dropdown.width,
                                item.height,
                                cursor_x,
                                cursor_y
                            );
                            if contains {
                                // Skip separators
                                if item.label == "-" {
                                    break;
                                }
                                match &item.item_type {
                                    DropdownItemType::Window { window_id } => {
                                        println!(
                                            "[menu_bar] Window selected: {} (id={})",
                                            item.label, window_id
                                        );
                                        // Restore/focus the window
                                        let _ = conn.restore_window(*window_id);
                                    }
                                    DropdownItemType::Action { action } => match action {
                                        DropdownAction::LaunchSettings => {
                                            println!("[menu_bar] Launching Settings via stemd");
                                            handle_launch_settings = true;
                                        }
                                        DropdownAction::FocusApp { app_id } => {
                                            println!("[menu_bar] Focusing app: {}", app_id);
                                            focus_app_id = Some(app_id.clone());
                                        }
                                    },
                                }
                                break;
                            }
                        }

                        // Close dropdown before blocking operation
                        if close_dropdown {
                            if let Some(old_id) = dropdown.surface_id {
                                let _ = conn.destroy_surface(old_id);
                            }
                            dropdown.close();
                        }

                        // Handle actions after dropdown is closed
                        if handle_launch_settings {
                            launch_or_focus_app("org.scarlet-os.desktop.settings");
                        }
                        if let Some(app_id) = focus_app_id {
                            launch_or_focus_app(&app_id);
                        }
                    }
                }
                SwsEvent::SurfaceConfigure {
                    surface_id: sid,
                    width,
                    height,
                } if sid == surface_id => {
                    println!("[menu_bar] SurfaceConfigure: {}x{}", width, height);
                }
                SwsEvent::SurfaceDestroyed { surface_id: sid } if sid == surface_id => {
                    println!("[menu_bar] Menu bar destroyed");
                    // Also close dropdown if open
                    if let Some(old_id) = dropdown.surface_id {
                        let _ = conn.destroy_surface(old_id);
                    }
                    return 0;
                }
                SwsEvent::SurfaceDestroyed { surface_id: sid }
                    if dropdown.surface_id == Some(sid) =>
                {
                    println!("[menu_bar] Dropdown destroyed externally");
                    dropdown.close();
                }
                _ => {}
            }
        }

        // Handle menu click after event processing
        if let Some(menu_idx) = menu_clicked {
            let label = &menu_items[menu_idx].label;
            println!("[menu_bar] Menu clicked: {}", label);

            // Show dropdown based on menu type
            if label == "Scarlet" {
                // Show system menu dropdown (Settings, etc.)
                show_app_menu_dropdown(&mut conn, &mut dropdown, &menu_items[menu_idx], menu_idx);
            } else {
                // Show app-specific menu dropdown
                show_app_menu_dropdown(&mut conn, &mut dropdown, &menu_items[menu_idx], menu_idx);
            }
        }

        // Update clock and system stats
        tick_ms = tick_ms.saturating_add(16);
        if tick_ms >= 1000 {
            tick_ms = 0;
            seconds = seconds.saturating_add(1);

            // Update clock text
            let mins = (seconds / 60) % 60;
            let secs = seconds % 60;
            status_items[2].label = format!("uptime {:02}:{:02}", mins, secs);

            // Simulate CPU/memory changes
            cpu_usage = (cpu_usage.wrapping_add(7) % 85) + 10;
            memory_usage = (memory_usage.wrapping_add(3) % 70) + 25;

            status_items[1].label = format!("CPU {}%", cpu_usage);
            status_items[0].label = format!("Mem {}%", memory_usage);

            needs_redraw = true;
        }

        // Note: Active app is now updated via FocusChanged events instead of polling

        if needs_redraw {
            draw_menu_bar(
                &mut conn,
                surface_id,
                seconds,
                cpu_usage,
                memory_usage,
                &pressed_items,
                &hovered_items,
                &menu_items,
                &status_items,
            );
        }

        thread::sleep(Duration::from_millis(16));
    }
}
