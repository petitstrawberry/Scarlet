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

extern crate scarlet_desktop_config;
extern crate scarlet_std as std;

use core::time::Duration;
use scarlet_desktop_config::{TaskbarConfig, TaskbarPosition};
use scarlet_ui::Color;
use scarlet_ui::graphics::{Canvas, measure_text_sized};
use std::string::String;
use std::thread;
use std::vec::Vec;
use std::{format, println};
use sws_client::{Connection, Event, InputEvent, WindowSizeLimits};
use sws_protocol::window_types;

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
    label: &'static str,
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

fn handle_input(
    ev: InputEvent,
    cursor_x: &mut i32,
    cursor_y: &mut i32,
    left_down: &mut bool,
    pressed_items: &mut Vec<usize>,
    menu_items: &[MenuItem],
) -> bool {
    match ev.type_ {
        0x03 => {
            // EV_ABS
            match ev.code {
                0x00 => *cursor_x = ev.value, // ABS_X
                0x01 => *cursor_y = ev.value, // ABS_Y
                _ => {}
            }
            false
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
                            let label = menu_items[idx].label;
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
    menu_items: &[MenuItem],
    status_items: &[StatusItem],
) {
    let Some(surface) = conn.surface_mut(surface_id) else {
        return;
    };

    let w = surface.width();
    let h = surface.height();

    // macOS-inspired colors
    let bg_color = Color::rgb(30, 30, 30);
    let text_color = Color::rgb(230, 230, 230);
    let text_dim = Color::rgb(160, 160, 160);
    let border_color = Color::rgb(50, 50, 50);
    let hover_bg = Color::rgb(60, 60, 60);
    let active_bg = Color::rgb(80, 80, 80);

    surface.with_buffer(|buf, width, height| {
        let mut canvas = Canvas::new(buf, width, height);

        // Bar background
        canvas.fill_rect(0, 0, w, h, bg_color);
        canvas.draw_hline(0, (h - 1) as i32, w, border_color);

        // Draw menu items (left side)
        let font_size = 13.0;
        for (i, item) in menu_items.iter().enumerate() {
            let is_pressed = pressed_items.contains(&i);

            // Draw hover/active background
            if is_pressed {
                canvas.fill_rect(
                    item.x,
                    item.y + 1,
                    item.width,
                    item.height.saturating_sub(2),
                    active_bg,
                );
            }

            // Draw text
            let text_x = item.x + 8;
            let text_y = item.y + ((item.height as i32 - 16) / 2).max(0);
            canvas.draw_text_sized(text_x, text_y, item.label, text_color, font_size);
        }

        // Draw status items (right side)
        for item in status_items {
            // Draw text
            let text_x = item.x + 6;
            let text_y = item.y + ((item.height as i32 - 16) / 2).max(0);
            canvas.draw_text_sized(text_x, text_y, &item.label, text_dim, font_size);
        }
    });

    let _ = conn.commit(surface_id);
}

/// Show window list dropdown menu
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
    const MAX_HEIGHT: u32 = 400;
    const DROPDOWN_WIDTH: u32 = 250;

    let item_count = windows.len().min(15); // Max 15 items visible
    let dropdown_height = (item_count as u32) * ITEM_HEIGHT + 4; // +4 for border

    // Create dropdown surface (AlwaysOnTop for popup)
    let surface_id = match conn.create_surface(DROPDOWN_WIDTH, dropdown_height) {
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
        "[menu_bar] Dropdown shown: {} windows at ({}, {})",
        windows.len(),
        x,
        y
    );
}

/// Show application menu dropdown (Scarlet menu with Settings, etc.)
fn show_app_menu_dropdown(
    conn: &mut Connection,
    dropdown: &mut DropdownState,
    menu_item: &MenuItem,
    menu_index: usize,
) {
    println!("[menu_bar] Showing app menu dropdown");

    // Close any existing dropdown
    if let Some(old_id) = dropdown.surface_id {
        let _ = conn.destroy_surface(old_id);
    }

    // Calculate dropdown size
    const ITEM_HEIGHT: u32 = 28;
    const DROPDOWN_WIDTH: u32 = 200;
    const NUM_ITEMS: usize = 1; // Only "Settings" for now

    let dropdown_height = NUM_ITEMS as u32 * ITEM_HEIGHT + 4;

    // Create dropdown surface (AlwaysOnTop for popup)
    let surface_id = match conn.create_surface(DROPDOWN_WIDTH, dropdown_height) {
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

    // Add Settings item
    dropdown.items.push(DropdownItem {
        label: String::from("Settings..."),
        item_type: DropdownItemType::Action {
            action: DropdownAction::LaunchSettings,
        },
        y: current_y,
        height: ITEM_HEIGHT,
    });
    current_y += ITEM_HEIGHT as i32;

    // Update dropdown state
    dropdown.surface_id = Some(surface_id);
    dropdown.x = x;
    dropdown.y = y;
    dropdown.width = DROPDOWN_WIDTH;
    dropdown.height = dropdown_height;
    dropdown.menu_index = Some(menu_index);

    // Draw dropdown
    draw_dropdown(conn, surface_id, dropdown);

    println!("[menu_bar] App menu dropdown shown at ({}, {})", x, y);
}

/// Draw dropdown menu
fn draw_dropdown(conn: &mut Connection, surface_id: u32, dropdown: &DropdownState) {
    let Some(surface) = conn.surface_mut(surface_id) else {
        return;
    };

    let w = surface.width();
    let h = surface.height();

    surface.with_buffer(|buf, width, height| {
        let mut canvas = Canvas::new(buf, width, height);

        // Background
        let bg_color = Color::rgb(40, 40, 40);
        let border_color = Color::rgb(70, 70, 70);
        let text_color = Color::rgb(220, 220, 220);
        let hover_color = Color::rgb(70, 130, 180); // Steel blue
        let separator_color = Color::rgb(60, 60, 60);

        canvas.fill_rect(0, 0, width, height, bg_color);

        // Draw border
        canvas.draw_rect(0, 0, width, height, border_color);

        // Draw items
        const FONT_SIZE: f32 = 13.0;
        for item in &dropdown.items {
            // Item background (hover effect would go here)
            canvas.fill_rect(2, item.y, width - 4, item.height, Color::rgb(45, 45, 45));

            // Draw text
            let text_x = 8;
            let text_y = item.y + ((item.height as i32 - 16) / 2).max(0);
            canvas.draw_text_sized(text_x, text_y, &item.label, text_color, FONT_SIZE);

            // Draw separator
            canvas.draw_line(
                2,
                item.y + item.height as i32,
                (width - 2) as i32,
                item.y + item.height as i32,
                separator_color,
            );
        }
    });

    let _ = conn.commit(surface_id);
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

    // Create a dummy surface first (needed for SCREEN_SIZE response routing)
    let dummy_surface_id = match conn.create_surface(16, 16) {
        Ok(id) => id,
        Err(_) => {
            println!("[menu_bar] Failed to create dummy surface");
            return 1;
        }
    };

    // Get screen size first
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

    // Destroy dummy surface
    let _ = conn.destroy_surface(dummy_surface_id);

    // Create surface with full screen width and 28px height
    let surface_id = match conn.create_surface(screen_width, bar_height) {
        Ok(id) => id,
        Err(_) => {
            println!("[menu_bar] Failed to create surface");
            return 1;
        }
    };

    let _ = conn.set_window_type(surface_id, window_types::TASKBAR);

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
    let mut dropdown = DropdownState::new();

    let mut seconds: u32 = 0;
    let mut tick_ms: u32 = 0;

    // Simulated system stats (placeholder for real data)
    let mut cpu_usage: u8 = 15;
    let mut memory_usage: u8 = 42;

    // Define menu items (left side)
    let mut menu_items: Vec<MenuItem> = Vec::new();
    let menus = ["Scarlet", "File", "Edit", "View", "Window", "Help"];
    let mut x_offset = 0i32;

    for label in &menus {
        let (tw, _) = measure_text_sized(label, 13.0);
        let width = tw.saturating_add(16).min(100); // Add padding, cap at 100px
        menu_items.push(MenuItem {
            label,
            x: x_offset,
            y: 0,
            width,
            height: bar_height,
        });
        x_offset += width as i32;
    }

    // Define status items (right side)
    let mut status_items: Vec<StatusItem> = Vec::new();
    let mut x_offset_right = screen_width as i32;

    // Clock
    let clock_text = format!("00:00");
    let (cw, _) = measure_text_sized(&clock_text, 13.0);
    let clock_width = cw.saturating_add(12);
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
    let (cpu_w, _) = measure_text_sized(&cpu_text, 13.0);
    let cpu_width = cpu_w.saturating_add(12);
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
    let (mem_w, _) = measure_text_sized(&mem_text, 13.0);
    let mem_width = mem_w.saturating_add(12);
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
        &menu_items,
        &status_items,
    );

    loop {
        let _ = conn.dispatch();
        let mut needs_redraw = false;
        let mut menu_clicked: Option<usize> = None;

        while let Some(ev) = conn.poll_event() {
            match ev {
                Event::Input(input) if input.surface_id == surface_id => {
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

                        if !on_menu_bar && !on_dropdown {
                            println!("[menu_bar] Closing dropdown (clicked outside)");
                            if let Some(old_id) = dropdown.surface_id {
                                let _ = conn.destroy_surface(old_id);
                            }
                            dropdown.close();
                        }
                    }
                }
                Event::Input(input) if dropdown.surface_id == Some(input.surface_id) => {
                    // Handle dropdown input
                    if input.type_ == 0x01 && input.code == 0x110 && input.value == 0 {
                        // Mouse up on dropdown - check for item clicks
                        for item in &dropdown.items {
                            if item.contains(cursor_x, cursor_y, dropdown.x, dropdown.width) {
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
                                            println!("[menu_bar] Launching Settings");
                                            spawn_application("/bin/scarlet_desktop_settings", &[]);
                                        }
                                    },
                                }
                                // Close dropdown after selection
                                if let Some(old_id) = dropdown.surface_id {
                                    let _ = conn.destroy_surface(old_id);
                                }
                                dropdown.close();
                                break;
                            }
                        }
                    }
                }
                Event::SurfaceConfigure {
                    surface_id: sid,
                    width,
                    height,
                } if sid == surface_id => {
                    println!("[menu_bar] SurfaceConfigure: {}x{}", width, height);
                }
                Event::SurfaceDestroyed { surface_id: sid } if sid == surface_id => {
                    println!("[menu_bar] Menu bar destroyed");
                    // Also close dropdown if open
                    if let Some(old_id) = dropdown.surface_id {
                        let _ = conn.destroy_surface(old_id);
                    }
                    return 0;
                }
                Event::SurfaceDestroyed { surface_id: sid } if dropdown.surface_id == Some(sid) => {
                    println!("[menu_bar] Dropdown destroyed externally");
                    dropdown.close();
                }
                _ => {}
            }
        }

        // Handle menu click after event processing
        if let Some(menu_idx) = menu_clicked {
            let label = menu_items[menu_idx].label;
            println!("[menu_bar] Menu clicked: {}", label);

            // Show app menu dropdown for "Scarlet" menu (index 0)
            if label == "Scarlet" {
                show_app_menu_dropdown(&mut conn, &mut dropdown, &menu_items[menu_idx], menu_idx);
            }
            // Show window list dropdown for "Window" menu (index 4)
            else if label == "Window" {
                show_window_list_dropdown(
                    &mut conn,
                    &mut dropdown,
                    &menu_items[menu_idx],
                    menu_idx,
                );
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

        if needs_redraw {
            draw_menu_bar(
                &mut conn,
                surface_id,
                seconds,
                cpu_usage,
                memory_usage,
                &pressed_items,
                &menu_items,
                &status_items,
            );
        }

        thread::sleep(Duration::from_millis(16));
    }
}
