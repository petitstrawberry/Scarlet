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

mod control_center;
mod status;

use alloc::collections::BTreeMap;
use alloc::vec;
use core::sync::atomic::{AtomicU8, Ordering};
use core::time::Duration;
use sas_client::SasClient;
use sas_protocol::{
    MASTER_VOLUME_UNITY_Q16, OUTPUT_ENTRY_FLAG_COMPATIBLE, OUTPUT_ENTRY_FLAG_CURRENT,
    OUTPUT_PREFERENCE_NAME, OUTPUT_PREFERENCE_PATH, OutputRequest,
};
use sbus::{Argument as SbusArgument, Message as SbusMessage};
use sbus_client::Connection as SbusConnection;
use scarlet_desktop_config::{
    DESKTOP_FILE_MANAGER_BUS_NAME, DESKTOP_FILE_MANAGER_INTERFACE,
    DESKTOP_FILE_MANAGER_OBJECT_PATH, DESKTOP_FILE_MANAGER_SHOW_METHOD, DESKTOP_LAUNCHER_BUS_NAME,
    DESKTOP_LAUNCHER_INTERFACE, DESKTOP_LAUNCHER_OBJECT_PATH, DESKTOP_LAUNCHER_SHOW_METHOD,
    DESKTOP_SETTINGS_BUS_NAME, DESKTOP_SETTINGS_GET_STATUS_PREFERENCES_METHOD,
    DESKTOP_SETTINGS_INTERFACE, DESKTOP_SETTINGS_OBJECT_PATH, DESKTOP_SETTINGS_SERVICE_INTERFACE,
    DESKTOP_SETTINGS_SERVICE_OBJECT_PATH, DESKTOP_SETTINGS_SIGNAL_SENDER,
    DESKTOP_STATUS_PREFERENCES_CHANGED_SIGNAL, StatusItemId, StatusPreferences,
};
use scarlet_os::time;
use scarlet_ui::buffer::Buffer;
use scarlet_ui::color::ColorPalette;
use scarlet_ui::element::{ElementRenderObject, LayoutConstraints};
use scarlet_ui::geometry::Size;
use scarlet_ui::graphics;
use scarlet_ui::platform::WindowPlacement;
use scarlet_ui::prelude::*;
use scarlet_ui::views::menu::MenuRenderObject;
use scarlet_ui::views::{MenuAction, MenuBar, MenuItem, MenuItemContent};
use scarlet_ui::{
    Icon, IconSize, MenuBarModel, MenuItemModel, PlatformWindow, dismiss_window, open_window,
};
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

use control_center::{
    ArmedPowerAction, AudioOutputSnapshot, AudioSnapshot, ControlCenterAction,
    ControlCenterMetrics, ControlCenterPresentation, ControlCenterSettingsLink,
    ControlCenterSnapshot, DynamicViews, InputEnvironmentSnapshot, NetworkInterfaceSnapshot,
    NetworkInterfaceState, NetworkSnapshot, SystemSnapshot, boxed, build_control_center_view,
};
use status::{StatusPresentation, StatusProvider, StatusProviderSnapshot};

const SWS_CONNECT_RETRIES: usize = 100;
const SWS_RETRY_DELAY_MS: u64 = 50;
const WINDOW_LIST_TIMEOUT_MS: u64 = 250;
const WINDOW_LIST_REFRESH_TICKS: u32 = 60;
const OVERVIEW_MENU_INDEX: usize = usize::MAX;
const CONTROL_CENTER_SCENE_KEY: &str = "control-center";
const TASKBAR_SETTINGS_LISTENER_BUS_NAME: &str = "org.scarlet-os.desktop.taskbar.settings-listener";
const SBUS_METHOD_TIMEOUT_MS: u64 = 1_000;
const CONTROL_CENTER_MARGIN: i32 = 8;
const OVERVIEW_SYSTEM_ROWS: usize = 3;
const OVERVIEW_NAVIGATION_ROWS: usize = 2;
const OVERVIEW_SEPARATOR_HEIGHT: f32 = 1.0;
const OVERVIEW_VERTICAL_PADDING: f32 = 4.0;

#[derive(Clone, Debug, PartialEq, Eq)]
struct WindowSnapshot {
    window_id: u32,
    app_id: String,
    title: String,
    window_type: u32,
    visible: bool,
    focused: bool,
    minimized: bool,
}

impl From<sws::WindowListEntry> for WindowSnapshot {
    fn from(entry: sws::WindowListEntry) -> Self {
        Self {
            window_id: entry.window_id,
            app_id: entry.app_id,
            title: entry.title,
            window_type: entry.window_type,
            visible: entry.visible,
            focused: entry.focused,
            minimized: entry.minimized,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ShellWindow {
    window_id: u32,
    app_id: String,
    title: String,
    visible: bool,
    focused: bool,
    minimized: bool,
}

fn is_shell_app(app_id: &str) -> bool {
    matches!(
        app_id,
        "org.scarlet-os.desktop.taskbar"
            | "org.scarlet-os.desktop.desktop"
            | "org.scarlet-os.desktop.background"
            | "org.scarlet-os.desktop.launcher"
    )
}

fn window_sort_group(window: &ShellWindow) -> u8 {
    if window.focused {
        0
    } else if window.visible && !window.minimized {
        1
    } else {
        2
    }
}

fn build_window_model(entries: Vec<WindowSnapshot>) -> Vec<ShellWindow> {
    let mut windows: Vec<ShellWindow> = entries
        .into_iter()
        .filter(|entry| {
            entry.window_id != 0
                && entry.window_type == window_types::NORMAL
                && !is_shell_app(&entry.app_id)
        })
        .map(|entry| ShellWindow {
            window_id: entry.window_id,
            app_id: entry.app_id,
            title: entry.title,
            visible: entry.visible,
            focused: entry.focused,
            minimized: entry.minimized,
        })
        .collect();
    windows.sort_by(|left, right| {
        window_sort_group(left)
            .cmp(&window_sort_group(right))
            .then_with(|| left.title.cmp(&right.title))
            .then_with(|| left.app_id.cmp(&right.app_id))
            .then_with(|| left.window_id.cmp(&right.window_id))
    });
    windows
}

fn window_title(window: &ShellWindow) -> &str {
    if window.title.trim().is_empty() {
        if window.app_id.trim().is_empty() {
            "Application"
        } else {
            window.app_id.as_str()
        }
    } else {
        window.title.as_str()
    }
}

fn shortened_label(label: &str, max_chars: usize) -> String {
    if label.chars().count() <= max_chars {
        return label.to_string();
    }
    let mut shortened = String::new();
    for ch in label.chars().take(max_chars.saturating_sub(3)) {
        shortened.push(ch);
    }
    shortened.push_str("...");
    shortened
}

fn overview_window_status(window: &ShellWindow) -> String {
    if window.focused {
        String::from("Active")
    } else if window.minimized || !window.visible {
        String::from("Minimized")
    } else {
        String::from("Open")
    }
}

fn focus_shell_window(window_id: u32) {
    if window_id == 0 {
        return;
    }
    if let Ok(conn) = sws::Connection::connect("/tmp/sws.sock") {
        let _ = conn.focus_window_any(window_id);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct OverviewGeometry {
    row_height: f32,
}

impl OverviewGeometry {
    const fn for_layout(_layout: ShellLayout) -> Self {
        Self { row_height: 28.0 }
    }
}

fn overview_page_capacity(screen_height: u32, layout: ShellLayout) -> usize {
    let available_height = screen_height.saturating_sub(layout.taskbar_height() + 8) as f32;
    let row_height = OverviewGeometry::for_layout(layout).row_height;
    let row_space =
        (available_height - OVERVIEW_VERTICAL_PADDING - OVERVIEW_SEPARATOR_HEIGHT).max(0.0);
    let total_rows = (row_space / row_height) as usize;
    total_rows
        .saturating_sub(OVERVIEW_SYSTEM_ROWS + OVERVIEW_NAVIGATION_ROWS)
        .max(1)
}

fn overview_page_count(item_count: usize, capacity: usize) -> usize {
    if item_count == 0 {
        1
    } else {
        item_count.div_ceil(capacity.max(1))
    }
}

fn overview_page_bounds(
    item_count: usize,
    capacity: usize,
    requested_page: usize,
) -> (usize, usize) {
    let capacity = capacity.max(1);
    let page = requested_page.min(overview_page_count(item_count, capacity) - 1);
    let start = page.saturating_mul(capacity).min(item_count);
    (start, start.saturating_add(capacity).min(item_count))
}

fn is_taskbar_debug_enabled() -> bool {
    static LOG_CACHE: AtomicU8 = AtomicU8::new(u8::MAX);
    let cached = LOG_CACHE.load(Ordering::Relaxed);
    if cached != u8::MAX {
        return cached != 0;
    }
    let enabled = match std::env::var("SWS_LOG") {
        Some(value) => matches!(
            value.as_str(),
            "debug" | "DEBUG" | "3" | "trace" | "TRACE" | "4"
        ),
        None => false,
    };
    LOG_CACHE.store(enabled as u8, Ordering::Relaxed);
    enabled
}

macro_rules! taskbar_debug {
    ($($arg:tt)*) => {
        if is_taskbar_debug_enabled() {
            std::println!($($arg)*);
        }
    };
}

/// Geometry policy shared by the desktop shell surfaces.
///
/// The shell deliberately keeps this independent of individual views so a
/// future overview, workspace switcher, or quick-settings surface can use the
/// same logical and physical coordinate system as the taskbar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ShellLayout {
    tablet_mode: bool,
}

/// Physical workarea reserved below the shell's top bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PhysicalWorkarea {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl ShellLayout {
    const LAPTOP_TASKBAR_HEIGHT: u32 = 32;
    const TABLET_TASKBAR_HEIGHT: u32 = Self::LAPTOP_TASKBAR_HEIGHT;

    /// Build a layout from the compositor state, treating an unknown state as
    /// the laptop/desktop default.
    const fn from_tablet_mode(tablet_mode: Option<bool>) -> Self {
        Self {
            tablet_mode: matches!(tablet_mode, Some(true)),
        }
    }

    /// Return the taskbar height in logical pixels.
    const fn taskbar_height(self) -> u32 {
        if self.tablet_mode {
            Self::TABLET_TASKBAR_HEIGHT
        } else {
            Self::LAPTOP_TASKBAR_HEIGHT
        }
    }

    /// Return the taskbar window size for a logical output width.
    fn taskbar_window_size(self, screen_width: f32) -> Size {
        Size::new(screen_width, self.taskbar_height() as f32)
    }

    /// Return the logical Y coordinate immediately below the taskbar.
    const fn popup_y(self) -> i32 {
        self.taskbar_height() as i32
    }

    /// Return the physical popup anchor directly below the taskbar.
    fn physical_popup_y(self, scale_milli: u32) -> i32 {
        scale_u32(self.popup_y() as u32, scale_milli) as i32
    }

    /// Return a physical workarea after reserving the scaled taskbar height.
    fn physical_workarea(
        self,
        physical_width: u32,
        physical_height: u32,
        scale_milli: u32,
    ) -> PhysicalWorkarea {
        let physical_bar_height = scale_u32(self.taskbar_height(), scale_milli);
        PhysicalWorkarea {
            x: 0,
            y: physical_bar_height as i32,
            width: physical_width,
            height: physical_height.saturating_sub(physical_bar_height),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct StatusItemTokens {
    logical_height: f32,
    font_size: f32,
    horizontal_padding: f32,
    spacing: f32,
    bar_padding: f32,
}

impl StatusItemTokens {
    const fn for_layout(_layout: ShellLayout) -> Self {
        Self {
            logical_height: 24.0,
            font_size: 13.0,
            horizontal_padding: 3.0,
            spacing: 3.0,
            bar_padding: MENU_BAR_OUTER_PADDING,
        }
    }
}

fn build_passive_clock(label: impl Into<String>, tokens: StatusItemTokens) -> impl View + Clone {
    passive_clock_control(label, tokens).frame_height(tokens.logical_height)
}

fn status_text_control(label: impl Into<String>, tokens: StatusItemTokens) -> MenuItem {
    MenuItem::new(label)
        .font_size(tokens.font_size)
        .padding(tokens.horizontal_padding)
}

fn passive_clock_control(label: impl Into<String>, tokens: StatusItemTokens) -> MenuItem {
    status_text_control(label, tokens)
}

fn toggle_control_center(open: State<bool>) {
    if open.get() {
        dismiss_window(CONTROL_CENTER_SCENE_KEY);
        open.set(false);
    } else {
        open_window(CONTROL_CENTER_SCENE_KEY);
        open.set(true);
    }
}

fn status_item_label(
    snapshot: &StatusProviderSnapshot,
    presentation: StatusPresentation,
    id: StatusItemId,
) -> Option<String> {
    snapshot
        .visible_items(presentation)
        .into_iter()
        .find(|descriptor| descriptor.id == id)
        .map(|descriptor| descriptor.label)
}

fn volume_status_icon(volume_percent: Option<u8>, muted: Option<bool>) -> Icon {
    if volume_percent.is_none()
        || muted.is_none()
        || muted == Some(true)
        || volume_percent == Some(0)
    {
        return Icon::Volume3;
    }
    match volume_percent.unwrap_or(0) {
        1..=50 => Icon::Volume2,
        _ => Icon::Volume,
    }
}

/// Build one system-status cluster which opens Control Center as a unit.
///
/// The clock is intentionally not part of this view. Callers append it after
/// this cluster so it remains the invariant far-right shell item.
fn build_status_cluster(
    snapshot: StatusProviderSnapshot,
    presentation: StatusPresentation,
    tokens: StatusItemTokens,
    control_center_open: State<bool>,
) -> impl View + Clone {
    let mut items = Vec::new();
    if let Some(cpu_label) = status_item_label(&snapshot, presentation, StatusItemId::Cpu) {
        items.push(boxed(
            status_text_control(cpu_label, tokens).frame_height(tokens.logical_height),
        ));
    }
    if snapshot.preferences.is_visible(StatusItemId::Audio) {
        let volume_icon = volume_status_icon(snapshot.audio_volume_percent, snapshot.audio_muted);
        items.push(boxed(
            MenuItem::new("")
                .icon(volume_icon)
                .icon_size(IconSize::Small)
                .font_size(tokens.font_size)
                .padding(tokens.horizontal_padding)
                .on_click(move || toggle_control_center(control_center_open.clone()))
                .frame(tokens.logical_height, tokens.logical_height),
        ));
    }
    HStack::new(DynamicViews::new(items))
        .spacing(tokens.spacing)
        .alignment(Alignment::Center)
}

struct SwsScreenConnection {
    connection: sws::Connection,
    logical_width: u32,
    logical_height: u32,
    scale_milli: u32,
    layout: ShellLayout,
    input_environment: Option<sws::InputEnvironment>,
}

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

fn query_output_scale(conn: &sws::Connection) -> u32 {
    conn.get_output_scale().unwrap_or(1000).max(1)
}

fn connect_sws_with_screen_size_retry() -> core::result::Result<SwsScreenConnection, ()> {
    for attempt in 0..SWS_CONNECT_RETRIES {
        if let Ok(conn) = sws::Connection::connect("/tmp/sws.sock")
            && let Ok((physical_width, physical_height)) = conn.get_screen_size()
        {
            let scale_milli = query_output_scale(&conn);
            let width = unscale_u32(physical_width, scale_milli);
            let height = unscale_u32(physical_height, scale_milli);
            let input_environment = conn.get_input_environment().ok();
            let layout = ShellLayout::from_tablet_mode(
                input_environment.and_then(|environment| environment.tablet_mode()),
            );
            println!(
                "[TaskBar] Connected to SWS after {} attempt(s); screen={}x{} scale_milli={} taskbar_height={}",
                attempt + 1,
                width,
                height,
                scale_milli,
                layout.taskbar_height(),
            );
            return Ok(SwsScreenConnection {
                connection: conn,
                logical_width: width,
                logical_height: height,
                scale_milli,
                layout,
                input_environment,
            });
        }

        std::thread::sleep(Duration::from_millis(SWS_RETRY_DELAY_MS));
    }

    Err(())
}

/// Publish the portion of the physical output not occupied by the shell bar.
fn publish_workarea(
    conn: &sws::Connection,
    layout: ShellLayout,
    physical_width: u32,
    physical_height: u32,
    scale_milli: u32,
) {
    let workarea = layout.physical_workarea(physical_width, physical_height, scale_milli);
    let _ = conn.set_workarea(workarea.x, workarea.y, workarea.width, workarea.height);
    println!(
        "[TaskBar] Workarea: x={}, y={}, width={}, height={}",
        workarea.x, workarea.y, workarea.width, workarea.height
    );
}

/// Query SWS output geometry and publish workarea for the supplied layout.
fn publish_current_workarea(conn: &sws::Connection, layout: ShellLayout) {
    if let Ok((physical_width, physical_height)) = conn.get_screen_size() {
        publish_workarea(
            conn,
            layout,
            physical_width,
            physical_height,
            query_output_scale(conn),
        );
    }
}

/// Return whether a platform taskbar surface must change size.
///
/// Keeping this comparison pure prevents the sync hook from issuing a resize
/// on every application-runner tick after a layout transition has settled.
fn taskbar_resize_needed(current: Size, desired: Size) -> bool {
    current != desired
}

fn control_center_body_position(screen_width: f32, popup_y: i32, body_width: f32) -> (i32, i32) {
    let outsets = ElevationRole::Floating.paint_outsets();
    (
        (screen_width - body_width - CONTROL_CENTER_MARGIN as f32 - outsets.right).max(0.0) as i32,
        popup_y + CONTROL_CENTER_MARGIN + outsets.top as i32,
    )
}

/// Apply one compositor input-environment snapshot to reactive shell state.
///
/// A layout transition closes a popup before the popup worker sees its new
/// anchor, then republishes SWS workarea using the same connection that
/// received the event.
fn apply_input_environment(
    conn: &sws::Connection,
    environment: sws::InputEnvironment,
    shell_layout: &State<ShellLayout>,
    open_menu_index: &State<Option<usize>>,
) {
    let next_layout = ShellLayout::from_tablet_mode(environment.tablet_mode());
    if shell_layout.get() != next_layout {
        open_menu_index.set(None);
        shell_layout.set(next_layout);
        println!(
            "[TaskBar] Input environment generation {} selected taskbar_height={}",
            environment.generation,
            next_layout.taskbar_height()
        );
        publish_current_workarea(conn, next_layout);
    }
}

/// Keep an independent SWS connection subscribed to shell-environment changes.
///
/// The ScarletUI runner owns a different connection, so a listener transport
/// failure cannot stop the taskbar from rendering. Each reconnect re-queries
/// the authoritative input environment before accepting notifications.
fn listen_for_input_environment_changes(
    shell_layout: State<ShellLayout>,
    open_menu_index: State<Option<usize>>,
) {
    loop {
        let Ok(SwsScreenConnection {
            connection: conn,
            input_environment,
            ..
        }) = connect_sws_with_screen_size_retry()
        else {
            println!("[TaskBar] Input-environment listener reconnect failed; retrying");
            std::thread::sleep(Duration::from_millis(SWS_RETRY_DELAY_MS));
            continue;
        };

        if let Some(environment) = input_environment {
            apply_input_environment(&conn, environment, &shell_layout, &open_menu_index);
        }

        loop {
            if conn.dispatch().is_err() {
                println!("[TaskBar] Input-environment listener transport lost; reconnecting");
                break;
            }
            while let Some(event) = conn.poll_event() {
                match event {
                    sws::event::Event::InputEnvironmentChanged(environment) => {
                        apply_input_environment(
                            &conn,
                            environment,
                            &shell_layout,
                            &open_menu_index,
                        );
                    }
                    sws::event::Event::ScreenSizeChanged { .. }
                    | sws::event::Event::OutputScaleChanged { .. } => {
                        publish_current_workarea(&conn, shell_layout.get());
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    }
}

/// TaskBar Application
#[derive(View, Clone)]
struct TaskBarApp {
    clock: State<u32>,
    screen_width: State<f32>,
    shell_layout: State<ShellLayout>,
    menu_bar: State<MenuBarModel>,
    active_window_id: State<u32>,
    menu_tree: State<MenuTree>,
    open_menu_index: State<Option<usize>>,
    popup_surface_id: State<Option<u32>>,
    menu_titles_cache: State<BTreeMap<u32, String>>,
    status_snapshot: State<StatusProviderSnapshot>,
    windows: State<Vec<ShellWindow>>,
    overview_page: State<usize>,
    control_center_open: State<bool>,
    control_center_window_id: State<Option<u32>>,
    control_center_volume: State<f32>,
    control_center_action: State<Option<ControlCenterAction>>,
    control_center_armed_power: State<Option<ArmedPowerAction>>,
    control_center_size: State<Size>,
}

impl TaskBarApp {
    fn new(shell_layout: ShellLayout) -> Self {
        let root_menu = MenuTree {
            items: default_root_menu_items(),
        };
        let status_snapshot = StatusProviderSnapshot {
            preferences: scarlet_desktop_config::load_desktop_config().status,
            ..StatusProviderSnapshot::default()
        };
        let initial_volume = status_snapshot.audio_volume_percent.unwrap_or(0) as f32;
        let initial_control_center =
            ControlCenterMetrics::resolve(ControlCenterPresentation::LaptopPopover, 0);
        Self {
            clock: State::new(StateId::new(2), 0),
            screen_width: State::new(StateId::new(3), 1920.0),
            shell_layout: State::new(StateId::new(13), shell_layout),
            menu_bar: State::new(StateId::new(4), menu_bar_from_tree(&root_menu)),
            active_window_id: State::new(StateId::new(5), 0),
            menu_tree: State::new(StateId::new(6), root_menu),
            open_menu_index: State::new(StateId::new(7), None),
            popup_surface_id: State::new(StateId::new(8), None),
            menu_titles_cache: State::new(StateId::new(9), BTreeMap::new()),
            status_snapshot: State::new(StateId::new(10), status_snapshot),
            windows: State::new(StateId::new(14), Vec::new()),
            overview_page: State::new(StateId::new(15), 0),
            control_center_open: State::new(StateId::new(16), false),
            control_center_window_id: State::new(StateId::new(17), None),
            control_center_volume: State::new(StateId::new(18), initial_volume),
            control_center_action: State::new(StateId::new(19), None),
            control_center_armed_power: State::new(StateId::new(20), None),
            control_center_size: State::new(StateId::new(21), initial_control_center.body_size()),
        }
    }

    fn resolve_menu_titles(&mut self, window_id: u32, menu_titles: &str) -> (String, bool) {
        if menu_titles.is_empty() {
            return (
                self.menu_titles_cache
                    .get()
                    .get(&window_id)
                    .cloned()
                    .unwrap_or_default(),
                false,
            );
        }

        let owned = menu_titles.to_string();
        let changed = self
            .menu_titles_cache
            .get()
            .get(&window_id)
            .is_none_or(|cached| cached != &owned);
        if changed {
            self.menu_titles_cache.update(|cache| {
                cache.insert(window_id, owned.clone());
            });
        }
        (owned, changed)
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

fn default_root_menu_items() -> Vec<TaskMenuItem> {
    vec![TaskMenuItem {
        id: String::from("system_scarlet"),
        title: String::from("Scarlet"),
        enabled: true,
        shortcut: None,
        children: default_system_menu_entries(),
    }]
}

fn default_system_menu_entries() -> Vec<TaskMenuEntry> {
    vec![
        TaskMenuEntry::Item(TaskMenuItem {
            id: String::from("system_launcher"),
            title: String::from("Applications"),
            enabled: true,
            shortcut: Some(String::from("Super+Space")),
            children: Vec::new(),
        }),
        TaskMenuEntry::Item(TaskMenuItem {
            id: String::from("system_terminal"),
            title: String::from("Terminal"),
            enabled: true,
            shortcut: None,
            children: Vec::new(),
        }),
        TaskMenuEntry::Item(TaskMenuItem {
            id: String::from("system_files"),
            title: String::from("Files"),
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

fn status_preferences_from_arguments(args: &[SbusArgument]) -> Option<StatusPreferences> {
    let [
        SbusArgument::String(order),
        SbusArgument::String(visible),
        SbusArgument::String(clock_format),
    ] = args
    else {
        return None;
    };
    StatusPreferences::from_ipc_values(order, visible, clock_format).ok()
}

fn query_status_preferences() -> core::result::Result<StatusPreferences, ()> {
    let mut connection = SbusConnection::connect().map_err(|_| ())?;
    let args = connection
        .call_method_timeout(
            DESKTOP_SETTINGS_BUS_NAME,
            DESKTOP_SETTINGS_SERVICE_OBJECT_PATH,
            DESKTOP_SETTINGS_SERVICE_INTERFACE,
            DESKTOP_SETTINGS_GET_STATUS_PREFERENCES_METHOD,
            Vec::new(),
            SBUS_METHOD_TIMEOUT_MS,
        )
        .map_err(|_| ())?;
    status_preferences_from_arguments(&args).ok_or(())
}

fn listen_for_status_preferences(status_snapshot: State<StatusProviderSnapshot>) {
    loop {
        if let Ok(preferences) = query_status_preferences() {
            status_snapshot.update(|snapshot| snapshot.preferences = preferences);
        }

        let mut connection = match SbusConnection::connect() {
            Ok(connection) => connection,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(SWS_RETRY_DELAY_MS));
                continue;
            }
        };
        if connection
            .register_service(TASKBAR_SETTINGS_LISTENER_BUS_NAME)
            .is_err()
        {
            std::thread::sleep(Duration::from_millis(SWS_RETRY_DELAY_MS));
            continue;
        }

        loop {
            match connection.receive_message() {
                Ok(SbusMessage::Signal {
                    sender,
                    path,
                    interface,
                    signal,
                    ..
                }) if sender == DESKTOP_SETTINGS_SIGNAL_SENDER
                    && path == DESKTOP_SETTINGS_OBJECT_PATH
                    && interface == DESKTOP_SETTINGS_INTERFACE
                    && signal == DESKTOP_STATUS_PREFERENCES_CHANGED_SIGNAL =>
                {
                    if let Ok(preferences) = query_status_preferences() {
                        status_snapshot.update(|snapshot| snapshot.preferences = preferences);
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }
}

fn poll_status_provider(status_snapshot: State<StatusProviderSnapshot>) {
    let mut provider = StatusProvider::new();
    let mut audio_client = None;

    loop {
        if audio_client.is_none() {
            audio_client = SasClient::connect().ok();
        }
        let audio_state = match audio_client.as_mut() {
            Some(client) => match client.control_state() {
                Ok(state) => Some(state),
                Err(_) => {
                    audio_client = None;
                    None
                }
            },
            None => None,
        };
        let preferences = status_snapshot.get().preferences;
        let sampled = provider.snapshot(&preferences, std::task::cpu_usage(), audio_state);
        status_snapshot.update(|current| {
            current.cpu_percent = sampled.cpu_percent;
            current.audio_volume_percent = sampled.audio_volume_percent;
            current.audio_muted = sampled.audio_muted;
        });
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn fixed_string(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    core::str::from_utf8(&bytes[..end])
        .unwrap_or_default()
        .to_string()
}

fn collect_audio_snapshot(status: &StatusProviderSnapshot) -> AudioSnapshot {
    let mut snapshot = match (status.audio_volume_percent, status.audio_muted) {
        (Some(volume), Some(muted)) => AudioSnapshot::from_status(volume, muted),
        _ => AudioSnapshot::unavailable(),
    };
    let Ok(mut client) = SasClient::connect() else {
        return snapshot;
    };
    let Ok(outputs) = client.list_outputs() else {
        return snapshot;
    };

    for output in outputs {
        let path = fixed_string(&output.path);
        let name = fixed_string(&output.name);
        let description = fixed_string(&output.description);
        let (id, fallback_name) = if !path.is_empty() {
            (format!("path:{}", path), path)
        } else if !name.is_empty() {
            (format!("name:{}", name), name.clone())
        } else {
            continue;
        };
        let label = if !description.is_empty() {
            description
        } else if !name.is_empty() {
            name
        } else {
            fallback_name
        };
        let current = output.flags & OUTPUT_ENTRY_FLAG_CURRENT != 0;
        if current {
            snapshot.current_output_id = Some(id.clone());
        }
        snapshot.outputs.push(AudioOutputSnapshot {
            id,
            name: label,
            available: current || output.flags & OUTPUT_ENTRY_FLAG_COMPATIBLE != 0,
        });
    }
    snapshot
}

fn collect_network_snapshot() -> NetworkSnapshot {
    let Ok((_status, interfaces)) = std::network::list_interfaces() else {
        return NetworkSnapshot {
            available: false,
            interfaces: Vec::new(),
        };
    };
    NetworkSnapshot {
        available: true,
        interfaces: interfaces
            .into_iter()
            .map(|interface| {
                let name = fixed_string(&interface.name);
                let connected = interface.ip_set != 0;
                NetworkInterfaceSnapshot {
                    name: if name.is_empty() {
                        String::from("Network interface")
                    } else {
                        name
                    },
                    state: if connected {
                        NetworkInterfaceState::Connected
                    } else {
                        NetworkInterfaceState::Disconnected
                    },
                    detail: connected.then(|| {
                        format!(
                            "{}.{}.{}.{}",
                            interface.ip_address[0],
                            interface.ip_address[1],
                            interface.ip_address[2],
                            interface.ip_address[3]
                        )
                    }),
                }
            })
            .collect(),
    }
}

fn collect_input_environment_snapshot() -> InputEnvironmentSnapshot {
    let environment = sws::Connection::connect("/tmp/sws.sock")
        .ok()
        .and_then(|connection| connection.get_input_environment().ok());
    match environment {
        Some(environment) => InputEnvironmentSnapshot {
            available: true,
            tablet_mode: environment.tablet_mode(),
            touch_present: Some(environment.has_direct_touch()),
            keyboard_present: Some(environment.has_keyboard()),
            pointer_present: Some(environment.has_fine_pointer()),
        },
        None => InputEnvironmentSnapshot {
            available: false,
            tablet_mode: None,
            touch_present: None,
            keyboard_present: None,
            pointer_present: None,
        },
    }
}

fn collect_control_center_snapshot(status: StatusProviderSnapshot) -> ControlCenterSnapshot {
    ControlCenterSnapshot {
        audio: collect_audio_snapshot(&status),
        network: collect_network_snapshot(),
        system: SystemSnapshot {
            cpu_percent: status.cpu_percent,
            task_count: Some(std::task::info().len().min(u32::MAX as usize) as u32),
        },
        input_environment: collect_input_environment_snapshot(),
    }
}

fn update_shared_audio_status(
    status: &State<StatusProviderSnapshot>,
    control_center_volume: &State<f32>,
    state: sas_protocol::ControlState,
) {
    let volume = ((state.master_volume_q16 as u64 * 100 + (MASTER_VOLUME_UNITY_Q16 / 2) as u64)
        / MASTER_VOLUME_UNITY_Q16 as u64)
        .min(100) as u8;
    status.update(|snapshot| {
        snapshot.audio_volume_percent = Some(volume);
        snapshot.audio_muted = Some(state.flags & sas_protocol::CONTROL_FLAG_MUTED != 0);
    });
    control_center_volume.set(volume as f32);
}

fn apply_control_center_action(
    action: ControlCenterAction,
    audio_client: &mut Option<SasClient>,
    status: &State<StatusProviderSnapshot>,
    control_center_volume: &State<f32>,
    control_center_open: &State<bool>,
) {
    match action {
        ControlCenterAction::SetVolume(percent) => {
            if audio_client.is_none() {
                *audio_client = SasClient::connect().ok();
            }
            let result = audio_client.as_mut().and_then(|client| {
                let q16 =
                    ((percent.min(100) as u64 * MASTER_VOLUME_UNITY_Q16 as u64 + 50) / 100) as u32;
                client.set_master_volume_q16(q16).ok()
            });
            if let Some(state) = result {
                update_shared_audio_status(status, control_center_volume, state);
            } else {
                *audio_client = None;
            }
        }
        ControlCenterAction::ToggleMute => {
            if audio_client.is_none() {
                *audio_client = SasClient::connect().ok();
            }
            let muted = status.get().audio_muted.unwrap_or(false);
            let result = audio_client
                .as_mut()
                .and_then(|client| client.set_master_muted(!muted).ok());
            if let Some(state) = result {
                update_shared_audio_status(status, control_center_volume, state);
            } else {
                *audio_client = None;
            }
        }
        ControlCenterAction::SelectOutput(id) => {
            let request = id
                .strip_prefix("path:")
                .and_then(|value| OutputRequest::new(OUTPUT_PREFERENCE_PATH, value))
                .or_else(|| {
                    id.strip_prefix("name:")
                        .and_then(|value| OutputRequest::new(OUTPUT_PREFERENCE_NAME, value))
                });
            if audio_client.is_none() {
                *audio_client = SasClient::connect().ok();
            }
            let result = request.and_then(|request| {
                audio_client
                    .as_mut()
                    .and_then(|client| client.set_output(request).ok())
            });
            if let Some(state) = result {
                update_shared_audio_status(status, control_center_volume, state);
            } else {
                *audio_client = None;
            }
        }
        ControlCenterAction::OpenSettings(
            ControlCenterSettingsLink::Network | ControlCenterSettingsLink::AllSettings,
        ) => {
            launch_app(b"org.scarlet-os.desktop.settings");
            dismiss_window(CONTROL_CENTER_SCENE_KEY);
            control_center_open.set(false);
        }
        ControlCenterAction::ConfirmPowerOff => {
            std::task::shutdown(std::task::ShutdownType::PowerOff);
        }
        ControlCenterAction::ConfirmReboot => {
            std::task::shutdown(std::task::ShutdownType::Reboot);
        }
        ControlCenterAction::ArmPowerOff | ControlCenterAction::ArmReboot => {}
    }
}

fn refresh_window_model(
    conn: &sws::Connection,
    windows: &State<Vec<ShellWindow>>,
) -> core::result::Result<(), ()> {
    let entries = conn
        .get_window_list_timeout(WINDOW_LIST_TIMEOUT_MS)
        .map_err(|_| ())?;
    let model = build_window_model(entries.into_iter().map(WindowSnapshot::from).collect());
    if windows.get() != model {
        windows.set(model);
    }
    Ok(())
}

fn listen_for_window_changes(windows: State<Vec<ShellWindow>>) {
    loop {
        let Ok(conn) = sws::Connection::connect("/tmp/sws.sock") else {
            std::thread::sleep(Duration::from_millis(SWS_RETRY_DELAY_MS));
            continue;
        };
        if refresh_window_model(&conn, &windows).is_err() {
            std::thread::sleep(Duration::from_millis(SWS_RETRY_DELAY_MS));
            continue;
        }

        let mut ticks_until_refresh = WINDOW_LIST_REFRESH_TICKS;
        loop {
            if conn.dispatch().is_err() {
                break;
            }
            let mut refresh_needed = false;
            while let Some(event) = conn.poll_event() {
                refresh_needed |= matches!(
                    event,
                    sws::event::Event::FocusChanged { .. }
                        | sws::event::Event::ActiveAppChanged { .. }
                        | sws::event::Event::SurfaceDestroyed { .. }
                        | sws::event::Event::SurfaceStateChanged { .. }
                );
            }
            if ticks_until_refresh == 0 {
                refresh_needed = true;
            }
            if refresh_needed {
                if refresh_window_model(&conn, &windows).is_err() {
                    break;
                }
                ticks_until_refresh = WINDOW_LIST_REFRESH_TICKS;
            } else {
                ticks_until_refresh = ticks_until_refresh.saturating_sub(1);
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    }
}

fn launch_app(app_id: &[u8]) {
    send_launch_command(0x01, app_id);
}

fn launch_new_app(app_id: &[u8]) {
    send_launch_command(0x05, app_id);
}

fn show_file_manager() {
    if request_file_manager_window() {
        return;
    }

    launch_app(scarlet_desktop_config::DESKTOP_FILES_APP_ID.as_bytes());
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(50));
        if request_file_manager_window() {
            return;
        }
    }
    println!("[TaskBar] File Manager service is not ready");
}

fn request_file_manager_window() -> bool {
    let Ok(mut connection) = SbusConnection::connect() else {
        return false;
    };
    connection
        .call_method_timeout(
            DESKTOP_FILE_MANAGER_BUS_NAME,
            DESKTOP_FILE_MANAGER_OBJECT_PATH,
            DESKTOP_FILE_MANAGER_INTERFACE,
            DESKTOP_FILE_MANAGER_SHOW_METHOD,
            Vec::new(),
            1_000,
        )
        .is_ok()
}

fn show_launcher() {
    for _ in 0..5 {
        if let Ok(mut connection) = SbusConnection::connect()
            && connection
                .call_method_timeout(
                    DESKTOP_LAUNCHER_BUS_NAME,
                    DESKTOP_LAUNCHER_OBJECT_PATH,
                    DESKTOP_LAUNCHER_INTERFACE,
                    DESKTOP_LAUNCHER_SHOW_METHOD,
                    Vec::new(),
                    1_000,
                )
                .is_ok()
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("[TaskBar] Resident launcher is not ready");
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

const MENU_BAR_FONT_SIZE: f32 = 13.0;
const MENU_BAR_ITEM_PADDING: f32 = 3.0;
const MENU_BAR_ITEM_SPACING: f32 = 2.0;
const MENU_BAR_OUTER_PADDING: f32 = 4.0;
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
    let mut items = default_root_menu_items();

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
        let mut app_items = Vec::new();
        for item in parsed {
            if item.id == "__app__" {
                app_children.extend(item.children);
            } else {
                app_items.push(item);
            }
        }

        // The current application's menu belongs immediately after the
        // desktop menu. App menus must not be appended behind File/Edit/etc.
        items.push(TaskMenuItem {
            id: String::from("system_app"),
            title: app_label,
            enabled: true,
            shortcut: None,
            children: app_children,
        });
        items.extend(app_items);
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
    _active_window_id: u32,
    open_menu_index: State<Option<usize>>,
) -> MenuBar {
    // println!(
    //     "[TaskBar] build_menu_bar_view: {} items, active_window_id={}",
    //     items.len(),
    //     active_window_id
    // );
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

fn build_overview_button(
    label: impl Into<String>,
    open_menu_index: State<Option<usize>>,
    overview_page: State<usize>,
) -> Button {
    Button::new(label)
        .font_size(15.0)
        .padding(10.0)
        .on_click(move || {
            if open_menu_index.get() == Some(OVERVIEW_MENU_INDEX) {
                open_menu_index.set(None);
            } else {
                overview_page.set(0);
                open_menu_index.set(Some(OVERVIEW_MENU_INDEX));
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
                    if item_id == "system_launcher" {
                        show_launcher();
                        return;
                    }
                    if item_id == "system_terminal" {
                        launch_new_app(b"org.scarlet-os.desktop.terminal");
                        return;
                    }
                    if item_id == "system_files" {
                        show_file_manager();
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
                    if let Ok(conn) = sws::Connection::connect("/tmp/sws.sock") {
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

fn overview_app_menu_indices(menu_tree: &MenuTree) -> Vec<usize> {
    menu_tree
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            ((item.id == "system_app" || !item.id.starts_with("system_"))
                && !item.children.is_empty())
            .then_some(index)
        })
        .collect()
}

fn build_overview_items(
    windows: &[ShellWindow],
    menu_tree: &MenuTree,
    requested_page: usize,
    page_capacity: usize,
    overview_page: State<usize>,
    open_menu_index: State<Option<usize>>,
) -> Vec<MenuItemContent> {
    let launcher_open = open_menu_index.clone();
    let files_open = open_menu_index.clone();
    let settings_open = open_menu_index.clone();
    let mut items = vec![
        MenuItemContent::new("Applications")
            .action(MenuAction::Submenu)
            .shortcut("Applications")
            .callback(move || {
                launcher_open.set(None);
                show_launcher();
            }),
        MenuItemContent::new("Files")
            .action(MenuAction::Submenu)
            .callback(move || {
                files_open.set(None);
                show_file_manager();
            }),
        MenuItemContent::new("Settings")
            .action(MenuAction::Submenu)
            .callback(move || {
                settings_open.set(None);
                launch_app(b"org.scarlet-os.desktop.settings");
            }),
        MenuItemContent::separator(),
    ];

    let app_menu_indices = overview_app_menu_indices(menu_tree);
    let dynamic_count = app_menu_indices.len().saturating_add(windows.len());
    let page_count = overview_page_count(dynamic_count, page_capacity);
    let page = requested_page.min(page_count - 1);
    let (start, end) = overview_page_bounds(dynamic_count, page_capacity, page);

    if dynamic_count == 0 {
        items.push(
            MenuItemContent::new("No open windows")
                .action(MenuAction::Submenu)
                .enabled(false),
        );
        return items;
    }

    for position in start..end {
        if let Some(menu_index) = app_menu_indices.get(position).copied() {
            let menu_open = open_menu_index.clone();
            let menu = &menu_tree.items[menu_index];
            let title = menu.title.clone();
            items.push(
                MenuItemContent::new(title)
                    .action(MenuAction::Submenu)
                    .shortcut("Application menu")
                    .enabled(menu.enabled)
                    .callback(move || menu_open.set(Some(menu_index))),
            );
            continue;
        }

        let window = &windows[position - app_menu_indices.len()];
        let detail = if window.app_id.is_empty() {
            overview_window_status(window)
        } else {
            format!("{} — {}", window.app_id, overview_window_status(window))
        };
        let window_id = window.window_id;
        let close = open_menu_index.clone();
        items.push(
            MenuItemContent::new(shortened_label(window_title(window), 38))
                .action(MenuAction::Submenu)
                .shortcut(detail)
                .callback(move || {
                    close.set(None);
                    focus_shell_window(window_id);
                }),
        );
    }

    if page_count > 1 {
        let previous_page = overview_page.clone();
        let next_page = overview_page;
        items.push(
            MenuItemContent::new("Previous")
                .action(MenuAction::Submenu)
                .shortcut(format!("Page {} of {}", page + 1, page_count))
                .enabled(page > 0)
                .callback(move || previous_page.set(page.saturating_sub(1))),
        );
        items.push(
            MenuItemContent::new("Next")
                .action(MenuAction::Submenu)
                .shortcut(format!("Page {} of {}", page + 1, page_count))
                .enabled(page + 1 < page_count)
                .callback(move || next_page.set((page + 1).min(page_count - 1))),
        );
    }
    items
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

enum ShellPopupRenderer {
    Menu(PopupMenuRenderer),
}

impl ShellPopupRenderer {
    fn size(&self) -> Size {
        match self {
            Self::Menu(renderer) => renderer.size(),
        }
    }

    fn handle_move(&mut self, x: i32, y: i32, pressed: bool) -> Option<ControlCenterAction> {
        match self {
            Self::Menu(renderer) => {
                let _ = renderer.handle_move(x as f32, y as f32);
                None
            }
        }
    }

    fn handle_press(&mut self, x: i32, y: i32) -> Option<ControlCenterAction> {
        match self {
            Self::Menu(_) => None,
        }
    }

    fn handle_release(&mut self, x: i32, y: i32) -> Option<ControlCenterAction> {
        match self {
            Self::Menu(renderer) => {
                renderer.handle_click(x as f32, y as f32);
                None
            }
        }
    }

    fn handle_cancel(&mut self) -> Option<ControlCenterAction> {
        match self {
            Self::Menu(_) => None,
        }
    }

    fn handle_exit(&mut self) {
        match self {
            Self::Menu(renderer) => {
                let _ = renderer.handle_move(-1.0, -1.0);
            }
        }
    }

    fn render(&mut self) {
        match self {
            Self::Menu(renderer) => renderer.render(),
        }
    }

    fn buffer(&self) -> Option<&Buffer> {
        match self {
            Self::Menu(renderer) => renderer.buffer(),
        }
    }
}

impl Application for TaskBarApp {
    fn on_focus_changed(&mut self, window_id: u32, app_name: &str, menu_titles: &str) {
        taskbar_debug!(
            "[TaskBar] on_focus_changed: window_id={}, app_name={}, menu_titles={}",
            window_id,
            app_name,
            menu_titles
        );
        if self.control_center_window_id.get() == Some(window_id) {
            return;
        }
        if self.control_center_open.get() && app_name != "TaskBar" && app_name != "Control Center" {
            dismiss_window(CONTROL_CENTER_SCENE_KEY);
            self.control_center_open.set(false);
            self.control_center_window_id.set(None);
            self.control_center_armed_power.set(None);
        }
        let (resolved_menu_titles, menu_changed) = self.resolve_menu_titles(window_id, menu_titles);
        self.update_menu_for_app(window_id, app_name, &resolved_menu_titles, menu_changed);
    }

    fn on_window_created(&mut self, ctx: &WindowContext, window: &mut dyn PlatformWindow) {
        if ctx.scene_key.as_str() == CONTROL_CENTER_SCENE_KEY {
            self.control_center_window_id
                .set(Some(ctx.platform_window_id as u32));
            let _ = window.set_opaque(false);
        }
    }

    fn on_window_close_requested(&mut self, ctx: &WindowContext) -> bool {
        if ctx.scene_key.as_str() == CONTROL_CENTER_SCENE_KEY {
            self.control_center_open.set(false);
            self.control_center_window_id.set(None);
            self.control_center_armed_power.set(None);
        }
        true
    }

    fn on_active_app_changed(&mut self, window_id: u32, app_name: &str, menu_titles: &str) {
        taskbar_debug!(
            "[TaskBar] on_active_app_changed: window_id={}, app_name={}, menu_titles={}",
            window_id,
            app_name,
            menu_titles
        );
        let (resolved_menu_titles, menu_changed) = self.resolve_menu_titles(window_id, menu_titles);
        self.update_menu_for_app(window_id, app_name, &resolved_menu_titles, menu_changed);
    }

    fn on_window_resize(&mut self, ctx: &WindowContext, width: u32, height: u32) {
        if ctx.scene_key.as_str() != "main" {
            return;
        }
        println!("[TaskBar] on_resize: width={}, height={}", width, height);
        self.screen_width.set(width as f32);
        self.open_menu_index.set(None);
        self.update_workarea_from_screen_query(width, self.shell_layout.get());
    }

    fn on_window_sync(&mut self, ctx: &WindowContext, window: &mut dyn PlatformWindow) {
        if ctx.scene_key.as_str() == CONTROL_CENTER_SCENE_KEY {
            let desired = self.control_center_size.get();
            if taskbar_resize_needed(window.managed_size(), desired) {
                let _ = window.resize_managed(desired.width as u32, desired.height as u32);
            }
            return;
        }
        if ctx.scene_key.as_str() != "main" {
            return;
        }
        let desired = self
            .shell_layout
            .get()
            .taskbar_window_size(self.screen_width.get());
        if taskbar_resize_needed(window.size(), desired)
            && let Err(error) = window.resize(desired.width.max(1.0) as u32, desired.height as u32)
        {
            println!("[TaskBar] Failed to resize shell surface: {}", error);
        }
    }

    fn on_screen_size_changed(&mut self, width: u32, height: u32) -> Option<Size> {
        println!("[TaskBar] on_screen_size_changed: {}x{}", width, height);
        self.screen_width.set(width as f32);
        self.open_menu_index.set(None);
        let layout = self.shell_layout.get();
        self.update_workarea(width, height, layout);
        Some(layout.taskbar_window_size(width as f32))
    }

    fn scenes(&self) -> impl Scene {
        let clock = self.clock.get();
        let screen_width = self.screen_width.get();
        let shell_layout = self.shell_layout.get();
        let _menu_bar = self.menu_bar.get();
        let menu_tree = self.menu_tree.get();
        // println!(
        //     "[TaskBar] scenes() called: menu_tree has {} items",
        //     menu_tree.items.len()
        // );
        let active_window_id = self.active_window_id.get();

        let hours = clock / 3600;
        let mins = (clock / 60) % 60;
        let status_snapshot = self.status_snapshot.get();
        let clock_label = status_snapshot.clock_label(hours as u8, mins as u8);
        // The desktop top bar intentionally uses a light material. Its status
        // labels and Tabler icons use ScarletUI's matching dark foreground.
        let taskbar_palette = ColorPalette::light();
        let status_tokens =
            StatusItemTokens::for_layout(ShellLayout::from_tablet_mode(Some(false)));
        if !self.control_center_open.get()
            && let Some(volume) = status_snapshot.audio_volume_percent
        {
            self.control_center_volume.set(volume as f32);
        }

        let control_center_snapshot = if self.control_center_open.get() {
            collect_control_center_snapshot(status_snapshot.clone())
        } else {
            ControlCenterSnapshot {
                audio: AudioSnapshot::unavailable(),
                network: NetworkSnapshot {
                    available: false,
                    interfaces: Vec::new(),
                },
                system: SystemSnapshot {
                    cpu_percent: status_snapshot.cpu_percent,
                    task_count: None,
                },
                input_environment: InputEnvironmentSnapshot {
                    available: false,
                    tablet_mode: None,
                    touch_present: None,
                    keyboard_present: None,
                    pointer_present: None,
                },
            }
        };
        let control_center_metrics = ControlCenterMetrics::resolve(
            ControlCenterPresentation::LaptopPopover,
            control_center_snapshot.audio.outputs.len(),
        );
        let control_center_size = control_center_metrics.body_size();
        if self.control_center_size.get() != control_center_size {
            self.control_center_size.set(control_center_size);
        }
        let (control_center_body_x, control_center_body_y) = control_center_body_position(
            screen_width,
            shell_layout.popup_y(),
            control_center_metrics.width as f32,
        );

        (
            WindowGroup::new(
                "main",
                Window::new(
                    "TaskBar",
                    hstack! {
                        build_menu_bar_view(
                            &menu_tree.items,
                            active_window_id,
                            self.open_menu_index.clone(),
                        ),
                        Spacer::new(),
                        build_status_cluster(
                            status_snapshot,
                            StatusPresentation::Compact,
                            status_tokens,
                            self.control_center_open.clone(),
                        ),
                        build_passive_clock(clock_label, status_tokens),
                    }
                    .spacing(status_tokens.spacing)
                    .alignment(Alignment::Center)
                    .padding(status_tokens.bar_padding),
                )
                .app_id("org.scarlet-os.desktop.taskbar")
                .decorated(false)
                .background_color(taskbar_palette.surface_variant())
                .window_type(scarlet_ui::views::window_type::TASKBAR)
                .active_on_focus(false)
                .resizable(false)
                .movable(false)
                .size(shell_layout.taskbar_window_size(screen_width)),
            ),
            Window::new(
                "Control Center",
                build_control_center_view(
                    ControlCenterPresentation::LaptopPopover,
                    control_center_snapshot,
                    self.control_center_volume.clone(),
                    self.control_center_action.clone(),
                    self.control_center_armed_power.clone(),
                ),
            )
            .scene_key(CONTROL_CENTER_SCENE_KEY)
            .open_at_launch(false)
            .app_id("org.scarlet-os.popup.control-center")
            .decorated(false)
            .background_color(scarlet_ui::color::Color::TRANSPARENT)
            .opaque(false)
            .corner_radius(ControlCenterMetrics::CORNER_RADIUS)
            .shadow_elevation(ElevationRole::Floating)
            .window_type(scarlet_ui::views::window_type::ALWAYS_ON_TOP)
            .focus_on_create(true)
            .active_on_focus(false)
            .resizable(false)
            .movable(false)
            .placement(WindowPlacement::At {
                x: control_center_body_x,
                y: control_center_body_y,
            })
            .size(control_center_size),
        )
    }

    fn exit_when_all_windows_closed(&self) -> bool {
        false
    }

    fn init(&mut self) {
        println!("[TaskBar] Initializing ScarletUI TaskBar");
        // Screen size will be obtained by sws_client in main()
        self.start_background_tasks();
    }
}

impl TaskBarApp {
    fn update_workarea(&self, screen_width: u32, screen_height: u32, layout: ShellLayout) {
        if let Ok(conn) = sws::Connection::connect("/tmp/sws.sock") {
            let scale_milli = query_output_scale(&conn);
            let physical_width = scale_u32(screen_width, scale_milli);
            let physical_height = scale_u32(screen_height, scale_milli);
            publish_workarea(&conn, layout, physical_width, physical_height, scale_milli);
        }
    }

    fn update_workarea_from_screen_query(&self, fallback_width: u32, layout: ShellLayout) {
        if let Ok(conn) = sws::Connection::connect("/tmp/sws.sock") {
            if let Ok((screen_width, screen_height)) = conn.get_screen_size() {
                let scale_milli = query_output_scale(&conn);
                publish_workarea(&conn, layout, screen_width, screen_height, scale_milli);
                return;
            }
        }
        println!(
            "[TaskBar] Failed to query screen size for workarea update (fallback_width={}, taskbar_height={})",
            fallback_width,
            layout.taskbar_height()
        );
    }

    fn start_background_tasks(&mut self) {
        let open_menu_index = self.open_menu_index.clone();
        let popup_surface_id = self.popup_surface_id.clone();
        let screen_width_popup = self.screen_width.clone();
        let shell_layout_popup = self.shell_layout.clone();
        let menu_tree = self.menu_tree.clone();
        let active_window_id = self.active_window_id.clone();
        let open_menu_index_popup = open_menu_index.clone();
        let popup_surface_id_popup = popup_surface_id.clone();
        let menu_tree_popup = menu_tree.clone();
        let active_window_id_popup = active_window_id.clone();
        let status_snapshot_provider = self.status_snapshot.clone();
        let status_snapshot_listener = self.status_snapshot.clone();
        let windows_listener = self.windows.clone();
        let windows_popup = self.windows.clone();
        let overview_page_popup = self.overview_page.clone();

        let shell_layout_listener = self.shell_layout.clone();
        let open_menu_index_listener = self.open_menu_index.clone();
        let control_center_action = self.control_center_action.clone();
        let control_center_status = self.status_snapshot.clone();
        let control_center_volume = self.control_center_volume.clone();
        let control_center_open = self.control_center_open.clone();

        std::thread::spawn(move || {
            listen_for_input_environment_changes(shell_layout_listener, open_menu_index_listener);
        });

        std::thread::spawn(move || {
            poll_status_provider(status_snapshot_provider);
        });

        std::thread::spawn(move || {
            listen_for_status_preferences(status_snapshot_listener);
        });

        std::thread::spawn(move || {
            listen_for_window_changes(windows_listener);
        });

        std::thread::spawn(move || {
            let mut audio_client: Option<SasClient> = None;
            loop {
                if let Some(action) = control_center_action.get() {
                    control_center_action.set(None);
                    apply_control_center_action(
                        action,
                        &mut audio_client,
                        &control_center_status,
                        &control_center_volume,
                        &control_center_open,
                    );
                }
                std::thread::sleep(Duration::from_millis(16));
            }
        });

        // Menu popup handling thread (still needed for interactive menu popup)
        std::thread::spawn(move || {
            let SwsScreenConnection {
                connection: conn,
                logical_width: popup_screen_width,
                logical_height: mut popup_screen_height,
                mut scale_milli,
                ..
            } = match connect_sws_with_screen_size_retry() {
                Ok(connection) => connection,
                Err(()) => {
                    println!("[TaskBar] Failed to connect to SWS for menu popup after retries");
                    return;
                }
            };
            graphics::set_current_scale_milli(scale_milli);
            screen_width_popup.set(popup_screen_width as f32);

            let mut popup_surface_id: Option<u32> = None;
            let mut popup_renderer: Option<ShellPopupRenderer> = None;
            let mut last_open_index: Option<usize> = None;
            let mut last_overview_windows: Vec<ShellWindow> = Vec::new();
            let mut last_overview_page = 0usize;

            let mut pointer_x = 0i32;
            let mut pointer_y = 0i32;
            let mut pointer_pressed = false;
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
                    pointer_pressed = false;
                    last_open_index = open_index;
                }

                let current_overview_windows = windows_popup.get();
                let current_overview_page = overview_page_popup.get();
                if open_index == Some(OVERVIEW_MENU_INDEX)
                    && popup_renderer.is_some()
                    && (current_overview_windows != last_overview_windows
                        || current_overview_page != last_overview_page)
                {
                    if let Some(surface_id) = popup_surface_id.take() {
                        let _ = conn.destroy_surface(surface_id);
                    }
                    popup_surface_id_popup.set(None);
                    popup_renderer = None;
                }

                if let Some(index) = open_index {
                    let overview_open = index == OVERVIEW_MENU_INDEX;
                    let menu_entries = menu_tree_value.items.get(index).map(|item| &item.children);
                    if overview_open || menu_entries.is_some_and(|entries| !entries.is_empty()) {
                        if popup_renderer.is_none() {
                            let layout = shell_layout_popup.get();
                            let geometry = OverviewGeometry::for_layout(layout);
                            let renderer = {
                                let items = if overview_open {
                                    last_overview_windows = current_overview_windows.clone();
                                    let page_capacity =
                                        overview_page_capacity(popup_screen_height, layout);
                                    let dynamic_count = overview_app_menu_indices(&menu_tree_value)
                                        .len()
                                        .saturating_add(current_overview_windows.len());
                                    let page_count =
                                        overview_page_count(dynamic_count, page_capacity);
                                    let page = current_overview_page.min(page_count - 1);
                                    if page != current_overview_page {
                                        overview_page_popup.set(page);
                                    }
                                    last_overview_page = page;
                                    build_overview_items(
                                        &current_overview_windows,
                                        &menu_tree_value,
                                        page,
                                        page_capacity,
                                        overview_page_popup.clone(),
                                        open_menu_index_popup.clone(),
                                    )
                                } else {
                                    build_menu_items(
                                        menu_entries.map_or(&[], Vec::as_slice),
                                        active_window_id_popup.get(),
                                        open_menu_index_popup.clone(),
                                    )
                                    .0
                                };
                                let menu_width = if overview_open {
                                    (screen_width_popup.get() - 16.0).clamp(220.0, 420.0)
                                } else {
                                    220.0
                                };
                                ShellPopupRenderer::Menu(PopupMenuRenderer::new(
                                    items,
                                    geometry.row_height,
                                    menu_width,
                                    scale_milli,
                                ))
                            };
                            let size = renderer.size();
                            let width = size.width as u32;
                            let height = size.height as u32;
                            let physical_width = scale_u32(width, scale_milli);
                            let physical_height = scale_u32(height, scale_milli);
                            popup_renderer = Some(renderer);
                            needs_render = true;

                            let screen_width = screen_width_popup.get().max(1.0);
                            let popup_x = if overview_open {
                                8.0
                            } else {
                                menu_bar_popup_x(&menu_tree_value.items, index)
                                    .min((screen_width - width as f32).max(0.0))
                            };
                            let popup_app_id = "org.scarlet-os.popup.menu";
                            let popup_title = "Menu";
                            let _surface_id = match popup_surface_id {
                                Some(id) => id,
                                None => {
                                    match conn.create_surface_with_type_and_policies_at(
                                        popup_app_id,
                                        popup_title,
                                        "",
                                        physical_width,
                                        physical_height,
                                        window_types::ALWAYS_ON_TOP,
                                        false,
                                        true,
                                        false,
                                        scale_i32(popup_x as i32, scale_milli),
                                        layout.physical_popup_y(scale_milli),
                                    ) {
                                        Ok(id) => {
                                            popup_surface_id = Some(id);
                                            popup_surface_id_popup.set(Some(id));
                                            // Creating a surface with
                                            // `focus_on_create` focuses it, but
                                            // older SWS versions did not also
                                            // raise it within its window-type
                                            // layer. Explicitly raise the
                                            // popup so it stays above the
                                            // application menu and content.
                                            let _ = conn.focus_window(id);
                                            id
                                        }
                                        Err(e) => {
                                            println!(
                                                "[TaskBar] Failed to create {} popup: {:?}",
                                                popup_title, e
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
                                        pointer_pressed = true;
                                        if let Some(renderer) = popup_renderer.as_mut() {
                                            let _ = renderer.handle_press(pointer_x, pointer_y);
                                            needs_render = true;
                                        }
                                    } else if let Some(renderer) = popup_renderer.as_mut() {
                                        let _ = if pointer_x < 0 || pointer_y < 0 {
                                            renderer.handle_cancel()
                                        } else {
                                            renderer.handle_release(pointer_x, pointer_y)
                                        };
                                        pointer_pressed = false;
                                        needs_render = true;
                                    }
                                }
                                (sws::event::event_type::EV_SYN, _) => {
                                    if pending_move {
                                        if let Some(renderer) = popup_renderer.as_mut() {
                                            if pointer_x < 0 || pointer_y < 0 {
                                                renderer.handle_exit();
                                            } else {
                                                let _ = renderer.handle_move(
                                                    pointer_x,
                                                    pointer_y,
                                                    pointer_pressed,
                                                );
                                            }
                                            needs_render = true;
                                        }
                                        pending_move = false;
                                    }
                                }
                                _ => {}
                            }
                        }
                        sws::event::Event::ScreenSizeChanged { width, height } => {
                            screen_width_popup.set(unscale_u32(width, scale_milli) as f32);
                            popup_screen_height = unscale_u32(height, scale_milli);
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
                            if let Ok((width, height)) = conn.get_screen_size() {
                                screen_width_popup.set(unscale_u32(width, scale_milli) as f32);
                                popup_screen_height = unscale_u32(height, scale_milli);
                            }
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
                        if let Some(buffer) = renderer.buffer() {
                            let src = buffer.as_slice();
                            let src_bytes = unsafe {
                                core::slice::from_raw_parts(
                                    src.as_ptr() as *const u8,
                                    src.len() * 4,
                                )
                            };
                            if conn
                                .with_surface_mut(surface_id, |surface| {
                                    surface.with_buffer(|dst, w, h| {
                                        let len = (w as usize)
                                            .saturating_mul(h as usize)
                                            .saturating_mul(4);
                                        let copy_len = len.min(dst.len()).min(src_bytes.len());
                                        dst[..copy_len].copy_from_slice(&src_bytes[..copy_len]);
                                    });
                                })
                                .is_some()
                            {
                                let _ = conn.commit(surface_id);
                            }
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

    fn update_menu_for_app(
        &mut self,
        window_id: u32,
        app_name: &str,
        menu_titles: &str,
        menu_changed: bool,
    ) {
        taskbar_debug!(
            "[TaskBar] update_menu_for_app: window_id={}, app_name={}, menu_titles={}",
            window_id,
            app_name,
            menu_titles
        );

        if self.popup_surface_id.get() == Some(window_id) {
            taskbar_debug!("[TaskBar] Skipping popup surface {}", window_id);
            return;
        }

        if app_name == "TaskBar" || app_name == "Menu" {
            taskbar_debug!("[TaskBar] Skipping menu update for {}", app_name);
            return;
        }

        if app_name.is_empty() {
            if self.active_window_id.get() == 0 {
                return;
            }
            taskbar_debug!("[TaskBar] No active application, showing default menu");
            self.active_window_id.set(0);
            self.open_menu_index.set(None);
            let tree = MenuTree {
                items: default_root_menu_items(),
            };
            self.menu_bar.set(menu_bar_from_tree(&tree));
            self.menu_tree.set(tree);
            return;
        }

        // A focus transition broadcasts both FOCUS_CHANGED and, when the
        // active application changes, ACTIVE_APP_CHANGED. The payloads are
        // intentionally equivalent for TaskBar, so rebuilding the full menu
        // tree twice only adds latency and an avoidable redraw.
        if self.active_window_id.get() == window_id && !menu_changed {
            return;
        }

        let tree = build_menu_tree(app_name, menu_titles);
        taskbar_debug!("[TaskBar] Built menu tree with {} items", tree.items.len());
        self.menu_bar.set(menu_bar_from_tree(&tree));
        self.menu_tree.set(tree);
        self.active_window_id.set(window_id);
        self.open_menu_index.set(None);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() {
    println!("[TaskBar] Starting ScarletUI TaskBar");

    // Get screen size from SWS before creating the app
    let (screen_width, shell_layout) = match connect_sws_with_screen_size_retry() {
        Ok(SwsScreenConnection {
            connection: conn,
            logical_width,
            logical_height,
            scale_milli,
            layout,
            ..
        }) => {
            publish_workarea(
                &conn,
                layout,
                scale_u32(logical_width, scale_milli),
                scale_u32(logical_height, scale_milli),
                scale_milli,
            );
            (logical_width as f32, layout)
        }
        Err(()) => {
            println!(
                "[TaskBar] Failed to connect to SWS after retries, using default screen width 1920"
            );
            (1920.0, ShellLayout::from_tablet_mode(None))
        }
    };

    let mut app = TaskBarApp::new(shell_layout);

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

#[cfg(test)]
mod tests {
    use super::{
        MenuTree, OVERVIEW_NAVIGATION_ROWS, OVERVIEW_SEPARATOR_HEIGHT, OVERVIEW_SYSTEM_ROWS,
        OVERVIEW_VERTICAL_PADDING, OverviewGeometry, ShellLayout, StatusItemId, StatusItemTokens,
        StatusPresentation, StatusProviderSnapshot, TaskMenuEntry, TaskMenuItem, WindowSnapshot,
        build_window_model, control_center_body_position, overview_app_menu_indices,
        overview_page_bounds, overview_page_capacity, overview_page_count, overview_window_status,
        passive_clock_control, scale_u32, status_item_label, taskbar_resize_needed,
        volume_status_icon,
    };
    use alloc::string::String;
    use alloc::sync::Arc;
    use alloc::vec;
    use core::sync::atomic::{AtomicBool, Ordering};
    use scarlet_ui::Icon;
    use scarlet_ui::element::{ElementRenderObject, LayoutConstraints};
    use scarlet_ui::views::menu::MenuRenderObject;
    use scarlet_ui::views::{MenuAction, MenuItemContent};
    use sws_protocol::window_types;

    fn snapshot(
        window_id: u32,
        title: &str,
        window_type: u32,
        visible: bool,
        focused: bool,
        minimized: bool,
    ) -> WindowSnapshot {
        WindowSnapshot {
            window_id,
            app_id: String::from("org.example.app"),
            title: String::from(title),
            window_type,
            visible,
            focused,
            minimized,
        }
    }

    #[test]
    fn shell_layout_uses_laptop_height_when_tablet_state_is_unknown_or_disabled() {
        assert_eq!(ShellLayout::from_tablet_mode(None).taskbar_height(), 32);
        assert_eq!(
            ShellLayout::from_tablet_mode(Some(false)).taskbar_height(),
            32
        );
    }

    #[test]
    fn tablet_mode_temporarily_keeps_the_approved_laptop_height() {
        assert_eq!(
            ShellLayout::from_tablet_mode(Some(true)).taskbar_height(),
            32
        );
        assert_eq!(ShellLayout::from_tablet_mode(Some(true)).popup_y(), 32);
    }

    #[test]
    fn physical_workarea_reserves_scaled_shell_height() {
        let layout = ShellLayout::from_tablet_mode(Some(true));
        let workarea = layout.physical_workarea(2880, 1800, 1500);
        assert_eq!(workarea.x, 0);
        assert_eq!(workarea.y, scale_u32(32, 1500) as i32);
        assert_eq!(workarea.width, 2880);
        assert_eq!(workarea.height, 1800 - scale_u32(32, 1500));
    }

    #[test]
    fn taskbar_resize_is_deduplicated_after_layout_sync() {
        let laptop = ShellLayout::from_tablet_mode(Some(false)).taskbar_window_size(1920.0);
        let tablet = ShellLayout::from_tablet_mode(Some(true)).taskbar_window_size(1920.0);
        assert!(!taskbar_resize_needed(laptop, laptop));
        assert!(!taskbar_resize_needed(laptop, tablet));
    }

    #[test]
    fn control_center_shadow_surface_keeps_the_requested_outer_margin() {
        let outsets = scarlet_ui::ElevationRole::Floating.paint_outsets();
        let (body_x, body_y) = control_center_body_position(1920.0, 32, 304.0);

        assert_eq!(body_x as f32 - outsets.left, 1920.0 - 8.0 - 324.0);
        assert_eq!(body_x as f32 + 304.0 + outsets.right, 1920.0 - 8.0);
        assert_eq!(body_y as f32 - outsets.top, 32.0 + 8.0);
    }

    #[test]
    fn tablet_mode_temporarily_uses_the_same_status_geometry_as_laptop() {
        let laptop = StatusItemTokens::for_layout(ShellLayout::from_tablet_mode(Some(false)));
        let tablet = StatusItemTokens::for_layout(ShellLayout::from_tablet_mode(Some(true)));
        assert_eq!(laptop.logical_height, 24.0);
        assert_eq!(laptop.font_size, 13.0);
        assert!(laptop.font_size * 1.2 + laptop.horizontal_padding * 2.0 <= laptop.logical_height);
        assert!(
            laptop.logical_height + laptop.bar_padding * 2.0
                <= ShellLayout::LAPTOP_TASKBAR_HEIGHT as f32
        );
        assert_eq!(tablet, laptop);
    }

    #[test]
    fn cpu_is_the_only_text_label_in_the_system_status_cluster() {
        let snapshot = StatusProviderSnapshot {
            cpu_percent: Some(17),
            audio_volume_percent: Some(50),
            audio_muted: Some(false),
            ..StatusProviderSnapshot::default()
        };
        assert_eq!(
            status_item_label(&snapshot, StatusPresentation::Compact, StatusItemId::Cpu),
            Some(String::from("CPU 17%"))
        );
        assert!(snapshot.preferences.is_visible(StatusItemId::Audio));
    }

    #[test]
    fn volume_icon_uses_the_official_tabler_outline_family_for_every_state() {
        assert_eq!(volume_status_icon(None, None), Icon::Volume3);
        assert_eq!(volume_status_icon(Some(50), Some(true)), Icon::Volume3);
        assert_eq!(volume_status_icon(Some(0), Some(false)), Icon::Volume3);
        assert_eq!(volume_status_icon(Some(1), Some(false)), Icon::Volume2);
        assert_eq!(volume_status_icon(Some(50), Some(false)), Icon::Volume2);
        assert_eq!(volume_status_icon(Some(51), Some(false)), Icon::Volume);
        assert_eq!(volume_status_icon(Some(100), Some(false)), Icon::Volume);
    }

    #[test]
    fn clock_uses_the_same_centered_menu_item_metrics_as_status_controls() {
        for layout in [
            ShellLayout::from_tablet_mode(Some(false)),
            ShellLayout::from_tablet_mode(Some(true)),
        ] {
            let tokens = StatusItemTokens::for_layout(layout);
            let clock = passive_clock_control("12:34", tokens);
            assert_eq!(clock.get_font_size(), tokens.font_size);
            assert_eq!(clock.get_padding(), tokens.horizontal_padding);
            assert!(!clock.is_selected());
            clock.invoke_on_click();
        }
    }

    #[test]
    fn window_model_filters_shell_and_non_normal_surfaces() {
        let mut shell = snapshot(2, "TaskBar", window_types::NORMAL, true, false, false);
        shell.app_id = String::from("org.scarlet-os.desktop.taskbar");
        let windows = build_window_model(vec![
            snapshot(1, "Editor", window_types::NORMAL, true, true, false),
            shell,
            snapshot(3, "Popup", window_types::ALWAYS_ON_TOP, true, false, false),
            snapshot(0, "Invalid", window_types::NORMAL, true, false, false),
        ]);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].window_id, 1);
    }

    #[test]
    fn window_model_orders_focused_visible_then_minimized_deterministically() {
        let windows = build_window_model(vec![
            snapshot(4, "Zulu", window_types::NORMAL, false, false, true),
            snapshot(3, "Beta", window_types::NORMAL, true, false, false),
            snapshot(2, "Alpha", window_types::NORMAL, true, false, false),
            snapshot(1, "Focused", window_types::NORMAL, true, true, false),
        ]);
        assert_eq!(
            windows
                .iter()
                .map(|window| window.window_id)
                .collect::<alloc::vec::Vec<_>>(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn overview_window_presentation_distinguishes_active_open_and_minimized() {
        let windows = build_window_model(vec![
            snapshot(1, "Focused", window_types::NORMAL, true, true, false),
            snapshot(2, "Open", window_types::NORMAL, true, false, false),
            snapshot(3, "Hidden", window_types::NORMAL, false, false, true),
        ]);
        assert_eq!(overview_window_status(&windows[0]), "Active");
        assert_eq!(overview_window_status(&windows[1]), "Open");
        assert_eq!(overview_window_status(&windows[2]), "Minimized");
    }

    #[test]
    fn overview_pagination_capacity_and_bounds_are_deterministic() {
        let laptop = ShellLayout::from_tablet_mode(Some(false));
        let tablet = ShellLayout::from_tablet_mode(Some(true));
        assert_eq!(
            OverviewGeometry::for_layout(tablet),
            OverviewGeometry::for_layout(laptop)
        );
        let capacity = overview_page_capacity(800, tablet);
        assert_eq!(capacity, overview_page_capacity(800, laptop));
        let popup_height = OVERVIEW_VERTICAL_PADDING
            + OVERVIEW_SEPARATOR_HEIGHT
            + ((OVERVIEW_SYSTEM_ROWS + OVERVIEW_NAVIGATION_ROWS + capacity) as f32
                * OverviewGeometry::for_layout(tablet).row_height);
        assert!(popup_height <= (800 - tablet.taskbar_height() - 8) as f32);
        assert_eq!(overview_page_count(23, 5), 5);
        assert_eq!(overview_page_bounds(23, 5, 0), (0, 5));
        assert_eq!(overview_page_bounds(23, 5, 3), (15, 20));
        assert_eq!(overview_page_bounds(23, 5, usize::MAX), (20, 23));
        assert_eq!(overview_page_bounds(0, 5, usize::MAX), (0, 0));
    }

    #[test]
    fn tablet_overview_exposes_active_app_and_top_level_application_menus() {
        let child = TaskMenuEntry::Item(TaskMenuItem {
            id: String::from("action"),
            title: String::from("Action"),
            enabled: true,
            shortcut: None,
            children: vec![],
        });
        let tree = MenuTree {
            items: vec![
                TaskMenuItem {
                    id: String::from("system_scarlet"),
                    title: String::from("Scarlet"),
                    enabled: true,
                    shortcut: None,
                    children: vec![child.clone()],
                },
                TaskMenuItem {
                    id: String::from("system_app"),
                    title: String::from("Active App"),
                    enabled: true,
                    shortcut: None,
                    children: vec![child.clone()],
                },
                TaskMenuItem {
                    id: String::from("file"),
                    title: String::from("File"),
                    enabled: true,
                    shortcut: None,
                    children: vec![child.clone()],
                },
                TaskMenuItem {
                    id: String::from("edit"),
                    title: String::from("Edit"),
                    enabled: true,
                    shortcut: None,
                    children: vec![child],
                },
            ],
        };
        assert_eq!(overview_app_menu_indices(&tree), vec![1, 2, 3]);
    }

    #[test]
    fn menu_renderer_treats_explicit_overview_action_as_full_touch_row() {
        let invoked = Arc::new(AtomicBool::new(false));
        let callback_invoked = invoked.clone();
        let items = vec![
            MenuItemContent::new("Window")
                .action(MenuAction::Submenu)
                .callback(move || callback_invoked.store(true, Ordering::Relaxed)),
        ];
        let mut renderer = MenuRenderObject::new(items, 48.0, 320.0);
        let size = renderer.layout(LayoutConstraints {
            min_width: 320.0,
            max_width: 320.0,
            min_height: 0.0,
            max_height: f32::INFINITY,
        });
        assert_eq!(size.height, 52.0);
        assert_eq!(renderer.hit_test(10.0, 1.9), None);
        assert_eq!(renderer.hit_test(10.0, 2.0), Some(0));
        assert_eq!(renderer.hit_test(10.0, 49.9), Some(0));
        assert_eq!(renderer.hit_test(10.0, 50.0), None);
        renderer.invoke_item(0);
        assert!(invoked.load(Ordering::Relaxed));
    }
}
