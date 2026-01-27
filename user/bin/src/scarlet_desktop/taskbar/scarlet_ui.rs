//! Scarlet Desktop TaskBar (ScarletUI version)
//!
//! macOS-style menu bar implemented with ScarletUI

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_desktop_config;
extern crate scarlet_ui;
extern crate scarlet_ui_macros;
extern crate scarlet_std as std;

use alloc::collections::BTreeMap;
use alloc::vec;
use core::time::Duration;
use scarlet_ui::prelude::*;
use scarlet_ui::buffer::Buffer;
use scarlet_ui::color::Color;
use scarlet_ui::element::{ElementRenderObject, LayoutConstraints};
use scarlet_ui::graphics;
use scarlet_ui::geometry::Size;
use scarlet_ui::{hstack, StateId};
use scarlet_ui::{MenuBarModel, MenuItemModel};
use scarlet_ui::views::{MenuAction, MenuBar, MenuItem, MenuItemContent};
use scarlet_ui::views::menu::MenuRenderObject;
use scarlet_ui_macros::View;
use serde::Deserialize;
use serde_json_core::from_str;
use std::{format, println};
use std::string::{String, ToString};
use std::vec::Vec;
use sws_client as sws;
use sws_protocol::window_types;

/// TaskBar Application
#[derive(View, Clone)]
struct TaskBarApp {
    cpu_usage: State<u8>,
    memory_usage: State<u8>,
    uptime: State<u32>,
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
            uptime: State::new(StateId::new(2), 0),
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

#[derive(Clone)]
struct MenuTree {
    items: Vec<TaskMenuItem>,
}

impl Default for MenuTree {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
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
            id: String::from("system_about"),
            title: String::from("About Scarlet"),
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
            title: String::from("Quit Scarlet"),
            enabled: true,
            shortcut: None,
            children: Vec::new(),
        }),
    ]
}

const MENU_BAR_FONT_SIZE: f32 = 16.0;
const MENU_BAR_ITEM_PADDING: f32 = 8.0;
const MENU_BAR_ITEM_SPACING: f32 = 2.0;
const MENU_BAR_OUTER_PADDING: f32 = 8.0;
const MENU_BAR_MAX_APP_LABEL: usize = 18;

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

    if !app_name.is_empty() {
        items.push(TaskMenuItem {
            id: String::from("system_app"),
            title: menu_bar_label(app_name),
            enabled: true,
            shortcut: None,
            children: Vec::new(),
        });
    }

    let cleaned = sanitize_menu_json(menu_titles);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        println!(
            "[TaskBar] Empty menu_titles after sanitize: orig_len={}, cleaned_len={}",
            menu_titles.len(),
            cleaned.len()
        );
    } else if trimmed.starts_with('{') {
        let parsed = parse_menu_tree_json(trimmed);
        println!(
            "[TaskBar] Parsed menu JSON: items={}",
            parsed.len()
        );
        items.extend(parsed);
    } else {
        println!(
            "[TaskBar] Non-JSON menu_titles: orig_len={}, cleaned_len={}, first_byte={:?}",
            menu_titles.len(),
            cleaned.len(),
            trimmed.as_bytes().get(0).copied()
        );
        items.extend(trimmed.split('|').map(|s| TaskMenuItem {
            id: s.to_string(),
            title: s.to_string(),
            enabled: true,
            shortcut: None,
            children: Vec::new(),
        }));
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
    let mut out = String::new();
    for ch in input.chars() {
        if ch == '\0' {
            break;
        }
        if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' {
            continue;
        }
        out.push(ch);
    }
    out
}

fn build_menu_entry(entry: MenuEntryPayload) -> Option<TaskMenuEntry> {
    if entry.separator.unwrap_or(false) {
        return Some(TaskMenuEntry::Separator);
    }

    let resolved_id = entry.id.unwrap_or_default();
    let mut resolved_title = entry.title.unwrap_or_default();
    if resolved_id.is_empty() && resolved_title.is_empty() {
        return None;
    }
    let resolved_id = if resolved_id.is_empty() {
        resolved_title.clone()
    } else {
        resolved_id
    };
    if resolved_title.is_empty() {
        resolved_title = resolved_id.clone();
    }
    let children = entry
        .items
        .unwrap_or_default()
        .into_iter()
        .filter_map(build_menu_entry)
        .collect();
    Some(TaskMenuEntry::Item(TaskMenuItem {
        id: resolved_id,
        title: resolved_title,
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
    println!("[TaskBar] build_menu_bar_view: {} items, active_window_id={}", items.len(), active_window_id);
    let entries = items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let title = item.title.to_string();
            let item_id = item.id.to_string();
            let has_children = !item.children.is_empty();
            let open_state_hover = open_menu_index.clone();
            let open_state_click = open_menu_index.clone();
            let window_id = active_window_id;
            let is_open = open_menu_index.get() == Some(idx);
            MenuItem::new(title)
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
                    if window_id == 0 || item_id.starts_with("system_") {
                        return;
                    }
                    if let Ok(mut conn) = sws::Connection::connect("/tmp/sws.sock") {
                        let _ = conn.activate_menu_item(window_id, &item_id);
                    }
                }
            })
        })
        .collect();
    MenuBar::new(entries).spacing(MENU_BAR_ITEM_SPACING)
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
}

impl PopupMenuRenderer {
    fn new(items: Vec<MenuItemContent>, item_height: f32, width: f32) -> Self {
        let mut render_object = MenuRenderObject::new(items, item_height, width);
        let constraints = LayoutConstraints {
            min_width: width,
            max_width: width,
            min_height: 0.0,
            max_height: f32::INFINITY,
        };
        let size = render_object.layout(constraints);
        render_object.render();
        Self { render_object, size }
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

    fn body(&self) -> impl View {
        let cpu = self.cpu_usage.get();
        let mem = self.memory_usage.get();
        let uptime = self.uptime.get();
        let screen_width = self.screen_width.get();
        let _menu_bar = self.menu_bar.get();
        let menu_tree = self.menu_tree.get();
        println!("[TaskBar] body() called: menu_tree has {} items", menu_tree.items.len());
        let active_window_id = self.active_window_id.get();

        let mins = (uptime / 60) % 60;
        let secs = uptime % 60;

        let bar_height = 40.0;
        let window_height = bar_height;

        Window::new("TaskBar",
            hstack! {
                build_menu_bar_view(&menu_tree.items, active_window_id, self.open_menu_index.clone()),
                Spacer::new(),
                Text::new(&format!("Mem {}%", mem))
                    .font_size(12.0)
                    .color(Color::rgb(0.280, 0.280, 0.310)),
                Text::new("•")
                    .font_size(12.0)
                    .color(Color::rgb(0.600, 0.600, 0.630)),
                Text::new(&format!("CPU {}%", cpu))
                    .font_size(12.0)
                    .color(Color::rgb(0.280, 0.280, 0.310)),
                Text::new("•")
                    .font_size(12.0)
                    .color(Color::rgb(0.600, 0.600, 0.630)),
                Text::new(&format!("Up {:02}:{:02}", mins, secs))
                    .font_size(12.0)
                    .color(Color::rgb(0.280, 0.280, 0.310)),
            }
            .spacing(10.0)
            .alignment(Alignment::Center)
            .padding(MENU_BAR_OUTER_PADDING)
        )
        .app_id("org.scarlet-os.desktop.taskbar")
        .decorated(false)
        .background_color(Some(Color::rgb(0.940, 0.940, 0.960)))
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
    fn start_background_tasks(&mut self) {
        // CPU/Memory simulation
        let cpu = self.cpu_usage.clone();
        let mem = self.memory_usage.clone();
        let open_menu_index = self.open_menu_index.clone();
        let popup_surface_id = self.popup_surface_id.clone();
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
            let mut conn = match sws::Connection::connect("/tmp/sws.sock") {
                Ok(conn) => conn,
                Err(e) => {
                    println!("[TaskBar] Failed to connect to SWS for menu popup: {:?}", e);
                    return;
                }
            };

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

                if let Some(index) = open_index {
                    if let Some(item) = menu_tree_value.items.get(index) {
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
                                    PopupMenuRenderer::new(items, item_height, menu_width);
                                let size = renderer.size();
                                let width = size.width as u32;
                                let height = size.height as u32;
                                popup_renderer = Some(renderer);
                                needs_render = true;

                                let bar_height = 40;
                                let popup_x = menu_bar_popup_x(&menu_tree_value.items, index);
                                let _surface_id = match popup_surface_id {
                                    Some(id) => id,
                                    None => {
                                        match conn.create_surface_with_type_and_policies_at(
                                            "org.scarlet-os.popup.menu",
                                            "Menu",
                                            "",
                                            width,
                                            height,
                                            window_types::ALWAYS_ON_TOP,
                                            false,
                                            true,
                                            false,
                                            popup_x as i32,
                                            bar_height as i32,
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
                        }
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
                                    pointer_x = input.value;
                                    pending_move = true;
                                }
                                (sws::event::event_type::EV_ABS, sws::event::abs_code::ABS_Y) => {
                                    pointer_y = input.value;
                                    pending_move = true;
                                }
                                (sws::event::event_type::EV_KEY, sws::event::key_code::BTN_LEFT) => {
                                    if input.value == 1 {
                                        // pressed
                                    } else {
                                        if let Some(renderer) = popup_renderer.as_ref() {
                                            renderer.handle_click(pointer_x as f32, pointer_y as f32);
                                        }
                                    }
                                }
                                (sws::event::event_type::EV_SYN, _) => {
                                    if pending_move {
                                        if let Some(renderer) = popup_renderer.as_mut() {
                                            if renderer.handle_move(
                                                pointer_x as f32,
                                                pointer_y as f32,
                                            ) {
                                                needs_render = true;
                                            }
                                        }
                                        pending_move = false;
                                    }
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }

                if needs_render {
                    if let (Some(renderer), Some(surface_id)) =
                        (popup_renderer.as_mut(), popup_surface_id)
                    {
                        renderer.render();
                        if let Some(buffer) = renderer.buffer() {
                            if let Some(surface) = conn.surface_mut(surface_id) {
                                let src = buffer.as_slice();
                                let src_bytes = unsafe {
                                    core::slice::from_raw_parts(
                                        src.as_ptr() as *const u8,
                                        src.len() * 4,
                                    )
                                };
                                surface.with_buffer(|dst, w, h| {
                                    let len =
                                        (w as usize).saturating_mul(h as usize).saturating_mul(4);
                                    let copy_len = len.min(dst.len()).min(src_bytes.len());
                                    dst[..copy_len].copy_from_slice(&src_bytes[..copy_len]);
                                });
                                let _ = conn.commit(surface_id);
                            }
                        }
                    }
                    needs_render = false;
                }

                std::thread::sleep(Duration::from_millis(16));
            }
        });

        // Uptime counter
        let uptime = self.uptime.clone();

        std::thread::spawn(move || {
            loop {
                uptime.update(|u| *u += 1);
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

    let bar_height: u32 = 40;

    // Get screen size from SWS before creating the app
    let screen_width = match sws::Connection::connect("/tmp/sws.sock") {
        Ok(mut conn) => {
            let (width, height) = match conn.get_screen_size() {
                Ok((width, height)) => {
                    println!("[TaskBar] Screen size: {}x{}", width, height);
                    (width, height)
                }
                Err(e) => {
                    println!("[TaskBar] Failed to get screen size: {:?}, using default 1920x1080", e);
                    (1920, 1080)
                }
            };

            let workarea_y = bar_height as i32;
            let workarea_height = height.saturating_sub(bar_height);
            let _ = conn.set_workarea(0, workarea_y, width, workarea_height);
            println!(
                "[TaskBar] Workarea: x=0, y={}, width={}, height={}",
                workarea_y, width, workarea_height
            );

            width as f32
        }
        Err(e) => {
            println!("[TaskBar] Failed to connect to SWS: {:?}, using default screen width 1920", e);
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
