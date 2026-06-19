//! Menu model definitions for application menu bars.

use crate::os::Mutex;
use crate::views::{MenuBar, MenuItem};
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;

pub type MenuCallback = Arc<dyn Fn() + Send + Sync + 'static>;

#[derive(Clone)]
pub enum MenuEntry {
    Item(MenuItemModel),
    Separator,
}

#[derive(Clone)]
pub struct MenuItemModel {
    id: String,
    title: String,
    enabled: bool,
    shortcut: Option<String>,
    children: Vec<MenuEntry>,
    on_activate: Option<MenuCallback>,
}

impl MenuItemModel {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            enabled: true,
            shortcut: None,
            children: Vec::new(),
            on_activate: None,
        }
    }

    /// Create an app-menu item. The taskbar merges its children into the
    /// auto-generated app-name dropdown.
    pub fn app() -> Self {
        Self::new("__app__", "")
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn children(mut self, children: Vec<MenuEntry>) -> Self {
        self.children = children;
        self
    }

    pub fn on_activate(mut self, callback: MenuCallback) -> Self {
        self.on_activate = Some(callback);
        self
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn enabled_value(&self) -> bool {
        self.enabled
    }

    pub fn shortcut_value(&self) -> Option<&str> {
        self.shortcut.as_deref()
    }

    pub fn children_value(&self) -> &[MenuEntry] {
        &self.children
    }
}

#[derive(Clone)]
pub struct MenuBarModel {
    items: Vec<MenuItemModel>,
}

impl MenuBarModel {
    pub fn new(items: Vec<MenuItemModel>) -> Self {
        Self { items }
    }

    pub fn items(&self) -> &[MenuItemModel] {
        &self.items
    }

    pub fn menu_titles(&self) -> String {
        let mut out = String::new();
        for (idx, item) in self.items.iter().enumerate() {
            if idx > 0 {
                out.push('|');
            }
            out.push_str(item.title());
        }
        out
    }

    pub fn to_json(&self) -> String {
        let mut out = String::from("{\"items\":[");
        for (idx, item) in self.items.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            write_menu_item_json(&mut out, item);
        }
        out.push_str("]}");
        out
    }

    pub fn to_menu_bar_view(&self) -> MenuBar {
        let items = self
            .items
            .iter()
            .map(|item| MenuItem::new(item.title()))
            .collect();
        MenuBar::new(items)
    }
}

impl Default for MenuBarModel {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

fn write_menu_item_json(out: &mut String, item: &MenuItemModel) {
    out.push('{');
    out.push_str("\"id\":\"");
    push_json_string(out, item.id());
    out.push_str("\",\"title\":\"");
    push_json_string(out, item.title());
    out.push_str("\",\"enabled\":");
    out.push_str(if item.enabled_value() {
        "true"
    } else {
        "false"
    });
    out.push_str(",\"shortcut\":");
    if let Some(sc) = item.shortcut_value() {
        out.push('"');
        push_json_string(out, sc);
        out.push('"');
    } else {
        out.push_str("null");
    }
    out.push_str(",\"items\":[");
    for (idx, entry) in item.children_value().iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        match entry {
            MenuEntry::Separator => out.push_str("{\"separator\":true}"),
            MenuEntry::Item(child) => write_menu_item_json(out, child),
        }
    }
    out.push_str("]}");
}

fn push_json_string(out: &mut String, value: &str) {
    for b in value.bytes() {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x00..=0x1f => {
                let _ = alloc::fmt::write(out, format_args!("\\u{:04x}", b));
            }
            _ => out.push(b as char),
        }
    }
}

type MenuCallbackKey = (u32, String);

static MENU_CALLBACKS: Mutex<BTreeMap<MenuCallbackKey, MenuCallback>> = Mutex::new(BTreeMap::new());

pub fn register_menu_callbacks(window_id: u32, menu_bar: &MenuBarModel) {
    let mut registry = MENU_CALLBACKS.lock();
    registry.retain(|(id, _), _| *id != window_id);
    for item in menu_bar.items() {
        collect_callbacks(window_id, item, &mut registry);
    }
}

pub fn invoke_menu_callback(window_id: u32, item_id: &str) -> bool {
    let key = (window_id, item_id.to_string());
    let callback = MENU_CALLBACKS.lock().get(&key).cloned();
    if let Some(callback) = callback {
        callback();
        true
    } else {
        false
    }
}

pub fn unregister_menu_callbacks(window_id: u32) {
    MENU_CALLBACKS
        .lock()
        .retain(|(registered_window_id, _), _| *registered_window_id != window_id);
}

fn collect_callbacks(
    window_id: u32,
    item: &MenuItemModel,
    registry: &mut BTreeMap<MenuCallbackKey, MenuCallback>,
) {
    if let Some(callback) = &item.on_activate {
        registry.insert((window_id, item.id().to_string()), callback.clone());
    }
    for entry in item.children_value() {
        if let MenuEntry::Item(child) = entry {
            collect_callbacks(window_id, child, registry);
        }
    }
}
