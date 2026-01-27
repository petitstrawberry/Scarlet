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

use alloc::vec;
use core::time::Duration;
use scarlet_ui::prelude::*;
use scarlet_ui::buffer::Buffer;
use scarlet_ui::color::Color;
use scarlet_ui::element::{ElementRenderObject, LayoutConstraints};
use scarlet_ui::geometry::Size;
use scarlet_ui::{hstack, StateId};
use scarlet_ui::{MenuBarModel, MenuItemModel};
use scarlet_ui::views::{MenuAction, MenuBar, MenuItem, MenuItemContent};
use scarlet_ui::views::menu::MenuRenderObject;
use scarlet_ui_macros::View;
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
            title: app_name.to_string(),
            enabled: true,
            shortcut: None,
            children: Vec::new(),
        });
    }

    let trimmed = menu_titles.trim();
    if !trimmed.is_empty() {
        if trimmed.starts_with('{') {
            items.extend(parse_menu_tree_json(trimmed));
        } else {
            items.extend(trimmed.split('|').map(|s| TaskMenuItem {
                id: s.to_string(),
                title: s.to_string(),
                enabled: true,
                shortcut: None,
                children: Vec::new(),
            }));
        }
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

enum JsonValue {
    Null,
    Bool(bool),
    String(String),
    Object(Vec<(String, JsonValue)>),
    Array(Vec<JsonValue>),
}

fn parse_menu_tree_json(input: &str) -> Vec<TaskMenuItem> {
    let bytes = input.as_bytes();
    let mut idx = 0usize;
    let Some(value) = parse_json_value(bytes, &mut idx) else {
        return Vec::new();
    };
    let JsonValue::Object(root) = value else {
        return Vec::new();
    };
    let Some(JsonValue::Array(items)) = object_get(&root, "items") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        if let Some(entry) = parse_menu_entry(&item) {
            if let TaskMenuEntry::Item(item) = entry {
                out.push(item);
            }
        }
    }
    out
}

fn parse_menu_entry(value: &JsonValue) -> Option<TaskMenuEntry> {
    let JsonValue::Object(fields) = value else {
        return None;
    };
    if let Some(JsonValue::Bool(true)) = object_get(fields, "separator") {
        return Some(TaskMenuEntry::Separator);
    }

    let id = match object_get(fields, "id") {
        Some(JsonValue::String(s)) => s.clone(),
        _ => String::new(),
    };
    let title = match object_get(fields, "title") {
        Some(JsonValue::String(s)) => s.clone(),
        _ => String::new(),
    };
    if id.is_empty() && title.is_empty() {
        return None;
    }

    let enabled = match object_get(fields, "enabled") {
        Some(JsonValue::Bool(v)) => *v,
        _ => true,
    };
    let shortcut = match object_get(fields, "shortcut") {
        Some(JsonValue::String(s)) => Some(s.clone()),
        _ => None,
    };

    let mut children = Vec::new();
    if let Some(JsonValue::Array(items)) = object_get(fields, "items") {
        for child in items {
            if let Some(entry) = parse_menu_entry(child) {
                children.push(entry);
            }
        }
    }

    let resolved_id = if id.is_empty() { title.clone() } else { id };
    let resolved_title = if title.is_empty() { resolved_id.clone() } else { title };

    Some(TaskMenuEntry::Item(TaskMenuItem {
        id: resolved_id,
        title: resolved_title,
        enabled,
        shortcut,
        children,
    }))
}

fn object_get<'a>(fields: &'a [(String, JsonValue)], key: &str) -> Option<&'a JsonValue> {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn parse_json_value(bytes: &[u8], idx: &mut usize) -> Option<JsonValue> {
    skip_ws(bytes, idx);
    if *idx >= bytes.len() {
        return None;
    }
    match bytes[*idx] {
        b'"' => {
            let (value, next) = parse_json_string(bytes, *idx + 1)?;
            *idx = next;
            Some(JsonValue::String(value))
        }
        b'{' => parse_json_object(bytes, idx),
        b'[' => parse_json_array(bytes, idx),
        b't' => {
            if bytes.get(*idx..*idx + 4)? == b"true" {
                *idx += 4;
                Some(JsonValue::Bool(true))
            } else {
                None
            }
        }
        b'f' => {
            if bytes.get(*idx..*idx + 5)? == b"false" {
                *idx += 5;
                Some(JsonValue::Bool(false))
            } else {
                None
            }
        }
        b'n' => {
            if bytes.get(*idx..*idx + 4)? == b"null" {
                *idx += 4;
                Some(JsonValue::Null)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn parse_json_object(bytes: &[u8], idx: &mut usize) -> Option<JsonValue> {
    let mut fields = Vec::new();
    *idx += 1;
    loop {
        skip_ws(bytes, idx);
        if *idx >= bytes.len() {
            return None;
        }
        if bytes[*idx] == b'}' {
            *idx += 1;
            break;
        }
        if bytes[*idx] != b'"' {
            return None;
        }
        let (key, next) = parse_json_string(bytes, *idx + 1)?;
        *idx = next;
        skip_ws(bytes, idx);
        if *idx >= bytes.len() || bytes[*idx] != b':' {
            return None;
        }
        *idx += 1;
        let value = parse_json_value(bytes, idx)?;
        fields.push((key, value));
        skip_ws(bytes, idx);
        if *idx >= bytes.len() {
            return None;
        }
        match bytes[*idx] {
            b',' => *idx += 1,
            b'}' => {
                *idx += 1;
                break;
            }
            _ => return None,
        }
    }
    Some(JsonValue::Object(fields))
}

fn parse_json_array(bytes: &[u8], idx: &mut usize) -> Option<JsonValue> {
    let mut values = Vec::new();
    *idx += 1;
    loop {
        skip_ws(bytes, idx);
        if *idx >= bytes.len() {
            return None;
        }
        if bytes[*idx] == b']' {
            *idx += 1;
            break;
        }
        let value = parse_json_value(bytes, idx)?;
        values.push(value);
        skip_ws(bytes, idx);
        if *idx >= bytes.len() {
            return None;
        }
        match bytes[*idx] {
            b',' => *idx += 1,
            b']' => {
                *idx += 1;
                break;
            }
            _ => return None,
        }
    }
    Some(JsonValue::Array(values))
}

fn skip_ws(bytes: &[u8], idx: &mut usize) {
    while *idx < bytes.len() && bytes[*idx].is_ascii_whitespace() {
        *idx += 1;
    }
}

fn parse_json_string(bytes: &[u8], mut idx: usize) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut escape = false;

    while idx < bytes.len() {
        let b = bytes[idx];
        if escape {
            match b {
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                _ => out.push(b as char),
            }
            escape = false;
            idx += 1;
            continue;
        }

        match b {
            b'\\' => {
                escape = true;
            }
            b'"' => {
                return Some((out, idx + 1));
            }
            _ => out.push(b as char),
        }
        idx += 1;
    }

    None
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
            let open_state = open_menu_index.clone();
            let window_id = active_window_id;
            MenuItem::new(title)
                .padding(4.0)
                .on_click(move || {
                if has_children {
                    if open_state.get() == Some(idx) {
                        open_state.set(None);
                    } else {
                        open_state.set(Some(idx));
                    }
                } else {
                    open_state.set(None);
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
    MenuBar::new(entries)
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
        let _ = (window_id, app_name, menu_titles);
    }

    fn on_active_app_changed(&mut self, window_id: u32, app_name: &str, menu_titles: &str) {
        println!(
            "[TaskBar] on_active_app_changed: window_id={}, app_name={}, menu_titles={}",
            window_id, app_name, menu_titles
        );
        self.update_menu_for_app(window_id, app_name, menu_titles);
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
            .padding(8.0)
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

                                let surface_id = match popup_surface_id {
                                    Some(id) => id,
                                    None => {
                                        match conn.create_surface_with_type_and_policies(
                                            "org.scarlet-os.popup.menu",
                                            "Menu",
                                            "",
                                            width,
                                            height,
                                            window_types::ALWAYS_ON_TOP,
                                            false,
                                            true,
                                            false,
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
                                let bar_height = 40;
                                let _ = conn.move_window(surface_id, 0, bar_height as i32);
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
