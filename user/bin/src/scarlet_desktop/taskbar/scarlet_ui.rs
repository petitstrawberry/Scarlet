//! Scarlet Desktop TaskBar (ScarletUI version)
//!
//! macOS-style menu bar implemented with ScarletUI

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_desktop_config;
extern crate scarlet_std as std;
extern crate scarlet_ui;
extern crate scarlet_ui_macros;

use alloc::collections::BTreeMap;
use alloc::vec;
use core::time::Duration;
use scarlet_os::time;
use scarlet_ui::buffer::Buffer;
use scarlet_ui::color::Color;
use scarlet_ui::element::{ElementRenderObject, LayoutConstraints};
use scarlet_ui::geometry::Size;
use scarlet_ui::graphics;
use scarlet_ui::prelude::*;
use scarlet_ui::views::menu::MenuRenderObject;
use scarlet_ui::views::{MenuAction, MenuBar, MenuItem, MenuItemContent};
use scarlet_ui::{MenuBarModel, MenuItemModel};
use scarlet_ui::{StateId, hstack};
use scarlet_ui_macros::View;
use serde::Deserialize;
use serde_json_core::from_str;
use std::io::Write;
use std::string::{String, ToString};
use std::vec::Vec;
use std::{format, println};
use sws_client as sws;
use sws_protocol::window_types;

const SWS_CONNECT_RETRIES: usize = 100;
const SWS_RETRY_DELAY_MS: u64 = 50;

type SwsScreenConnection = (sws::Connection, u32, u32, u32);

fn scale_milli_or_default(scale_milli: u32) -> u32 {
    scale_milli.max(1)
}

fn scale_u32(value: u32, scale_milli: u32) -> u32 {
    let scale_milli = scale_milli_or_default(scale_milli) as u64;
    (((value as u64) * scale_milli + 999) / 1000).max(1) as u32
}

fn scale_i32(value: i32, scale_milli: u32) -> i32 {
    let scale_milli = scale_milli_or_default(scale_milli) as i64;
    ((value as i64) * scale_milli / 1000) as i32
}

fn unscale_u32(value: u32, scale_milli: u32) -> u32 {
    let scale_milli = scale_milli_or_default(scale_milli) as u64;
    (((value as u64) * 1000 + scale_milli - 1) / scale_milli).max(1) as u32
}

fn unscale_i32(value: i32, scale_milli: u32) -> i32 {
    let scale_milli = scale_milli_or_default(scale_milli) as i64;
    ((value as i64) * 1000 / scale_milli) as i32
}

fn query_output_scale(conn: &mut sws::Connection) -> u32 {
    conn.get_output_scale().unwrap_or(1000).max(1)
}

fn connect_sws_with_screen_size_retry() -> core::result::Result<SwsScreenConnection, ()> {
    for attempt in 0..SWS_CONNECT_RETRIES {
        if let Ok(mut conn) = sws::Connection::connect("/tmp/sws.sock")
            && let Ok((physical_width, physical_height)) = conn.get_screen_size()
        {
            let scale_milli = query_output_scale(&mut conn);
            let width = unscale_u32(physical_width, scale_milli);
            let height = unscale_u32(physical_height, scale_milli);
            println!(
                "[TaskBar] Connected to SWS after {} attempt(s); screen={}x{} scale_milli={}",
                attempt + 1,
                width,
                height,
                scale_milli
            );
            return Ok((conn, width, height, scale_milli));
        }

        std::thread::sleep(Duration::from_millis(SWS_RETRY_DELAY_MS));
    }

    Err(())
}

/// TaskBar Application
#[derive(View, Clone)]
struct TaskBarApp {
    cpu_usage: State<u8>,
    memory_usage: State<u8>,
    clock: State<u32>,
    screen_width: State<f32>,
    menu_bar: State<MenuBarModel>,
    active_window_id: State<u32>,
    menu_tree: State<MenuTree>,
    open_menu_index: State<Option<usize>>,
    popup_surface_id: State<Option<u32>>,
    menu_titles_cache: State<BTreeMap<u32, String>>,
}

impl TaskBarApp {
    fn new() -> Self {
        Self {
            cpu_usage: State::new(StateId::new(0), 15),
            memory_usage: State::new(StateId::new(1), 42),
            clock: State::new(StateId::new(2), 0),
            screen_width: State::new(StateId::new(3), 1920.0),
            menu_bar: State::new(
                StateId::new(4),
                MenuBarModel::new(vec![MenuItemModel::new("system_scarlet", "Scarlet")]),
            ),
            active_window_id: State::new(StateId::new(5), 0),
            menu_tree: State::new(
                StateId::new(6),
                MenuTree {
                    items: vec![TaskMenuItem {
                        id: String::from("system_scarlet"),
                        title: String::from("Scarlet"),
                        enabled: true,
                        shortcut: None,
                        children: default_system_menu_entries(),
                    }],
                },
            ),
            open_menu_index: State::new(StateId::new(7), None),
            popup_surface_id: State::new(StateId::new(8), None),
            menu_titles_cache: State::new(StateId::new(9), BTreeMap::new()),
        }
    }
}

#[derive(Clone, Default)]
struct MenuTree {
    items: Vec<TaskMenuItem>,
}

#[derive(Clone)]
struct TaskMenuItem {
    id: String,
    title: String,
    enabled: bool,
    shortcut: Option<String>,
    children: Vec<TaskMenuEntry>,
}

#[derive(Clone)]
enum TaskMenuEntry {
    Item(TaskMenuItem),
    Separator,
}

#[derive(Deserialize)]
struct MenuTreePayload {
    items: Vec<MenuEntryPayload>,
}

#[derive(Deserialize)]
struct MenuEntryPayload {
    separator: Option<bool>,
    id: Option<String>,
    title: Option<String>,
    enabled: Option<bool>,
    shortcut: Option<String>,
    items: Option<Vec<MenuEntryPayload>>,
}

fn default_system_menu_entries() -> Vec<TaskMenuEntry> {
    vec![
        TaskMenuEntry::Item(TaskMenuItem {
            id: String::from("system_terminal"),
            title: String::from("Terminal"),
            enabled: true,
            shortcut: None,
            children: Vec::new(),
        }),
        TaskMenuEntry::Item(TaskMenuItem {
            id: String::from("system_clock"),
            title: String::from("Clock"),
            enabled: true,
            shortcut: None,
            children: Vec::new(),
        }),
        TaskMenuEntry::Item(TaskMenuItem {
            id: String::from("system_settings"),
            title: String::from("Settings"),
            enabled: true,
            shortcut: None,
            children: Vec::new(),
        }),
        TaskMenuEntry::Separator,
        TaskMenuEntry::Item(TaskMenuItem {
            id: String::from("system_quit"),
            title: String::from("Shutdown"),
            enabled: true,
            shortcut: None,
            children: Vec::new(),
        }),
    ]
}

fn launch_app(app_id: &[u8]) {
    send_launch_command(0x01, app_id);
}

fn launch_new_app(app_id: &[u8]) {
    send_launch_command(0x05, app_id);
}

fn send_launch_command(command: u8, app_id: &[u8]) {
    if let Ok(mut stream) = std::socket::Socket::new()
        && stream.connect("/tmp/stemd.sock").is_ok()
    {
        let exec_path = b"";
        let mut msg = alloc::vec::Vec::new();
        msg.push(command);
        msg.extend_from_slice(&(app_id.len() as u32).to_le_bytes());
        msg.extend_from_slice(app_id);
        msg.extend_from_slice(&(exec_path.len() as u32).to_le_bytes());
        msg.extend_from_slice(exec_path);
        let _ = stream.write(&msg);
    }
}

const MENU_BAR_FONT_SIZE: f32 = 16.0;
const MENU_BAR_ITEM_PADDING: f32 = 8.0;
const MENU_BAR_ITEM_SPACING: f32 = 2.0;
const MENU_BAR_OUTER_PADDING: f32 = 8.0;
const MENU_BAR_MAX_APP_LABEL: usize = 18;
const TASKBAR_HEIGHT: u32 = 40;

fn menu_bar_label(title: &str) -> String {
    if title.chars().count() <= MENU_BAR_MAX_APP_LABEL {
        return title.to_string();
    }
    let mut shortened = String::new();
    for ch in title.chars().take(MENU_BAR_MAX_APP_LABEL.saturating_sub(3)) {
        shortened.push(ch);
    }
    shortened.push_str("...");
    shortened
}

fn menu_bar_item_width(label: &str) -> f32 {
    let (text_w, _text_h) = graphics::measure_text_sized(label, MENU_BAR_FONT_SIZE);
    text_w as f32 + MENU_BAR_ITEM_PADDING * 2.0
}

fn menu_bar_popup_x(items: &[TaskMenuItem], index: usize) -> f32 {
    let mut x = MENU_BAR_OUTER_PADDING;
    for (i, item) in items.iter().enumerate() {
        if i >= index {
            break;
        }
        x += menu_bar_item_width(&item.title) + MENU_BAR_ITEM_SPACING;
    }
    x
}

fn build_menu_tree(app_name: &str, menu_titles: &str) -> MenuTree {
    let mut items = Vec::new();
    items.push(TaskMenuItem {
        id: String::from("system_scarlet"),
        title: String::from("Scarlet"),
        enabled: true,
        shortcut: None,
        children: default_system_menu_entries(),
    });

    let cleaned = sanitize_menu_json(menu_titles);
    let trimmed = cleaned.trim();
    let parsed: Vec<TaskMenuItem> = if trimmed.is_empty() {
        Vec::new()
    } else if trimmed.starts_with('{') {
        parse_menu_tree_json(trimmed)
    } else {
        trimmed
            .split('|')
            .map(|s| TaskMenuItem {
                id: s.to_string(),
                title: s.to_string(),
                enabled: true,
                shortcut: None,
                children: Vec::new(),
            })
            .collect()
    };

    if !app_name.is_empty() {
        let app_label = menu_bar_label(app_name);

        let mut app_children = Vec::new();
        for item in &parsed {
            if item.id == "__app__" {
                app_children.extend(item.children.iter().cloned());
            } else {
                items.push(item.clone());
            }
        }

        items.push(TaskMenuItem {
            id: String::from("system_app"),
            title: app_label,
            enabled: true,
            shortcut: None,
            children: app_children,
        });
    } else {
        items.extend(parsed);
    }

    MenuTree { items }
}

fn menu_bar_from_tree(tree: &MenuTree) -> MenuBarModel {
    let items = tree
        .items
        .iter()
        .map(|item| MenuItemModel::new(item.id.clone(), item.title.clone()))
        .collect();
    MenuBarModel::new(items)
}

fn menu_height(entries: &[TaskMenuEntry], item_height: f32) -> f32 {
    let mut total = 4.0;
    for entry in entries {
        total += match entry {
            TaskMenuEntry::Separator => 1.0,
            TaskMenuEntry::Item(_) => item_height,
        };
    }
    total
}

fn parse_menu_tree_json(input: &str) -> Vec<TaskMenuItem> {
    let cleaned = sanitize_menu_json(input);
    let trimmed = cleaned.trim();
    let candidate = match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(start), Some(end)) if start < end => &trimmed[start..=end],
        _ => trimmed,
    };
    let Ok((payload, _)) = from_str::<MenuTreePayload>(candidate) else {
        println!(
            "[TaskBar] Failed to parse menu JSON (len={}, cleaned_len={}, candidate_len={})",
            input.len(),
            cleaned.len(),
            candidate.len()
        );
        return Vec::new();
    };
    payload
        .items
        .into_iter()
        .filter_map(build_menu_entry)
        .filter_map(|entry| match entry {
            TaskMenuEntry::Item(item) => Some(item),
            TaskMenuEntry::Separator => None,
        })
        .collect()
}

fn sanitize_menu_json(input: &str) -> String {
    // Pre-allocate with capacity to reduce reallocations
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch == '\0' {
            break;
        }
        if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' {
            continue;
        }
        out.push(ch);
    }
    // Shrink to fit to free unused capacity immediately
    out.shrink_to_fit();
    out
}

fn build_menu_entry(entry: MenuEntryPayload) -> Option<TaskMenuEntry> {
    if entry.separator.unwrap_or(false) {
        return Some(TaskMenuEntry::Separator);
    }

    // Use unwrap_or_default() efficiently to avoid multiple moves
    let resolved_id = entry.id.unwrap_or_default();
    let resolved_title = entry.title.unwrap_or_default();

    if resolved_id.is_empty() && resolved_title.is_empty() {
        return None;
    }

    // Avoid clones by using the values directly
    let (final_id, final_title) = if resolved_id.is_empty() {
        (&resolved_title, &resolved_title)
    } else if resolved_title.is_empty() {
        (&resolved_id, &resolved_id)
    } else {
        (&resolved_id, &resolved_title)
    };

    let children = entry
        .items
        .unwrap_or_default()
        .into_iter()
        .filter_map(build_menu_entry)
        .collect();

    Some(TaskMenuEntry::Item(TaskMenuItem {
        id: final_id.clone(),
        title: final_title.clone(),
        enabled: entry.enabled.unwrap_or(true),
        shortcut: entry.shortcut,
        children,
    }))
}

fn build_menu_bar_view(
    items: &[TaskMenuItem],
    active_window_id: u32,
    open_menu_index: State<Option<usize>>,
) -> MenuBar {
    println!(
        "[TaskBar] build_menu_bar_view: {} items, active_window_id={}",
        items.len(),
        active_window_id
    );
    let has_children_by_index: Vec<bool> =
        items.iter().map(|item| !item.children.is_empty()).collect();
    let entries = items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            // Avoid creating intermediate Strings - use references directly
            // MenuItem::new will handle the conversion
            let has_children = !item.children.is_empty();
            let open_state_hover = open_menu_index.clone();
            let open_state_click = open_menu_index.clone();
            let is_open = open_menu_index.get() == Some(idx);

            MenuItem::new(item.title.as_str())
                .font_size(MENU_BAR_FONT_SIZE)
                .padding(MENU_BAR_ITEM_PADDING)
                .selected(is_open)
                .on_hover(move || {
                    if !has_children {
                        return;
                    }
                    if open_state_hover.get().is_some() && open_state_hover.get() != Some(idx) {
                        open_state_hover.set(Some(idx));
                    }
                })
                .on_click(move || {
                    if has_children {
                        if open_state_click.get() == Some(idx) {
                            open_state_click.set(None);
                        } else {
                            open_state_click.set(Some(idx));
                        }
                    } else {
                        open_state_click.set(None);
                    }
                })
        })
        .collect();
    let open_state_bar = open_menu_index.clone();
    let hover_children = has_children_by_index.clone();
    MenuBar::new(entries)
        .spacing(MENU_BAR_ITEM_SPACING)
        .on_hover_index(move |idx| {
            if !hover_children.get(idx).copied().unwrap_or(false) {
                return;
            }
            if open_state_bar.get().is_some() && open_state_bar.get() != Some(idx) {
                open_state_bar.set(Some(idx));
            }
        })
}

fn build_menu_items(
    entries: &[TaskMenuEntry],
    active_window_id: u32,
    open_menu_index: State<Option<usize>>,
) -> (Vec<MenuItemContent>, f32) {
    let mut items = Vec::new();
    for entry in entries {
        match entry {
            TaskMenuEntry::Separator => {
                items.push(MenuItemContent::separator());
            }
            TaskMenuEntry::Item(item) => {
                let mut content = MenuItemContent::new(item.title.clone())
                    .action(MenuAction::Submenu)
                    .enabled(item.enabled);
                if let Some(ref shortcut) = item.shortcut {
                    content = content.shortcut(shortcut.clone());
                }
                let item_id = item.id.clone();
                let open_state = open_menu_index.clone();
                let window_id = active_window_id;
                content = content.callback(move || {
                    open_state.set(None);
                    // Handle system menu items
                    if item_id == "system_terminal" {
                        launch_new_app(b"org.scarlet-os.desktop.terminal");
                        return;
                    }
                    if item_id == "system_clock" {
                        launch_new_app(b"org.scarlet-os.desktop.clock");
                        return;
                    }
                    if item_id == "system_settings" {
                        launch_app(b"org.scarlet-os.desktop.settings");
                        return;
                    }
                    if item_id == "system_quit" {
                        // TODO: Show shutdown dialog
                        println!("[TaskBar] System shutdown requested");
                        return;
                    }
                    // Handle application menu items
                    if window_id == 0 || item_id.starts_with("system_") {
                        return;
                    }
                    if let Ok(mut conn) = sws::Connection::connect("/tmp/sws.sock") {
                        let _ = conn.activate_menu_item(window_id, &item_id);
                    }
                });
                items.push(content);
            }
        }
    }
    let item_height = 28.0;
    let height = menu_height(entries, item_height);
    (items, height)
}

struct PopupMenuRenderer {
    render_object: MenuRenderObject,
    size: Size,
    scale_milli: u32,
}

impl PopupMenuRenderer {
    fn new(items: Vec<MenuItemContent>, item_height: f32, width: f32, scale_milli: u32) -> Self {
        graphics::set_current_scale_milli(scale_milli);
        let mut render_object = MenuRenderObject::new(items, item_height, width);
        let constraints = LayoutConstraints {
            min_width: width,
            max_width: width,
            min_height: 0.0,
            max_height: f32::INFINITY,
        };
        let size = render_object.layout(constraints);
        render_object.render();
        Self {
            render_object,
            size,
            scale_milli,
        }
    }

    fn size(&self) -> Size {
        self.size
    }

    fn handle_move(&mut self, x: f32, y: f32) -> bool {
        let hovered = self.render_object.hit_test(x, y);
        if hovered != self.render_object.hovered() {
            self.render_object.set_hovered(hovered);
            true
        } else {
            false
        }
    }

    fn handle_click(&self, x: f32, y: f32) {
        if let Some(index) = self.render_object.hit_test(x, y) {
            self.render_object.invoke_item(index);
        }
    }

    fn render(&mut self) {
        graphics::set_current_scale_milli(self.scale_milli);
        self.render_object.render();
    }

    fn buffer(&self) -> Option<&Buffer> {
        self.render_object.get_buffer()
    }
}

impl Application for TaskBarApp {
    fn on_focus_changed(&mut self, window_id: u32, app_name: &str, menu_titles: &str) {
        println!(
            "[TaskBar] on_focus_changed: window_id={}, app_name={}, menu_titles={}",
            window_id, app_name, menu_titles
        );
        let resolved_menu_titles = if menu_titles.is_empty() {
            self.menu_titles_cache
                .get()
                .get(&window_id)
                .cloned()
                .unwrap_or_default()
        } else {
            let owned = menu_titles.to_string();
            self.menu_titles_cache.update(|cache| {
                cache.insert(window_id, owned.clone());
            });
            owned
        };
        self.update_menu_for_app(window_id, app_name, &resolved_menu_titles);
    }

    fn on_active_app_changed(&mut self, window_id: u32, app_name: &str, menu_titles: &str) {
        println!(
            "[TaskBar] on_active_app_changed: window_id={}, app_name={}, menu_titles={}",
            window_id, app_name, menu_titles
        );
        let resolved_menu_titles = if menu_titles.is_empty() {
            self.menu_titles_cache
                .get()
                .get(&window_id)
                .cloned()
                .unwrap_or_default()
        } else {
            let owned = menu_titles.to_string();
            self.menu_titles_cache.update(|cache| {
                cache.insert(window_id, owned.clone());
            });
            owned
        };
        self.update_menu_for_app(window_id, app_name, &resolved_menu_titles);
    }

    fn on_resize(&mut self, width: u32, height: u32) {
        println!("[TaskBar] on_resize: width={}, height={}", width, height);
        self.screen_width.set(width as f32);
        self.open_menu_index.set(None);
        self.update_workarea_from_screen_query(width, TASKBAR_HEIGHT);
    }

    fn on_screen_size_changed(&mut self, width: u32, height: u32) -> Option<Size> {
        println!("[TaskBar] on_screen_size_changed: {}x{}", width, height);
        self.screen_width.set(width as f32);
        self.open_menu_index.set(None);
        self.update_workarea(width, height, TASKBAR_HEIGHT);
        Some(Size::new(width as f32, TASKBAR_HEIGHT as f32))
    }

    fn body(&self) -> impl View {
        let cpu = self.cpu_usage.get();
        let mem = self.memory_usage.get();
        let clock = self.clock.get();
        let screen_width = self.screen_width.get();
        let _menu_bar = self.menu_bar.get();
        let menu_tree = self.menu_tree.get();
        println!(
            "[TaskBar] body() called: menu_tree has {} items",
            menu_tree.items.len()
        );
        let active_window_id = self.active_window_id.get();

        let hours = clock / 3600;
        let mins = (clock / 60) % 60;

        let bar_height = TASKBAR_HEIGHT as f32;
        let window_height = bar_height;

        Window::new("TaskBar",
            hstack! {
                build_menu_bar_view(&menu_tree.items, active_window_id, self.open_menu_index.clone()),
                Spacer::new(),
                Text::new(format!("Mem {}%", mem))
                    .font_size(12.0)
                    .color(Color::rgb(0.280, 0.280, 0.310)),
                Text::new("•")
                    .font_size(12.0)
                    .color(Color::rgb(0.600, 0.600, 0.630)),
                Text::new(format!("CPU {}%", cpu))
                    .font_size(12.0)
                    .color(Color::rgb(0.280, 0.280, 0.310)),
                Text::new("•")
                    .font_size(12.0)
                    .color(Color::rgb(0.600, 0.600, 0.630)),
                Text::new(format!("{:02}:{:02}", hours, mins))
                    .font_size(12.0)
                    .color(Color::rgb(0.280, 0.280, 0.310)),
            }
            .spacing(10.0)
            .alignment(Alignment::Center)
            .padding(MENU_BAR_OUTER_PADDING)
        )
        .app_id("org.scarlet-os.desktop.taskbar")
        .decorated(false)
        .background_color(Color::rgb(0.940, 0.940, 0.960))
        .window_type(scarlet_ui::views::window_type::TASKBAR)
        .active_on_focus(false)
        .resizable(false)
        .movable(false)
        .size(Size::new(screen_width, window_height))
    }

    fn init(&mut self) {
        println!("[TaskBar] Initializing ScarletUI TaskBar");
        // Screen size will be obtained by sws_client in main()
        self.start_background_tasks();
    }
}

impl TaskBarApp {
    fn update_workarea(&self, screen_width: u32, screen_height: u32, bar_height: u32) {
        if let Ok(mut conn) = sws::Connection::connect("/tmp/sws.sock") {
            let scale_milli = query_output_scale(&mut conn);
            let physical_width = scale_u32(screen_width, scale_milli);
            let physical_height = scale_u32(screen_height, scale_milli);
            let workarea_y = scale_i32(bar_height as i32, scale_milli);
            let workarea_height =
                physical_height.saturating_sub(scale_u32(bar_height, scale_milli));
            let _ = conn.set_workarea(0, workarea_y, physical_width, workarea_height);
            println!(
                "[TaskBar] Workarea: x=0, y={}, width={}, height={}",
                workarea_y, physical_width, workarea_height
            );
        }
    }

    fn update_workarea_from_screen_query(&self, fallback_width: u32, bar_height: u32) {
        if let Ok(mut conn) = sws::Connection::connect("/tmp/sws.sock") {
            if let Ok((screen_width, screen_height)) = conn.get_screen_size() {
                let scale_milli = query_output_scale(&mut conn);
                let physical_bar_height = scale_u32(bar_height, scale_milli);
                let workarea_y = physical_bar_height as i32;
                let workarea_height = screen_height.saturating_sub(physical_bar_height);
                let _ = conn.set_workarea(0, workarea_y, screen_width, workarea_height);
                println!(
                    "[TaskBar] Workarea: x=0, y={}, width={}, height={}",
                    workarea_y, screen_width, workarea_height
                );
                return;
            }
        }
        println!(
            "[TaskBar] Failed to query screen size for workarea update (fallback_width={}, bar_height={})",
            fallback_width, bar_height
        );
    }

    fn start_background_tasks(&mut self) {
        // CPU/Memory simulation
        let cpu = self.cpu_usage.clone();
        let mem = self.memory_usage.clone();
        let open_menu_index = self.open_menu_index.clone();
        let popup_surface_id = self.popup_surface_id.clone();
        let screen_width_popup = self.screen_width.clone();
        let menu_tree = self.menu_tree.clone();
        let active_window_id = self.active_window_id.clone();
        let open_menu_index_popup = open_menu_index.clone();
        let popup_surface_id_popup = popup_surface_id.clone();
        let menu_tree_popup = menu_tree.clone();
        let active_window_id_popup = active_window_id.clone();

        std::thread::spawn(move || {
            loop {
                cpu.update(|c| *c = (*c + 7) % 85 + 10);
                mem.update(|m| *m = (*m + 3) % 70 + 25);
                std::thread::sleep(Duration::from_secs(1));
            }
        });

        // Menu popup handling thread (still needed for interactive menu popup)
        std::thread::spawn(move || {
            let (mut conn, popup_screen_width, _, mut scale_milli) =
                match connect_sws_with_screen_size_retry() {
                    Ok((conn, width, height, scale_milli)) => (conn, width, height, scale_milli),
                    Err(()) => {
                        println!("[TaskBar] Failed to connect to SWS for menu popup after retries");
                        return;
                    }
                };
            graphics::set_current_scale_milli(scale_milli);
            screen_width_popup.set(popup_screen_width as f32);

            let mut popup_surface_id: Option<u32> = None;
            let mut popup_renderer: Option<PopupMenuRenderer> = None;
            let mut last_open_index: Option<usize> = None;

            let mut pointer_x = 0i32;
            let mut pointer_y = 0i32;
            let mut pending_move = false;
            let mut needs_render = false;

            loop {
                let open_index = open_menu_index_popup.get();
                let menu_tree_value = menu_tree_popup.get();

                if open_index != last_open_index {
                    if let Some(surface_id) = popup_surface_id.take() {
                        let _ = conn.destroy_surface(surface_id);
                    }
                    popup_surface_id_popup.set(None);
                    popup_renderer = None;
                    last_open_index = open_index;
                }

                if let Some(index) = open_index
                    && let Some(item) = menu_tree_value.items.get(index)
                {
                    if !item.children.is_empty() {
                        if popup_renderer.is_none() {
                            let (items, _height) = build_menu_items(
                                &item.children,
                                active_window_id_popup.get(),
                                open_menu_index_popup.clone(),
                            );
                            let item_height = 28.0;
                            let menu_width = 220.0;
                            let renderer =
                                PopupMenuRenderer::new(items, item_height, menu_width, scale_milli);
                            let size = renderer.size();
                            let width = size.width as u32;
                            let height = size.height as u32;
                            let physical_width = scale_u32(width, scale_milli);
                            let physical_height = scale_u32(height, scale_milli);
                            popup_renderer = Some(renderer);
                            needs_render = true;

                            let bar_height = TASKBAR_HEIGHT as i32;
                            let screen_width = screen_width_popup.get().max(1.0);
                            let popup_x = menu_bar_popup_x(&menu_tree_value.items, index)
                                .min((screen_width - width as f32).max(0.0));
                            let _surface_id = match popup_surface_id {
                                Some(id) => id,
                                None => {
                                    match conn.create_surface_with_type_and_policies_at(
                                        "org.scarlet-os.popup.menu",
                                        "Menu",
                                        "",
                                        physical_width,
                                        physical_height,
                                        window_types::ALWAYS_ON_TOP,
                                        false,
                                        true,
                                        false,
                                        scale_i32(popup_x as i32, scale_milli),
                                        scale_i32(bar_height, scale_milli),
                                    ) {
                                        Ok(id) => {
                                            popup_surface_id = Some(id);
                                            popup_surface_id_popup.set(Some(id));
                                            id
                                        }
                                        Err(e) => {
                                            println!(
                                                "[TaskBar] Failed to create menu popup: {:?}",
                                                e
                                            );
                                            popup_renderer = None;
                                            last_open_index = None;
                                            std::thread::sleep(Duration::from_millis(16));
                                            continue;
                                        }
                                    }
                                }
                            };
                        }
                    } else {
                        if let Some(surface_id) = popup_surface_id.take() {
                            let _ = conn.destroy_surface(surface_id);
                        }
                        popup_surface_id_popup.set(None);
                        popup_renderer = None;
                        last_open_index = None;
                        open_menu_index_popup.set(None);
                    }
                }

                let _ = conn.dispatch();
                while let Some(ev) = conn.poll_event() {
                    match ev {
                        sws::event::Event::FocusChanged {
                            window_id,
                            app_id,
                            app_name,
                            ..
                        } => {
                            if popup_surface_id_popup.get() == Some(window_id) {
                                continue;
                            }
                            if app_id == "org.scarlet-os.desktop.taskbar"
                                || app_name == "TaskBar"
                                || app_name == "Menu"
                            {
                                continue;
                            }
                            open_menu_index_popup.set(None);
                        }
                        sws::event::Event::Input(input) => {
                            if Some(input.surface_id) != popup_surface_id {
                                continue;
                            }
                            match (input.type_, input.code) {
                                (sws::event::event_type::EV_ABS, sws::event::abs_code::ABS_X) => {
                                    pointer_x = unscale_i32(input.value, scale_milli);
                                    pending_move = true;
                                }
                                (sws::event::event_type::EV_ABS, sws::event::abs_code::ABS_Y) => {
                                    pointer_y = unscale_i32(input.value, scale_milli);
                                    pending_move = true;
                                }
                                (
                                    sws::event::event_type::EV_KEY,
                                    sws::event::key_code::BTN_LEFT,
                                ) => {
                                    if input.value == 1 {
                                        // pressed
                                    } else if let Some(renderer) = popup_renderer.as_ref() {
                                        renderer.handle_click(pointer_x as f32, pointer_y as f32);
                                    }
                                }
                                (sws::event::event_type::EV_SYN, _) => {
                                    if pending_move {
                                        if let Some(renderer) = popup_renderer.as_mut()
                                            && renderer
                                                .handle_move(pointer_x as f32, pointer_y as f32)
                                        {
                                            needs_render = true;
                                        }
                                        pending_move = false;
                                    }
                                }
                                _ => {}
                            }
                        }
                        sws::event::Event::ScreenSizeChanged { width, .. } => {
                            screen_width_popup.set(unscale_u32(width, scale_milli) as f32);
                            open_menu_index_popup.set(None);
                            if let Some(surface_id) = popup_surface_id.take() {
                                let _ = conn.destroy_surface(surface_id);
                            }
                            popup_surface_id_popup.set(None);
                            popup_renderer = None;
                            last_open_index = None;
                        }
                        sws::event::Event::OutputScaleChanged {
                            scale_milli: next_scale_milli,
                        } => {
                            scale_milli = next_scale_milli.max(1);
                            graphics::set_current_scale_milli(scale_milli);
                            open_menu_index_popup.set(None);
                            if let Some(surface_id) = popup_surface_id.take() {
                                let _ = conn.destroy_surface(surface_id);
                            }
                            popup_surface_id_popup.set(None);
                            popup_renderer = None;
                            last_open_index = None;
                        }
                        _ => {}
                    }
                }

                if needs_render {
                    if let (Some(renderer), Some(surface_id)) =
                        (popup_renderer.as_mut(), popup_surface_id)
                    {
                        renderer.render();
                        if let Some(buffer) = renderer.buffer()
                            && let Some(surface) = conn.surface_mut(surface_id)
                        {
                            let src = buffer.as_slice();
                            let src_bytes = unsafe {
                                core::slice::from_raw_parts(
                                    src.as_ptr() as *const u8,
                                    src.len() * 4,
                                )
                            };
                            surface.with_buffer(|dst, w, h| {
                                let len = (w as usize).saturating_mul(h as usize).saturating_mul(4);
                                let copy_len = len.min(dst.len()).min(src_bytes.len());
                                dst[..copy_len].copy_from_slice(&src_bytes[..copy_len]);
                            });
                            let _ = conn.commit(surface_id);
                        }
                    }
                    needs_render = false;
                }

                std::thread::sleep(Duration::from_millis(16));
            }
        });

        // Wall clock: seconds-of-day (UTC), refreshed once per second.
        let clock = self.clock.clone();

        std::thread::spawn(move || {
            loop {
                let secs_of_day = time::system_time_ns()
                    .map(|ns| {
                        let offset = time::local_utc_offset_seconds().unwrap_or(0);
                        let local = (ns / 1_000_000_000) as i64 + offset;
                        (((local % 86_400) + 86_400) % 86_400) as u32
                    })
                    .unwrap_or(0);
                clock.update(|c| *c = secs_of_day);
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    fn update_menu_for_app(&mut self, window_id: u32, app_name: &str, menu_titles: &str) {
        println!(
            "[TaskBar] update_menu_for_app: window_id={}, app_name={}, menu_titles={}",
            window_id, app_name, menu_titles
        );

        if self.popup_surface_id.get() == Some(window_id) {
            println!("[TaskBar] Skipping popup surface {}", window_id);
            return;
        }

        if app_name == "TaskBar" || app_name == "Menu" {
            println!("[TaskBar] Skipping menu update for {}", app_name);
            return;
        }

        if app_name.is_empty() {
            println!("[TaskBar] No active application, showing default menu");
            self.active_window_id.set(0);
            self.open_menu_index.set(None);
            let tree = MenuTree {
                items: vec![TaskMenuItem {
                    id: String::from("system_scarlet"),
                    title: String::from("Scarlet"),
                    enabled: true,
                    shortcut: None,
                    children: default_system_menu_entries(),
                }],
            };
            self.menu_bar.set(menu_bar_from_tree(&tree));
            self.menu_tree.set(tree);
            return;
        }

        let tree = build_menu_tree(app_name, menu_titles);
        println!("[TaskBar] Built menu tree with {} items", tree.items.len());
        self.menu_bar.set(menu_bar_from_tree(&tree));
        self.menu_tree.set(tree);
        self.active_window_id.set(window_id);
        self.open_menu_index.set(None);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("[TaskBar] Starting ScarletUI TaskBar");

    let bar_height: u32 = TASKBAR_HEIGHT;

    // Get screen size from SWS before creating the app
    let screen_width = match connect_sws_with_screen_size_retry() {
        Ok((mut conn, width, height, scale_milli)) => {
            let physical_width = scale_u32(width, scale_milli);
            let physical_height = scale_u32(height, scale_milli);
            let workarea_y = scale_i32(bar_height as i32, scale_milli);
            let workarea_height =
                physical_height.saturating_sub(scale_u32(bar_height, scale_milli));
            let _ = conn.set_workarea(0, workarea_y, physical_width, workarea_height);
            println!(
                "[TaskBar] Workarea: x=0, y={}, width={}, height={}",
                workarea_y, physical_width, workarea_height
            );

            width as f32
        }
        Err(()) => {
            println!(
                "[TaskBar] Failed to connect to SWS after retries, using default screen width 1920"
            );
            1920.0
        }
    };

    let mut app = TaskBarApp::new();

    // Update screen_width state with actual screen size
    app.screen_width.update(|w| *w = screen_width);

    match app.run() {
        Ok(_) => {
            println!("[TaskBar] Application exited successfully");
        }
        Err(e) => {
            println!("[TaskBar] Application error: {}", e);
        }
    }
}
