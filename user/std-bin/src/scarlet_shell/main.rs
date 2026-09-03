//! Scarlet workspace shell.
//!
//! Owns the desktop bar, Control Center, Home, and the privileged workspace
//! controller connection while SWS retains authoritative surface state.

extern crate alloc;

mod background;
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
    DESKTOP_FILE_MANAGER_OBJECT_PATH, DESKTOP_FILE_MANAGER_SHOW_METHOD, DESKTOP_SETTINGS_BUS_NAME,
    DESKTOP_SETTINGS_GET_STATUS_PREFERENCES_METHOD, DESKTOP_SETTINGS_INTERFACE,
    DESKTOP_SETTINGS_OBJECT_PATH, DESKTOP_SETTINGS_SERVICE_INTERFACE,
    DESKTOP_SETTINGS_SERVICE_OBJECT_PATH, DESKTOP_SETTINGS_SIGNAL_SENDER,
    DESKTOP_STATUS_PREFERENCES_CHANGED_SIGNAL, DESKTOP_STEMD_BUS_NAME, DESKTOP_STEMD_INTERFACE,
    DESKTOP_STEMD_LAUNCH_OR_FOCUS_METHOD, DESKTOP_STEMD_LIST_APPLICATIONS_METHOD,
    DESKTOP_STEMD_OBJECT_PATH, StatusItemId, StatusPreferences,
};
use scarlet_os::socket::Socket;
use scarlet_os::time;
use scarlet_os::{network, process, scheduler};
use scarlet_ui::buffer::Buffer;
use scarlet_ui::color::{Color, ColorPalette};
use scarlet_ui::element::{ElementRenderObject, LayoutConstraints};
use scarlet_ui::geometry::Size;
use scarlet_ui::graphics;
use scarlet_ui::platform::WindowPlacement;
use scarlet_ui::prelude::*;
use scarlet_ui::views::menu::MenuRenderObject;
use scarlet_ui::views::{MenuAction, MenuBar, MenuItem, MenuItemContent};
use scarlet_ui::{GridView, IconView, StateId, hstack, vstack};
use scarlet_ui::{
    Icon, IconSize, IconWeight, KeyCode, ListView, MenuBarModel, MenuItemModel, PlatformWindow,
    dismiss_window, open_window,
};
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
const HOME_SCENE_KEY: &str = "home";
const HOME_CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const HOME_CATALOG_TIMEOUT_MS: u64 = 3_000;
const HOME_SEARCH_RESULT_ROW_HEIGHT: f32 = 64.0;
const HOME_GRID_CELL_WIDTH: f32 = 116.0;
const HOME_GRID_COLUMN_SPACING: f32 = 14.0;
const HOME_GRID_ICON_SIZE: u16 = 45;
const HOME_GRID_ICON_PADDING: f32 = 12.5;
const HOME_GRID_LABEL_SIZE: f32 = 16.0;
const HOME_DRAWER_RESULTS_SPACING: f32 = 20.0;
const HOME_BACKDROP_TINT: Color = Color::rgba_f32(0.30, 0.34, 0.41, 0.78);
const STATUS_NAVIGATION_HOVER: Color = Color::rgba_f32(0.0, 0.0, 0.0, 0.24);
const STATUS_NAVIGATION_ACTIVE: Color = Color::rgba_f32(0.0, 0.0, 0.0, 0.36);
const STATUS_BAR_SETTINGS_LISTENER_BUS_NAME: &str =
    "org.scarlet-os.desktop.status-bar.settings-listener";
const SBUS_METHOD_TIMEOUT_MS: u64 = 1_000;
const CONTROL_CENTER_MARGIN: i32 = 8;
const OVERVIEW_SYSTEM_ROWS: usize = 3;
const OVERVIEW_NAVIGATION_ROWS: usize = 2;
const OVERVIEW_SEPARATOR_HEIGHT: f32 = 1.0;
const OVERVIEW_VERTICAL_PADDING: f32 = 4.0;

fn set_state_if_changed<T>(state: &State<T>, next: T) -> bool
where
    T: Clone + PartialEq,
{
    if state.get() == next {
        return false;
    }
    state.set(next);
    true
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
struct HomeApplication {
    app_id: String,
    name: String,
    icon: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspaceCommand {
    ShowHome,
    ShowWorkspace,
    ReturnToWorkspace,
    ToggleOverview,
    Cycle(i32),
    ToggleSplit,
}

fn repaired_shell_layout_after_removal(
    layout: sws::TabletLayout,
    removed_window_id: u32,
    remaining_window_ids: &[u32],
) -> sws::TabletLayout {
    let fallback = || {
        remaining_window_ids
            .first()
            .copied()
            .map_or(sws::TabletLayout::Empty, |window_id| {
                sws::TabletLayout::Single { window_id }
            })
    };
    match layout {
        sws::TabletLayout::Empty => fallback(),
        sws::TabletLayout::Single { window_id } if window_id == removed_window_id => fallback(),
        sws::TabletLayout::Single { window_id } => sws::TabletLayout::Single { window_id },
        sws::TabletLayout::Split {
            first_window_id,
            second_window_id,
            ..
        } if first_window_id == removed_window_id => {
            if remaining_window_ids.contains(&second_window_id) {
                sws::TabletLayout::Single {
                    window_id: second_window_id,
                }
            } else {
                fallback()
            }
        }
        sws::TabletLayout::Split {
            first_window_id,
            second_window_id,
            ..
        } if second_window_id == removed_window_id => {
            if remaining_window_ids.contains(&first_window_id) {
                sws::TabletLayout::Single {
                    window_id: first_window_id,
                }
            } else {
                fallback()
            }
        }
        layout => layout,
    }
}

fn workspace_transaction_for_command(
    state: &sws::WorkspaceState,
    command: WorkspaceCommand,
) -> Option<sws::WorkspaceTransaction> {
    let mut active_workspace = state.active_workspace;
    let mut presentation = state.presentation;
    let mut workspaces = state.workspaces.clone();

    match command {
        WorkspaceCommand::ShowHome => {
            if presentation == sws::ShellPresentation::Home {
                return None;
            }
            presentation = sws::ShellPresentation::Home;
        }
        WorkspaceCommand::ShowWorkspace => {
            if presentation == sws::ShellPresentation::Workspace {
                return None;
            }
            presentation = sws::ShellPresentation::Workspace;
        }
        WorkspaceCommand::ReturnToWorkspace => {
            active_workspace = state.normal_workspace;
            if presentation == sws::ShellPresentation::Workspace
                && active_workspace == state.active_workspace
            {
                return None;
            }
            presentation = sws::ShellPresentation::Workspace;
        }
        WorkspaceCommand::ToggleOverview => {
            if presentation == sws::ShellPresentation::Overview {
                active_workspace = state.normal_workspace;
                presentation = sws::ShellPresentation::Workspace;
            } else {
                presentation = sws::ShellPresentation::Overview;
            }
        }
        WorkspaceCommand::Cycle(direction) => {
            if workspaces.len() < 2 || direction == 0 {
                return None;
            }
            let index = workspaces
                .iter()
                .position(|workspace| workspace.id == active_workspace)?;
            let next = if direction < 0 {
                index.checked_sub(1)?
            } else {
                let next = index.saturating_add(1);
                (next < workspaces.len()).then_some(next)?
            };
            active_workspace = workspaces[next].id;
            presentation = sws::ShellPresentation::Workspace;
        }
        WorkspaceCommand::ToggleSplit => {
            let active_index = workspaces
                .iter()
                .position(|workspace| workspace.id == active_workspace)?;
            match workspaces[active_index].tablet_layout {
                sws::TabletLayout::Split {
                    first_window_id, ..
                } => {
                    workspaces[active_index].tablet_layout = sws::TabletLayout::Single {
                        window_id: first_window_id,
                    };
                }
                sws::TabletLayout::Empty => return None,
                sws::TabletLayout::Single {
                    window_id: first_window_id,
                } => {
                    let local_candidate = workspaces[active_index]
                        .window_ids
                        .iter()
                        .copied()
                        .find(|window_id| *window_id != first_window_id);
                    let donor = if local_candidate.is_none() {
                        (1..workspaces.len()).find_map(|offset| {
                            let index = (active_index + offset) % workspaces.len();
                            workspaces[index]
                                .window_ids
                                .first()
                                .copied()
                                .map(|window_id| (index, window_id))
                        })
                    } else {
                        None
                    };
                    let second_window_id = local_candidate.or_else(|| donor.map(|(_, id)| id))?;

                    if let Some((donor_index, _)) = donor {
                        workspaces[donor_index]
                            .window_ids
                            .retain(|window_id| *window_id != second_window_id);
                        workspaces[donor_index].tablet_layout = repaired_shell_layout_after_removal(
                            workspaces[donor_index].tablet_layout,
                            second_window_id,
                            &workspaces[donor_index].window_ids,
                        );
                        workspaces[active_index].window_ids.push(second_window_id);
                    }
                    let active_index = workspaces
                        .iter()
                        .position(|workspace| workspace.id == active_workspace)?;
                    workspaces[active_index].tablet_layout = sws::TabletLayout::Split {
                        axis: sws::SplitAxis::Horizontal,
                        first_window_id,
                        second_window_id,
                        ratio_milli: 500,
                    };
                }
            }
            presentation = sws::ShellPresentation::Workspace;
        }
    }

    Some(sws::WorkspaceTransaction {
        base_generation: state.generation,
        active_workspace,
        presentation,
        workspaces,
        // Runtime animation remains deliberately disabled until the complete
        // state and input paths have settled.
        transition: sws::TransitionSpec::default(),
    })
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
    let available_height = screen_height.saturating_sub(layout.status_bar_height() + 8) as f32;
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

fn is_status_bar_debug_enabled() -> bool {
    static LOG_CACHE: AtomicU8 = AtomicU8::new(u8::MAX);
    let cached = LOG_CACHE.load(Ordering::Relaxed);
    if cached != u8::MAX {
        return cached != 0;
    }
    let enabled = match std::env::var("SWS_LOG") {
        Ok(value) => matches!(
            value.as_str(),
            "debug" | "DEBUG" | "3" | "trace" | "TRACE" | "4"
        ),
        Err(_) => false,
    };
    LOG_CACHE.store(enabled as u8, Ordering::Relaxed);
    enabled
}

macro_rules! status_bar_debug {
    ($($arg:tt)*) => {
        if is_status_bar_debug_enabled() {
            std::println!($($arg)*);
        }
    };
}

/// Geometry policy shared by the desktop shell surfaces.
///
/// The shell deliberately keeps this independent of individual views so a
/// future overview, workspace switcher, or quick-settings surface can use the
/// same logical and physical coordinate system as the StatusBar.
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
    const LAPTOP_STATUS_BAR_HEIGHT: u32 = 32;
    const TABLET_STATUS_BAR_HEIGHT: u32 = 40;

    /// Build a layout from the compositor state, treating an unknown state as
    /// the laptop/desktop default.
    const fn from_tablet_mode(tablet_mode: Option<bool>) -> Self {
        Self {
            tablet_mode: matches!(tablet_mode, Some(true)),
        }
    }

    /// Return the StatusBar height in logical pixels.
    const fn status_bar_height(self) -> u32 {
        if self.tablet_mode {
            Self::TABLET_STATUS_BAR_HEIGHT
        } else {
            Self::LAPTOP_STATUS_BAR_HEIGHT
        }
    }

    /// Return the StatusBar window size for a logical output width.
    fn status_bar_window_size(self, screen_width: f32) -> Size {
        Size::new(screen_width, self.status_bar_height() as f32)
    }

    /// Return the logical Y coordinate immediately below the StatusBar.
    const fn popup_y(self) -> i32 {
        self.status_bar_height() as i32
    }

    /// Return the physical popup anchor directly below the StatusBar.
    fn physical_popup_y(self, scale_milli: u32) -> i32 {
        scale_u32(self.popup_y() as u32, scale_milli) as i32
    }

    /// Return a physical workarea after reserving the scaled StatusBar height.
    fn physical_workarea(
        self,
        physical_width: u32,
        physical_height: u32,
        scale_milli: u32,
    ) -> PhysicalWorkarea {
        let physical_bar_height = scale_u32(self.status_bar_height(), scale_milli);
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
    const fn for_layout(layout: ShellLayout) -> Self {
        if layout.tablet_mode {
            Self {
                logical_height: 30.0,
                font_size: 14.0,
                horizontal_padding: 6.0,
                spacing: 5.0,
                bar_padding: MENU_BAR_OUTER_PADDING,
            }
        } else {
            Self {
                logical_height: 24.0,
                font_size: 13.0,
                horizontal_padding: 3.0,
                spacing: 3.0,
                bar_padding: MENU_BAR_OUTER_PADDING,
            }
        }
    }
}

fn build_passive_clock(
    label: impl Into<String>,
    tokens: StatusItemTokens,
    foreground: Option<Color>,
) -> impl View + Clone {
    passive_clock_control(label, tokens, foreground).frame_height(tokens.logical_height)
}

fn menu_item_with_foreground(item: MenuItem, foreground: Option<Color>) -> MenuItem {
    match foreground {
        Some(color) => item
            .foreground_color(color)
            .interaction_background_colors(STATUS_NAVIGATION_HOVER, STATUS_NAVIGATION_ACTIVE),
        None => item,
    }
}

fn status_text_control(
    label: impl Into<String>,
    tokens: StatusItemTokens,
    foreground: Option<Color>,
) -> MenuItem {
    let item = MenuItem::new(label)
        .font_size(tokens.font_size)
        .padding(tokens.horizontal_padding);
    menu_item_with_foreground(item, foreground)
}

fn passive_clock_control(
    label: impl Into<String>,
    tokens: StatusItemTokens,
    foreground: Option<Color>,
) -> MenuItem {
    status_text_control(label, tokens, foreground)
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
    foreground: Option<Color>,
) -> impl View + Clone {
    let mut items = Vec::new();
    if let Some(cpu_label) = status_item_label(&snapshot, presentation, StatusItemId::Cpu) {
        items.push(boxed(
            status_text_control(cpu_label, tokens, foreground).frame_height(tokens.logical_height),
        ));
    }
    if snapshot.preferences.is_visible(StatusItemId::Audio) {
        let volume_icon = volume_status_icon(snapshot.audio_volume_percent, snapshot.audio_muted);
        let item = MenuItem::new("")
            .icon(volume_icon)
            .icon_size(IconSize::Small)
            .font_size(tokens.font_size)
            .padding(tokens.horizontal_padding)
            .on_click(move || toggle_control_center(control_center_open.clone()));
        let item = menu_item_with_foreground(item, foreground);
        items.push(boxed(
            item.frame(tokens.logical_height, tokens.logical_height),
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
                "[StatusBar] Connected to SWS after {} attempt(s); screen={}x{} scale_milli={} status_bar_height={}",
                attempt + 1,
                width,
                height,
                scale_milli,
                layout.status_bar_height(),
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
        "[StatusBar] Workarea: x={}, y={}, width={}, height={}",
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

/// Return whether a platform StatusBar surface must change size.
///
/// Keeping this comparison pure prevents the sync hook from issuing a resize
/// on every application-runner tick after a layout transition has settled.
fn status_bar_resize_needed(current: Size, desired: Size) -> bool {
    current != desired
}

fn control_center_body_position(
    screen_width: f32,
    screen_height: f32,
    popup_y: i32,
    body_size: Size,
    presentation: ControlCenterPresentation,
) -> (i32, i32) {
    let outsets = ElevationRole::Floating.paint_outsets();
    match presentation {
        ControlCenterPresentation::LaptopPopover => (
            (screen_width - body_size.width - CONTROL_CENTER_MARGIN as f32 - outsets.right).max(0.0)
                as i32,
            popup_y + CONTROL_CENTER_MARGIN + outsets.top as i32,
        ),
        ControlCenterPresentation::TabletSheet => (
            ((screen_width - body_size.width) / 2.0).max(0.0) as i32,
            (screen_height - body_size.height - 24.0 - outsets.bottom)
                .max((popup_y + CONTROL_CENTER_MARGIN) as f32) as i32,
        ),
    }
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
            "[StatusBar] Input environment generation {} selected status_bar_height={}",
            environment.generation,
            next_layout.status_bar_height()
        );
        publish_current_workarea(conn, next_layout);
    }
}

/// Keep an independent SWS connection subscribed to shell-environment changes.
///
/// The ScarletUI runner owns a different connection, so a listener transport
/// failure cannot stop the StatusBar from rendering. Each reconnect re-queries
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
            println!("[StatusBar] Input-environment listener reconnect failed; retrying");
            std::thread::sleep(Duration::from_millis(SWS_RETRY_DELAY_MS));
            continue;
        };

        if let Some(environment) = input_environment {
            apply_input_environment(&conn, environment, &shell_layout, &open_menu_index);
        }

        loop {
            if conn.dispatch().is_err() {
                println!("[StatusBar] Input-environment listener transport lost; reconnecting");
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

/// Unified visual workspace shell application.
#[derive(View, Clone)]
struct ShellApp {
    clock: State<u32>,
    screen_width: State<f32>,
    screen_height: State<f32>,
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
    home_applications: State<Vec<HomeApplication>>,
    home_filtered_applications: State<Vec<HomeApplication>>,
    home_query: State<String>,
    home_search_focused: State<bool>,
    home_selected: State<Option<usize>>,
    home_hovered: State<Option<usize>>,
    home_window_id: State<u32>,
    workspace_state: State<Option<sws::WorkspaceState>>,
    workspace_commands: State<Vec<WorkspaceCommand>>,
}

impl ShellApp {
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
            screen_height: State::new(StateId::new(22), 1080.0),
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
            home_applications: State::new(StateId::new(23), Vec::new()),
            home_filtered_applications: State::new(StateId::new(28), Vec::new()),
            home_query: State::new(StateId::new(29), String::new()),
            home_search_focused: State::new(StateId::new(31), false),
            home_selected: State::new(StateId::new(24), None),
            home_hovered: State::new(StateId::new(30), None),
            home_window_id: State::new(StateId::new(25), 0),
            workspace_state: State::new(StateId::new(26), None),
            workspace_commands: State::new(StateId::new(27), Vec::new()),
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
    Vec::new()
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
            .register_service(STATUS_BAR_SETTINGS_LISTENER_BUS_NAME)
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
        let sampled = provider.snapshot(&preferences, scheduler::cpu_usage(), audio_state);
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
    let Ok(interfaces) = network::list_interface_configs() else {
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
                let name = interface.interface_name().unwrap_or_default().to_string();
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
            task_count: process::task_count().map(|count| count.min(u32::MAX as usize) as u32),
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
            process::shutdown(process::ShutdownType::PowerOff);
        }
        ControlCenterAction::ConfirmReboot => {
            process::shutdown(process::ShutdownType::Reboot);
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
    println!("[StatusBar] File Manager service is not ready");
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

fn send_launch_command(command: u8, app_id: &[u8]) {
    if let Ok(mut stream) = Socket::new()
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
const HOME_BUTTON_WIDTH: f32 = 64.0;
const LEADING_CONTROLS_SPACING: f32 = 8.0;

#[derive(Clone, Copy)]
struct MenuBarMetrics {
    font_size: f32,
    item_padding: f32,
    item_spacing: f32,
}

impl MenuBarMetrics {
    const fn for_layout(layout: ShellLayout) -> Self {
        if layout.tablet_mode {
            Self {
                font_size: 14.0,
                item_padding: 5.0,
                item_spacing: 4.0,
            }
        } else {
            Self {
                font_size: MENU_BAR_FONT_SIZE,
                item_padding: MENU_BAR_ITEM_PADDING,
                item_spacing: MENU_BAR_ITEM_SPACING,
            }
        }
    }
}

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

fn menu_bar_item_width(label: &str, layout: ShellLayout) -> f32 {
    let metrics = MenuBarMetrics::for_layout(layout);
    let (text_w, _text_h) = graphics::measure_text_sized(label, metrics.font_size);
    text_w as f32 + metrics.item_padding * 2.0
}

fn menu_bar_popup_x(items: &[TaskMenuItem], index: usize, layout: ShellLayout) -> f32 {
    let metrics = MenuBarMetrics::for_layout(layout);
    let mut x = MENU_BAR_OUTER_PADDING + HOME_BUTTON_WIDTH + LEADING_CONTROLS_SPACING;
    for (i, item) in items.iter().enumerate() {
        if i >= index {
            break;
        }
        x += menu_bar_item_width(&item.title, layout) + metrics.item_spacing;
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

fn status_bar_menu_items(
    menu_tree: &MenuTree,
    presentation: sws::ShellPresentation,
) -> Vec<TaskMenuItem> {
    if presentation == sws::ShellPresentation::Workspace {
        return menu_tree.items.clone();
    }
    menu_tree
        .items
        .iter()
        .filter(|item| item.id == "system_scarlet")
        .cloned()
        .collect()
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
            "[StatusBar] Failed to parse menu JSON (len={}, cleaned_len={}, candidate_len={})",
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
    layout: ShellLayout,
    foreground: Option<Color>,
) -> MenuBar {
    // println!(
    //     "[StatusBar] build_menu_bar_view: {} items, active_window_id={}",
    //     items.len(),
    //     active_window_id
    // );
    let has_children_by_index: Vec<bool> =
        items.iter().map(|item| !item.children.is_empty()).collect();
    let metrics = MenuBarMetrics::for_layout(layout);
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

            let menu_item = MenuItem::new(item.title.as_str())
                .font_size(metrics.font_size)
                .padding(metrics.item_padding)
                .selected(is_open);
            let menu_item = menu_item_with_foreground(menu_item, foreground);

            menu_item
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
        .spacing(metrics.item_spacing)
        .on_hover_index(move |idx| {
            if !hover_children.get(idx).copied().unwrap_or(false) {
                return;
            }
            if open_state_bar.get().is_some() && open_state_bar.get() != Some(idx) {
                open_state_bar.set(Some(idx));
            }
        })
}

fn enqueue_workspace_command(commands: &State<Vec<WorkspaceCommand>>, command: WorkspaceCommand) {
    commands.update(|pending| pending.push(command));
}

fn build_home_button(
    state: Option<sws::WorkspaceState>,
    commands: State<Vec<WorkspaceCommand>>,
) -> impl View + Clone {
    let shell_navigation = state
        .as_ref()
        .is_some_and(|state| state.presentation != sws::ShellPresentation::Workspace);
    let position = state.as_ref().map_or(String::from("—"), |state| {
        let index = state
            .workspaces
            .iter()
            .position(|workspace| workspace.id == state.active_workspace)
            .unwrap_or(0);
        format!("{}/{}", index + 1, state.workspaces.len())
    });
    let palette = if shell_navigation {
        ColorPalette::dark()
    } else {
        ColorPalette::light()
    };
    hstack! {
        IconView::new(Icon::Apps)
            .size(IconSize::Small)
            .color(palette.text()),
        Text::new(position)
            .font_size(12.0)
            .color(palette.text())
    }
    .spacing(4.0)
    .alignment(Alignment::Center)
    .padding(5.0)
    .frame(HOME_BUTTON_WIDTH, 24.0)
    .background(
        palette
            .text()
            .with_opacity(if shell_navigation { 0.14 } else { 0.06 }),
    )
    .clip_radius(10.0)
    .on_click(move || enqueue_workspace_command(&commands, WorkspaceCommand::ToggleOverview))
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
                        println!("[StatusBar] System shutdown requested");
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
    let files_open = open_menu_index.clone();
    let settings_open = open_menu_index.clone();
    let mut items = vec![
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

fn home_icon(name: &str) -> Icon {
    match name {
        "apps" => Icon::Package,
        "applications-development" | "code" => Icon::Code,
        "file-description" => Icon::FileDescription,
        "file-music" => Icon::FileMusic,
        "folder" => Icon::Folder,
        "image" => Icon::Photo,
        "preferences-system" => Icon::Settings,
        "preferences-system-time" => Icon::Clock,
        "text-editor" => Icon::FileText,
        "utilities-system-monitor" => Icon::ChartBar,
        "utilities-terminal" => Icon::Terminal,
        "video" | "multimedia-player" => Icon::Video,
        _ => Icon::Apps,
    }
}

fn home_icon_tile_color(name: &str) -> Color {
    match name {
        "apps" => Color::rgb(186, 95, 43),
        "applications-development" | "code" => Color::rgb(47, 94, 174),
        "file-description" => Color::rgb(28, 119, 126),
        "file-music" => Color::rgb(152, 62, 161),
        "folder" => Color::rgb(46, 112, 190),
        "image" => Color::rgb(180, 62, 121),
        "preferences-system" => Color::rgb(84, 96, 119),
        "preferences-system-time" => Color::rgb(195, 76, 54),
        "text-editor" => Color::rgb(31, 132, 103),
        "utilities-system-monitor" => Color::rgb(25, 128, 139),
        "utilities-terminal" => Color::rgb(68, 78, 96),
        "video" | "multimedia-player" => Color::rgb(103, 70, 177),
        _ => Color::rgb(184, 55, 79),
    }
}

fn launch_home_application(source_window_id: u32, application: &HomeApplication) -> bool {
    let Ok(connection) = sws::Connection::connect("/tmp/sws.sock") else {
        return false;
    };
    let Ok(token) = connection.request_activation_token(source_window_id, &application.app_id)
    else {
        return false;
    };
    SbusConnection::connect()
        .and_then(|mut connection| {
            connection.call_method_timeout(
                DESKTOP_STEMD_BUS_NAME,
                DESKTOP_STEMD_OBJECT_PATH,
                DESKTOP_STEMD_INTERFACE,
                DESKTOP_STEMD_LAUNCH_OR_FOCUS_METHOD,
                vec![
                    SbusArgument::String(application.app_id.clone()),
                    SbusArgument::String(token),
                ],
                HOME_CATALOG_TIMEOUT_MS,
            )
        })
        .is_ok()
}

fn load_home_applications() -> Vec<HomeApplication> {
    let Ok(arguments) = SbusConnection::connect().and_then(|mut connection| {
        connection.call_method_timeout(
            DESKTOP_STEMD_BUS_NAME,
            DESKTOP_STEMD_OBJECT_PATH,
            DESKTOP_STEMD_INTERFACE,
            DESKTOP_STEMD_LIST_APPLICATIONS_METHOD,
            Vec::new(),
            HOME_CATALOG_TIMEOUT_MS,
        )
    }) else {
        return Vec::new();
    };

    let mut applications = Vec::new();
    for fields in arguments.chunks(3) {
        let [
            SbusArgument::String(app_id),
            SbusArgument::String(name),
            SbusArgument::String(icon),
        ] = fields
        else {
            continue;
        };
        if app_id.is_empty()
            || name.is_empty()
            || app_id == "org.scarlet-os.desktop.launcher"
            || is_shell_app(app_id)
        {
            continue;
        }
        applications.push(HomeApplication {
            app_id: app_id.clone(),
            name: name.clone(),
            icon: icon.clone(),
        });
    }
    applications.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.app_id.cmp(&right.app_id))
    });
    applications
}

fn refresh_home_catalog(applications: State<Vec<HomeApplication>>) {
    let mut previous = Vec::new();
    loop {
        let next = load_home_applications();
        if next != previous {
            applications.set(next.clone());
            previous = next;
        }
        std::thread::sleep(HOME_CATALOG_REFRESH_INTERVAL);
    }
}

fn filter_home_applications(applications: &[HomeApplication], query: &str) -> Vec<HomeApplication> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return applications.to_vec();
    }
    let mut matches = applications
        .iter()
        .filter_map(|application| {
            let name = application.name.to_lowercase();
            let app_id = application.app_id.to_lowercase();
            let rank = if name == query {
                Some(0)
            } else if name.starts_with(&query) {
                Some(1)
            } else if name.contains(&query) {
                Some(2)
            } else if app_id == query {
                Some(3)
            } else if app_id.starts_with(&query) {
                Some(4)
            } else if app_id.contains(&query) {
                Some(5)
            } else {
                None
            }?;
            Some((rank, application.clone()))
        })
        .collect::<Vec<_>>();
    matches.sort_by_key(|(rank, _)| *rank);
    matches
        .into_iter()
        .map(|(_, application)| application)
        .collect()
}

fn home_grid_columns(width: f32) -> usize {
    let horizontal_padding = if width < 720.0 { 20.0 } else { 48.0 };
    let drawer_padding = if width < 720.0 { 16.0 } else { 24.0 };
    let available_width = (width - horizontal_padding * 2.0 - drawer_padding * 2.0).max(1.0);
    (((available_width + HOME_GRID_COLUMN_SPACING)
        / (HOME_GRID_CELL_WIDTH + HOME_GRID_COLUMN_SPACING)) as usize)
        .clamp(1, 6)
}

fn application_drawer_accepts_keyboard(presentation: sws::ShellPresentation) -> bool {
    presentation == sws::ShellPresentation::Home
}

fn next_home_selection_index(
    current: usize,
    application_count: usize,
    columns: usize,
    keycode: KeyCode,
) -> usize {
    let last = application_count.saturating_sub(1);
    let current = current.min(last);
    let columns = columns.max(1);
    match keycode {
        KeyCode::Left => current.saturating_sub(1),
        KeyCode::Right => current.saturating_add(1).min(last),
        KeyCode::Up => current.saturating_sub(columns),
        KeyCode::Down => current.saturating_add(columns).min(last),
        _ => current,
    }
}

fn maintain_home_filter(
    applications: State<Vec<HomeApplication>>,
    query: State<String>,
    filtered: State<Vec<HomeApplication>>,
    selected: State<Option<usize>>,
    hovered: State<Option<usize>>,
) {
    let mut previous_applications = Vec::new();
    let mut previous_query = String::new();
    loop {
        let current_applications = applications.get();
        let current_query = query.get();
        if current_applications != previous_applications || current_query != previous_query {
            let next = filter_home_applications(&current_applications, &current_query);
            selected.set((!next.is_empty()).then_some(0));
            hovered.set(None);
            filtered.set(next);
            previous_applications = current_applications;
            previous_query = current_query;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}

fn maintain_workspace_shell_role(
    state: State<Option<sws::WorkspaceState>>,
    commands: State<Vec<WorkspaceCommand>>,
) {
    loop {
        let Ok(connection) = sws::Connection::connect("/tmp/sws.sock") else {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        let Ok(mut snapshot) = connection.register_system_shell() else {
            std::thread::sleep(Duration::from_millis(250));
            continue;
        };
        println!(
            "[Shell] Registered workspace role at generation {}",
            snapshot.generation
        );
        state.set(Some(snapshot.clone()));
        loop {
            if let Some(command) = commands.get().first().copied() {
                if let Ok(latest) = connection.get_workspace_state() {
                    snapshot = latest;
                }
                let result = match workspace_transaction_for_command(&snapshot, command) {
                    Some(transaction) => connection.apply_workspace_transaction(&transaction),
                    None => Ok(snapshot.clone()),
                };
                match result {
                    Ok(updated) => {
                        snapshot = updated;
                        state.set(Some(snapshot.clone()));
                    }
                    Err(error) => {
                        println!("[Shell] Workspace command failed: {:?}", error);
                    }
                }
                commands.update(|pending| {
                    if pending.first().copied() == Some(command) {
                        pending.remove(0);
                    }
                });
            }
            if connection.dispatch().is_err() {
                state.set(None);
                break;
            }
            while let Some(event) = connection.poll_event() {
                if let sws::event::Event::WorkspaceStateChanged(updated) = event {
                    snapshot = updated;
                    state.set(Some(snapshot.clone()));
                }
            }
            std::thread::sleep(Duration::from_millis(16));
        }
    }
}

impl ShellApp {
    fn shell_presentation(&self) -> sws::ShellPresentation {
        self.workspace_state
            .get()
            .map_or(sws::ShellPresentation::Workspace, |state| {
                state.presentation
            })
    }

    fn show_application_drawer(&self) {
        if self.shell_presentation() != sws::ShellPresentation::Home {
            enqueue_workspace_command(&self.workspace_commands, WorkspaceCommand::ShowHome);
        }
    }

    fn lower_application_drawer(&self) {
        if self.shell_presentation() == sws::ShellPresentation::Home {
            self.home_search_focused.set(false);
            enqueue_workspace_command(&self.workspace_commands, WorkspaceCommand::ToggleOverview);
        }
    }

    fn dismiss_shell_depth(&self) {
        match self.shell_presentation() {
            sws::ShellPresentation::Home => self.lower_application_drawer(),
            sws::ShellPresentation::Overview => enqueue_workspace_command(
                &self.workspace_commands,
                WorkspaceCommand::ReturnToWorkspace,
            ),
            sws::ShellPresentation::Workspace => {}
        }
    }

    fn launch_home_application(&self, application: HomeApplication) {
        if launch_home_application(self.home_window_id.get(), &application) {
            self.home_query.set(String::new());
            self.home_search_focused.set(false);
            enqueue_workspace_command(&self.workspace_commands, WorkspaceCommand::ShowWorkspace);
        }
    }

    fn launch_selected_home_application(&self) {
        let applications = self.home_filtered_applications.get();
        let Some(application) = self
            .home_selected
            .get()
            .and_then(|index| applications.get(index))
            .cloned()
        else {
            return;
        };
        self.launch_home_application(application);
    }

    fn move_home_selection(&self, keycode: KeyCode) {
        let applications = self.home_filtered_applications.get();
        if applications.is_empty() {
            self.home_selected.set(None);
            return;
        }
        let current = self
            .home_selected
            .get()
            .unwrap_or(0)
            .min(applications.len().saturating_sub(1));
        let columns = if self.home_query.get().trim().is_empty() {
            home_grid_columns(self.screen_width.get())
        } else {
            1
        };
        self.home_selected.set(Some(next_home_selection_index(
            current,
            applications.len(),
            columns,
            keycode,
        )));
        self.home_hovered.set(None);
    }

    fn handle_home_key(&self, event: KeyEvent) -> bool {
        if !application_drawer_accepts_keyboard(self.shell_presentation()) {
            return false;
        }
        match event {
            KeyEvent::Char { c } if !c.is_control() => {
                self.home_search_focused.set(true);
                self.home_query.update(|query| query.push(c));
                true
            }
            KeyEvent::Pressed { keycode, .. } => {
                let searching = !self.home_query.get().trim().is_empty();
                match keycode {
                    KeyCode::Up | KeyCode::Down => {
                        self.move_home_selection(keycode);
                        true
                    }
                    KeyCode::Left | KeyCode::Right if !searching => {
                        self.move_home_selection(keycode);
                        true
                    }
                    KeyCode::Enter => {
                        self.launch_selected_home_application();
                        true
                    }
                    KeyCode::Escape => {
                        if searching {
                            self.home_query.set(String::new());
                            self.home_search_focused.set(false);
                        } else {
                            self.dismiss_shell_depth();
                        }
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn home_application_cell(
        &self,
        index: usize,
        application: HomeApplication,
        selected: Option<usize>,
        row_height: f32,
    ) -> impl View + Clone + use<> {
        let palette = ColorPalette::dark();
        let hovered = self.home_hovered.get() == Some(index);
        let background = if selected == Some(index) && hovered {
            palette.text().with_opacity(0.30)
        } else if selected == Some(index) {
            palette.text().with_opacity(0.24)
        } else if hovered {
            palette.text().with_opacity(0.18)
        } else {
            palette.window_background().with_opacity(0.0)
        };
        let icon_tile_color = home_icon_tile_color(&application.icon);
        let icon_tile_size = HOME_GRID_ICON_SIZE as f32 + HOME_GRID_ICON_PADDING * 2.0;
        let label_height =
            graphics::measure_text_sized(&application.name, HOME_GRID_LABEL_SIZE).1 as f32;
        let label_gap = 4.0;
        let outer_padding =
            ((row_height - icon_tile_size - label_gap - label_height) * 0.5).max(0.0);
        let hover_state = self.home_hovered.clone();
        let exit_state = hover_state.clone();
        let launch_app = self.clone();
        let launch_application = application.clone();
        vstack! {
            Spacer::new().frame(1.0, outer_padding),
            IconView::new(home_icon(&application.icon))
                .size(IconSize::Pixels(HOME_GRID_ICON_SIZE))
                .weight(IconWeight::Bold)
                .color(Color::WHITE)
                .padding(HOME_GRID_ICON_PADDING)
                .background(icon_tile_color)
                .clip_radius(17.5),
            Spacer::new().frame(1.0, label_gap),
            Text::new(application.name)
                .font_size(HOME_GRID_LABEL_SIZE)
                .color(palette.text()),
            Spacer::new().frame(1.0, outer_padding),
        }
        .spacing(0.0)
        .alignment(Alignment::Center)
        .frame(f32::INFINITY, row_height)
        .background(background)
        .clip_radius(13.0)
        .on_hover(move || hover_state.set(Some(index)))
        .on_exit(move || {
            if exit_state.get() == Some(index) {
                exit_state.set(None);
            }
        })
        .on_click(move || launch_app.launch_home_application(launch_application.clone()))
    }

    fn home_application_list_row(
        &self,
        index: usize,
        application: HomeApplication,
        selected: Option<usize>,
    ) -> impl View + Clone + use<> {
        let palette = ColorPalette::dark();
        let hovered = self.home_hovered.get() == Some(index);
        let background = if selected == Some(index) && hovered {
            palette.text().with_opacity(0.30)
        } else if selected == Some(index) {
            palette.text().with_opacity(0.24)
        } else if hovered {
            palette.text().with_opacity(0.18)
        } else {
            palette.window_background().with_opacity(0.0)
        };
        let icon_tile_color = home_icon_tile_color(&application.icon);
        let hover_state = self.home_hovered.clone();
        let exit_state = hover_state.clone();
        let launch_app = self.clone();
        let launch_application = application.clone();

        hstack! {
            IconView::new(home_icon(&application.icon))
                .size(IconSize::Pixels(26))
                .weight(IconWeight::Bold)
                .color(Color::WHITE)
                .padding(8.0)
                .background(icon_tile_color)
                .clip_radius(11.0),
            hstack! {
                Text::new(application.name)
                    .font_size(15.0)
                    .color(palette.text()),
                Text::new(application.app_id)
                    .font_size(13.0)
                    .color(palette.text_secondary())
                    .padding_insets(EdgeInsets {
                        top: 0.0,
                        left: 8.0,
                        bottom: 0.0,
                        right: 0.0,
                    }),
            }
            .spacing(8.0),
            Spacer::new(),
            Text::new("Application")
                .font_size(13.0)
                .color(palette.text_secondary()),
        }
        .spacing(14.0)
        .alignment(Alignment::Center)
        .padding(12.0)
        .frame(f32::INFINITY, HOME_SEARCH_RESULT_ROW_HEIGHT)
        .background(background)
        .clip_radius(10.0)
        .on_hover(move || hover_state.set(Some(index)))
        .on_exit(move || {
            if exit_state.get() == Some(index) {
                exit_state.set(None);
            }
        })
        .on_click(move || launch_app.launch_home_application(launch_application.clone()))
    }

    fn home_content(&self) -> impl View + Clone + use<> {
        let width = self.screen_width.get().max(320.0);
        let height = self.screen_height.get().max(320.0);
        let shell_layout = self.shell_layout.get();
        let presentation = self.shell_presentation();
        let shell_state_ready = self.workspace_state.get().is_some();
        let shell_visible = shell_state_ready && presentation != sws::ShellPresentation::Workspace;
        let drawer_padding = if width < 720.0 { 16.0 } else { 24.0 };
        let columns = home_grid_columns(width);
        let row_height = if height < 760.0 { 110.0 } else { 116.0 };
        let bar_height = shell_layout.status_bar_height() as f32;
        let work_height = (height - bar_height).max(1.0);
        // This surface is prepared while hidden. Keep its geometry identical
        // before and after the asynchronous workspace-state notification so
        // the first visible frame cannot expose the drawer moving into place.
        let workspace_total_height = sws_protocol::workspace::workspace_region_height(
            work_height as u32,
            shell_layout.tablet_mode,
            presentation,
            1000,
        ) as f32;
        let compact_rail_height = sws_protocol::workspace::workspace_region_height(
            work_height as u32,
            false,
            sws::ShellPresentation::Home,
            1000,
        ) as f32;
        let drawer_height = (work_height - compact_rail_height).max(1.0);
        let drawer_width = width;
        let drawer_content_width = (drawer_width - drawer_padding * 2.0).max(1.0);
        let drawer_lip_height = sws_protocol::workspace::DRAWER_SHEET_LIP_HEIGHT as f32;
        let drawer_top = if !shell_state_ready || presentation == sws::ShellPresentation::Workspace
        {
            work_height
        } else if presentation == sws::ShellPresentation::Home {
            compact_rail_height
        } else if shell_layout.tablet_mode {
            workspace_total_height
        } else {
            work_height - drawer_lip_height
        };
        let drawer_header_height = 44.0;
        let drawer_body_height = (drawer_height - drawer_lip_height).max(1.0);
        let drawer_body_top_padding = if height < 760.0 { 6.0 } else { 10.0 };
        let drawer_body_bottom_padding = drawer_padding;
        let drawer_content_height =
            (drawer_body_height - drawer_body_top_padding - drawer_body_bottom_padding).max(1.0);
        let grid_height =
            (drawer_content_height - drawer_header_height - HOME_DRAWER_RESULTS_SPACING).max(1.0);
        let cell_app = self.clone();
        let list_app = self.clone();
        let key_app = self.clone();
        let palette = ColorPalette::dark();
        let search_palette = ColorPalette::light();
        let application_count = self.home_filtered_applications.get().len();
        let searching = !self.home_query.get().trim().is_empty();
        let app_grid_width = (columns as f32 * HOME_GRID_CELL_WIDTH
            + columns.saturating_sub(1) as f32 * HOME_GRID_COLUMN_SPACING)
            .min(drawer_content_width);
        let grid = GridView::new(
            self.home_filtered_applications.clone(),
            self.home_selected.clone(),
            columns,
            row_height,
            move |index, application, selected| {
                cell_app.home_application_cell(index, application, selected, row_height)
            },
        )
        .spacing(HOME_GRID_COLUMN_SPACING)
        .row_spacing(14.0)
        .minimum_cell_width(96.0)
        .frame(app_grid_width, grid_height);
        let centered_grid = hstack! {
            Spacer::new(),
            grid,
            Spacer::new(),
        }
        .frame(drawer_content_width, grid_height)
        .alignment(Alignment::Top);
        let list = ListView::new(
            self.home_filtered_applications.clone(),
            self.home_selected.clone(),
            HOME_SEARCH_RESULT_ROW_HEIGHT,
            move |index, application, selected| {
                list_app.home_application_list_row(index, application, selected)
            },
        )
        .frame(drawer_content_width, grid_height);
        let populated_results = if searching {
            Either::A(list)
        } else {
            Either::B(centered_grid)
        };
        let results = if application_count == 0 {
            Either::A(
                vstack! {
                    IconView::new(Icon::Search)
                        .size(IconSize::Large)
                        .color(palette.text_secondary()),
                    Text::new(if searching {
                        "No applications found"
                    } else {
                        "No applications available"
                    })
                    .font_size(14.0)
                    .color(palette.text_secondary()),
                }
                .spacing(8.0)
                .alignment(Alignment::Center)
                .frame(drawer_content_width, grid_height),
            )
        } else {
            Either::B(populated_results)
        };

        let submit_app = self.clone();
        let cancel_query = self.home_query.clone();
        let cancel_focus = self.home_search_focused.clone();
        let cancel_app = self.clone();
        let empty_focus = self.home_search_focused.clone();
        let search = TextField::new(self.home_query.clone())
            .autofocus(
                application_drawer_accepts_keyboard(presentation) && self.home_search_focused.get(),
            )
            .placeholder("Search applications")
            .font_size(14.0)
            .padding(8.0)
            .background_color(search_palette.window_background().with_opacity(0.86))
            .border_color(search_palette.divider().with_opacity(0.65))
            .focused_border_color(palette.primary().with_opacity(0.9))
            .text_color(search_palette.text())
            .on_submit(move || submit_app.launch_selected_home_application())
            .on_cancel(move || {
                if cancel_query.get().trim().is_empty() {
                    cancel_app.dismiss_shell_depth();
                } else {
                    cancel_query.set(String::new());
                    cancel_focus.set(false);
                }
            })
            .on_empty(move || empty_focus.set(false))
            .frame(
                (width * 0.32).clamp(if width < 720.0 { 180.0 } else { 280.0 }, 460.0),
                36.0,
            );
        let drawer_header_side_width = if width < 520.0 {
            52.0
        } else if width < 720.0 {
            84.0
        } else {
            112.0
        };
        let application_count_label = if application_count == 1 {
            String::from("1 app")
        } else {
            format!("{} apps", application_count)
        };
        let drawer_toggle_app = self.clone();
        let drawer_lip = hstack! {
            Spacer::new(),
            Spacer::new()
                .frame(44.0, 4.0)
                .background(palette.text().with_opacity(0.52))
                .clip_radius(2.0),
            Spacer::new(),
        }
        .alignment(Alignment::Center)
        .frame(drawer_width, drawer_lip_height)
        .on_click(move || {
            if drawer_toggle_app.shell_presentation() == sws::ShellPresentation::Home {
                drawer_toggle_app.lower_application_drawer();
            } else {
                drawer_toggle_app.show_application_drawer();
            }
        });

        let drawer_body = vstack! {
            hstack! {
                Text::new("Applications")
                    .font_size(16.0)
                    .color(palette.text())
                    .alignment(Alignment::Leading)
                    .frame(drawer_header_side_width, drawer_header_height),
                Spacer::new(),
                search,
                Spacer::new(),
                Text::new(application_count_label)
                    .font_size(11.0)
                    .color(palette.text_secondary())
                    .alignment(Alignment::Trailing)
                    .frame(drawer_header_side_width, drawer_header_height),
            }
            .alignment(Alignment::Center)
            .frame(drawer_content_width, drawer_header_height),
            results,
        }
        .spacing(HOME_DRAWER_RESULTS_SPACING)
        .alignment(Alignment::TopLeading)
        .frame(drawer_content_width, drawer_content_height)
        .padding_insets(EdgeInsets {
            top: drawer_body_top_padding,
            left: drawer_padding,
            bottom: drawer_body_bottom_padding,
            right: drawer_padding,
        })
        .frame(drawer_width, drawer_body_height);
        let drawer = vstack! {
            drawer_lip,
            drawer_body,
        }
        .alignment(Alignment::TopLeading)
        .frame(drawer_width, drawer_height);
        let drawer_layer = vstack! {
            Spacer::new().frame(width, drawer_top),
            drawer,
        }
        .alignment(Alignment::TopLeading)
        .frame(width, work_height)
        .clip_radius(0.0);

        drawer_layer
            .padding_insets(EdgeInsets {
                top: bar_height,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            })
            .frame(width, height)
            .background(if shell_visible {
                HOME_BACKDROP_TINT
            } else {
                HOME_BACKDROP_TINT.with_opacity(0.0)
            })
            .on_key(move |event| key_app.handle_home_key(event))
    }
}

impl Application for ShellApp {
    fn on_focus_changed(&mut self, window_id: u32, app_name: &str, menu_titles: &str) {
        status_bar_debug!(
            "[StatusBar] on_focus_changed: window_id={}, app_name={}, menu_titles={}",
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
        } else if ctx.scene_key.as_str() == HOME_SCENE_KEY {
            self.home_window_id.set(ctx.platform_window_id as u32);
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
        status_bar_debug!(
            "[StatusBar] on_active_app_changed: window_id={}, app_name={}, menu_titles={}",
            window_id,
            app_name,
            menu_titles
        );
        let (resolved_menu_titles, menu_changed) = self.resolve_menu_titles(window_id, menu_titles);
        self.update_menu_for_app(window_id, app_name, &resolved_menu_titles, menu_changed);
    }

    fn on_window_resize(&mut self, ctx: &WindowContext, width: u32, height: u32) {
        if ctx.scene_key.as_str() == HOME_SCENE_KEY {
            self.screen_width.set(width as f32);
            self.screen_height.set(height as f32);
            return;
        }
        if ctx.scene_key.as_str() != "main" {
            return;
        }
        println!("[StatusBar] on_resize: width={}, height={}", width, height);
        self.screen_width.set(width as f32);
        self.open_menu_index.set(None);
        self.update_workarea_from_screen_query(width, self.shell_layout.get());
    }

    fn on_window_sync(&mut self, ctx: &WindowContext, window: &mut dyn PlatformWindow) {
        if ctx.scene_key.as_str() == CONTROL_CENTER_SCENE_KEY {
            let desired = self.control_center_size.get();
            if status_bar_resize_needed(window.managed_size(), desired) {
                let _ = window.resize_managed(desired.width as u32, desired.height as u32);
            }
            return;
        }
        if ctx.scene_key.as_str() == HOME_SCENE_KEY {
            let desired = Size::new(self.screen_width.get(), self.screen_height.get());
            if status_bar_resize_needed(window.size(), desired)
                && let Err(error) = window.resize(
                    desired.width.max(1.0) as u32,
                    desired.height.max(1.0) as u32,
                )
            {
                println!("[Shell] Failed to resize Home surface: {}", error);
            }
            return;
        }
        if ctx.scene_key.as_str() != "main" {
            return;
        }
        let desired = self
            .shell_layout
            .get()
            .status_bar_window_size(self.screen_width.get());
        if status_bar_resize_needed(window.size(), desired)
            && let Err(error) = window.resize(desired.width.max(1.0) as u32, desired.height as u32)
        {
            println!("[StatusBar] Failed to resize shell surface: {}", error);
        }
    }

    fn on_screen_size_changed(&mut self, width: u32, height: u32) -> Option<Size> {
        println!("[StatusBar] on_screen_size_changed: {}x{}", width, height);
        self.screen_width.set(width as f32);
        self.screen_height.set(height as f32);
        self.open_menu_index.set(None);
        let layout = self.shell_layout.get();
        self.update_workarea(width, height, layout);
        Some(layout.status_bar_window_size(width as f32))
    }

    fn scenes(&self) -> impl Scene {
        let clock = self.clock.get();
        let screen_width = self.screen_width.get();
        let screen_height = self.screen_height.get();
        let shell_layout = self.shell_layout.get();
        let shell_presentation = self.shell_presentation();
        let shell_navigation = shell_presentation != sws::ShellPresentation::Workspace;
        let _menu_bar = self.menu_bar.get();
        let menu_tree = self.menu_tree.get();
        // println!(
        //     "[StatusBar] scenes() called: menu_tree has {} items",
        //     menu_tree.items.len()
        // );
        let active_window_id = self.active_window_id.get();

        let hours = clock / 3600;
        let mins = (clock / 60) % 60;
        let status_snapshot = self.status_snapshot.get();
        let clock_label = status_snapshot.clock_label(hours as u8, mins as u8);
        // The desktop top bar intentionally uses a light material. Its status
        // labels and Tabler icons use ScarletUI's matching dark foreground.
        let status_bar_palette = if shell_navigation {
            ColorPalette::dark()
        } else {
            ColorPalette::light()
        };
        let status_bar_background = if shell_navigation {
            scarlet_ui::color::Color::TRANSPARENT
        } else {
            status_bar_palette.surface_variant()
        };
        let status_tokens = StatusItemTokens::for_layout(shell_layout);
        let status_presentation = if shell_layout.tablet_mode {
            StatusPresentation::Touch
        } else {
            StatusPresentation::Compact
        };
        if !self.control_center_open.get()
            && let Some(volume) = status_snapshot.audio_volume_percent
        {
            set_state_if_changed(&self.control_center_volume, volume as f32);
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
        let control_center_presentation = if shell_layout.tablet_mode {
            ControlCenterPresentation::TabletSheet
        } else {
            ControlCenterPresentation::LaptopPopover
        };
        let control_center_metrics = ControlCenterMetrics::resolve(
            control_center_presentation,
            control_center_snapshot.audio.outputs.len(),
        );
        let control_center_size = control_center_metrics.body_size();
        set_state_if_changed(&self.control_center_size, control_center_size);
        let (control_center_body_x, control_center_body_y) = control_center_body_position(
            screen_width,
            screen_height,
            shell_layout.popup_y(),
            control_center_size,
            control_center_presentation,
        );
        let home_button =
            build_home_button(self.workspace_state.get(), self.workspace_commands.clone());
        let visible_menu_items = status_bar_menu_items(&menu_tree, shell_presentation);
        let status_foreground = shell_navigation.then(|| status_bar_palette.text());
        let menu_controls = build_menu_bar_view(
            &visible_menu_items,
            active_window_id,
            self.open_menu_index.clone(),
            shell_layout,
            status_foreground,
        );
        let leading_controls = hstack! {
            home_button,
            menu_controls,
        }
        .spacing(LEADING_CONTROLS_SPACING)
        .alignment(Alignment::Center);
        let trailing_controls = hstack! {
            build_status_cluster(
                status_snapshot.clone(),
                status_presentation,
                status_tokens,
                self.control_center_open.clone(),
                status_foreground,
            ),
            build_passive_clock(clock_label, status_tokens, status_foreground),
        }
        .spacing(status_tokens.spacing)
        .alignment(Alignment::Center);

        (
            WindowGroup::new(
                "main",
                Window::new(
                    "Scarlet Shell",
                    hstack! {
                        leading_controls,
                        Spacer::new(),
                        trailing_controls,
                    }
                    .spacing(status_tokens.spacing)
                    .alignment(Alignment::Center)
                    .padding(status_tokens.bar_padding),
                )
                .app_id("org.scarlet-os.desktop.shell")
                .decorated(false)
                .background_color(status_bar_background)
                // The shell changes between an opaque workspace material and
                // per-pixel transparency without recreating this surface.
                // Keep alpha composition enabled from the first frame.
                .opaque(false)
                .window_type(scarlet_ui::views::window_type::TASKBAR)
                .active_on_focus(false)
                .resizable(false)
                .movable(false)
                .size(shell_layout.status_bar_window_size(screen_width)),
            ),
            Window::new(
                "Control Center",
                build_control_center_view(
                    control_center_presentation,
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
            Window::new("Home", self.home_content())
                .scene_key(HOME_SCENE_KEY)
                .open_at_launch(false)
                .app_id("org.scarlet-os.desktop.shell.home")
                .decorated(false)
                .background_color(scarlet_ui::color::Color::TRANSPARENT)
                .opaque(false)
                .window_type(sws_protocol::window_types::SHELL_BACKGROUND)
                .focus_on_create(false)
                .active_on_focus(false)
                .resizable(false)
                .movable(false)
                .placement(WindowPlacement::At { x: 0, y: 0 })
                .size(Size::new(screen_width, screen_height)),
        )
    }

    fn exit_when_all_windows_closed(&self) -> bool {
        false
    }

    fn init(&mut self) {
        println!("[Shell] Initializing Scarlet workspace shell");
        // Pre-create the full-output Home surface at its final geometry. SWS
        // keeps it workspace-invisible until Home/Overview is selected, so the
        // first reveal is only a visibility change and never exposes a
        // create-then-move or create-then-resize frame.
        open_window(HOME_SCENE_KEY);
        // Screen size will be obtained by sws_client in main()
        std::thread::spawn(background::run);
        self.start_background_tasks();
        let home_applications = self.home_applications.clone();
        std::thread::spawn(move || refresh_home_catalog(home_applications));
        let filter_applications = self.home_applications.clone();
        let filter_query = self.home_query.clone();
        let filtered_applications = self.home_filtered_applications.clone();
        let filtered_selection = self.home_selected.clone();
        let filtered_hover = self.home_hovered.clone();
        std::thread::spawn(move || {
            maintain_home_filter(
                filter_applications,
                filter_query,
                filtered_applications,
                filtered_selection,
                filtered_hover,
            )
        });
        let workspace_state = self.workspace_state.clone();
        let workspace_commands = self.workspace_commands.clone();
        std::thread::spawn(move || {
            maintain_workspace_shell_role(workspace_state, workspace_commands)
        });
    }
}

impl ShellApp {
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
            "[StatusBar] Failed to query screen size for workarea update (fallback_width={}, status_bar_height={})",
            fallback_width,
            layout.status_bar_height()
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
                    println!("[StatusBar] Failed to connect to SWS for menu popup after retries");
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
                                menu_bar_popup_x(&menu_tree_value.items, index, layout)
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
                                                "[StatusBar] Failed to create {} popup: {:?}",
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
        status_bar_debug!(
            "[StatusBar] update_menu_for_app: window_id={}, app_name={}, menu_titles={}",
            window_id,
            app_name,
            menu_titles
        );

        if self.popup_surface_id.get() == Some(window_id) {
            status_bar_debug!("[StatusBar] Skipping popup surface {}", window_id);
            return;
        }

        if matches!(
            app_name,
            "TaskBar" | "StatusBar" | "Scarlet Shell" | "Home" | "Menu"
        ) {
            status_bar_debug!("[StatusBar] Skipping menu update for {}", app_name);
            return;
        }

        if app_name.is_empty() {
            if self.active_window_id.get() == 0 {
                return;
            }
            status_bar_debug!("[StatusBar] No active application, showing default menu");
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
        // intentionally equivalent for StatusBar, so rebuilding the full menu
        // tree twice only adds latency and an avoidable redraw.
        if self.active_window_id.get() == window_id && !menu_changed {
            return;
        }

        let tree = build_menu_tree(app_name, menu_titles);
        status_bar_debug!(
            "[StatusBar] Built menu tree with {} items",
            tree.items.len()
        );
        self.menu_bar.set(menu_bar_from_tree(&tree));
        self.menu_tree.set(tree);
        self.active_window_id.set(window_id);
        self.open_menu_index.set(None);
    }
}

fn main() {
    println!("[Shell] Starting Scarlet workspace shell");

    // Get screen size from SWS before creating the app
    let (screen_width, screen_height, shell_layout) = match connect_sws_with_screen_size_retry() {
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
            (logical_width as f32, logical_height as f32, layout)
        }
        Err(()) => {
            println!(
                "[StatusBar] Failed to connect to SWS after retries, using default screen width 1920"
            );
            (1920.0, 1080.0, ShellLayout::from_tablet_mode(None))
        }
    };

    let mut app = ShellApp::new(shell_layout);

    // Update screen_width state with actual screen size
    app.screen_width.update(|w| *w = screen_width);
    app.screen_height.update(|h| *h = screen_height);

    match app.run() {
        Ok(_) => {
            println!("[Shell] Application exited successfully");
        }
        Err(e) => {
            println!("[Shell] Application error: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ControlCenterPresentation, HOME_BUTTON_WIDTH, HomeApplication, LEADING_CONTROLS_SPACING,
        MENU_BAR_OUTER_PADDING, MenuTree, OVERVIEW_NAVIGATION_ROWS, OVERVIEW_SEPARATOR_HEIGHT,
        OVERVIEW_SYSTEM_ROWS, OVERVIEW_VERTICAL_PADDING, OverviewGeometry, ShellLayout,
        StatusItemId, StatusItemTokens, StatusPresentation, StatusProviderSnapshot, TaskMenuEntry,
        TaskMenuItem, WindowSnapshot, WorkspaceCommand, application_drawer_accepts_keyboard,
        build_window_model, control_center_body_position, filter_home_applications,
        home_grid_columns, menu_bar_item_width, menu_bar_popup_x, next_home_selection_index,
        overview_app_menu_indices, overview_page_bounds, overview_page_capacity,
        overview_page_count, overview_window_status, passive_clock_control, scale_u32,
        set_state_if_changed, status_bar_menu_items, status_bar_resize_needed, status_item_label,
        volume_status_icon, workspace_transaction_for_command,
    };
    use alloc::string::String;
    use alloc::sync::Arc;
    use alloc::vec;
    use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use scarlet_ui::element::{ElementRenderObject, LayoutConstraints};
    use scarlet_ui::geometry::Size;
    use scarlet_ui::views::menu::MenuRenderObject;
    use scarlet_ui::views::{MenuAction, MenuItemContent};
    use scarlet_ui::{Icon, KeyCode, Listenable, State, StateId};
    use sws_client as sws;
    use sws_protocol::window_types;

    fn workspace_state(workspaces: Vec<sws::WorkspaceSnapshot>) -> sws::WorkspaceState {
        sws::WorkspaceState {
            generation: 7,
            active_workspace: workspaces[0].id,
            normal_workspace: workspaces[0].id,
            presentation: sws::ShellPresentation::Workspace,
            workspaces,
        }
    }

    #[test]
    fn unchanged_render_derived_state_does_not_notify_or_rebuild() {
        let state = State::new(StateId::new(90_001), 42u32);
        let notifications = Arc::new(AtomicUsize::new(0));
        let observed = notifications.clone();
        let subscription = state.subscribe_any(Arc::new(move || {
            observed.fetch_add(1, Ordering::Relaxed);
        }));

        assert!(!set_state_if_changed(&state, 42));
        assert_eq!(notifications.load(Ordering::Relaxed), 0);
        assert!(set_state_if_changed(&state, 43));
        assert_eq!(notifications.load(Ordering::Relaxed), 1);
        assert!(state.unsubscribe(subscription));
    }

    fn single_workspace(id: u32, window_id: u32) -> sws::WorkspaceSnapshot {
        sws::WorkspaceSnapshot {
            id,
            window_ids: vec![window_id],
            tablet_layout: sws::TabletLayout::Single { window_id },
        }
    }

    #[test]
    fn home_command_changes_only_presentation() {
        let state = workspace_state(vec![single_workspace(1, 10)]);
        let transaction =
            workspace_transaction_for_command(&state, WorkspaceCommand::ShowHome).unwrap();
        assert_eq!(transaction.base_generation, state.generation);
        assert_eq!(transaction.presentation, sws::ShellPresentation::Home);
        assert_eq!(transaction.workspaces, state.workspaces);
    }

    #[test]
    fn application_drawer_owns_keyboard_only_while_open() {
        assert!(!application_drawer_accepts_keyboard(
            sws::ShellPresentation::Workspace
        ));
        assert!(!application_drawer_accepts_keyboard(
            sws::ShellPresentation::Overview
        ));
        assert!(application_drawer_accepts_keyboard(
            sws::ShellPresentation::Home
        ));
    }

    #[test]
    fn cycle_command_stops_at_edges_and_returns_to_workspace_presentation() {
        let mut state = workspace_state(vec![single_workspace(1, 10), single_workspace(2, 20)]);
        state.presentation = sws::ShellPresentation::Overview;
        assert!(workspace_transaction_for_command(&state, WorkspaceCommand::Cycle(-1)).is_none());

        let next = workspace_transaction_for_command(&state, WorkspaceCommand::Cycle(1)).unwrap();
        assert_eq!(next.active_workspace, 2);
        assert_eq!(next.presentation, sws::ShellPresentation::Workspace);

        state.active_workspace = 2;
        assert!(workspace_transaction_for_command(&state, WorkspaceCommand::Cycle(1)).is_none());
    }

    #[test]
    fn overview_dismissal_uses_the_committed_empty_workspace_selection() {
        let mut state = workspace_state(vec![
            single_workspace(1, 10),
            sws::WorkspaceSnapshot {
                id: 2,
                window_ids: vec![],
                tablet_layout: sws::TabletLayout::Empty,
            },
        ]);
        state.active_workspace = 2;
        state.normal_workspace = 2;
        state.presentation = sws::ShellPresentation::Overview;

        let transaction =
            workspace_transaction_for_command(&state, WorkspaceCommand::ReturnToWorkspace).unwrap();
        assert_eq!(transaction.active_workspace, 2);
        assert_eq!(transaction.presentation, sws::ShellPresentation::Workspace);
    }

    #[test]
    fn split_command_merges_the_adjacent_workspace_atomically() {
        let state = workspace_state(vec![single_workspace(1, 10), single_workspace(2, 20)]);
        let transaction =
            workspace_transaction_for_command(&state, WorkspaceCommand::ToggleSplit).unwrap();
        assert_eq!(transaction.workspaces.len(), 2);
        assert_eq!(transaction.workspaces[0].window_ids, vec![10, 20]);
        assert!(transaction.workspaces[1].window_ids.is_empty());
        assert_eq!(
            transaction.workspaces[0].tablet_layout,
            sws::TabletLayout::Split {
                axis: sws::SplitAxis::Horizontal,
                first_window_id: 10,
                second_window_id: 20,
                ratio_milli: 500,
            }
        );
    }

    #[test]
    fn unsplit_parks_the_second_scene_without_losing_membership() {
        let state = workspace_state(vec![sws::WorkspaceSnapshot {
            id: 1,
            window_ids: vec![10, 20],
            tablet_layout: sws::TabletLayout::Split {
                axis: sws::SplitAxis::Horizontal,
                first_window_id: 10,
                second_window_id: 20,
                ratio_milli: 640,
            },
        }]);
        let transaction =
            workspace_transaction_for_command(&state, WorkspaceCommand::ToggleSplit).unwrap();
        assert_eq!(transaction.workspaces[0].window_ids, vec![10, 20]);
        assert_eq!(
            transaction.workspaces[0].tablet_layout,
            sws::TabletLayout::Single { window_id: 10 }
        );
    }

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
        assert_eq!(ShellLayout::from_tablet_mode(None).status_bar_height(), 32);
        assert_eq!(
            ShellLayout::from_tablet_mode(Some(false)).status_bar_height(),
            32
        );
    }

    #[test]
    fn tablet_mode_uses_a_larger_touch_status_bar() {
        assert_eq!(
            ShellLayout::from_tablet_mode(Some(true)).status_bar_height(),
            40
        );
        assert_eq!(ShellLayout::from_tablet_mode(Some(true)).popup_y(), 40);
    }

    #[test]
    fn physical_workarea_reserves_scaled_shell_height() {
        let layout = ShellLayout::from_tablet_mode(Some(true));
        let workarea = layout.physical_workarea(2880, 1800, 1500);
        assert_eq!(workarea.x, 0);
        assert_eq!(workarea.y, scale_u32(40, 1500) as i32);
        assert_eq!(workarea.width, 2880);
        assert_eq!(workarea.height, 1800 - scale_u32(40, 1500));
    }

    #[test]
    fn status_bar_resize_is_deduplicated_after_layout_sync() {
        let laptop = ShellLayout::from_tablet_mode(Some(false)).status_bar_window_size(1920.0);
        let tablet = ShellLayout::from_tablet_mode(Some(true)).status_bar_window_size(1920.0);
        assert!(!status_bar_resize_needed(laptop, laptop));
        assert!(status_bar_resize_needed(laptop, tablet));
    }

    #[test]
    fn first_app_menu_popup_starts_after_compact_home_button() {
        assert_eq!(
            menu_bar_popup_x(&[], 0, ShellLayout::from_tablet_mode(Some(false))),
            MENU_BAR_OUTER_PADDING + HOME_BUTTON_WIDTH + LEADING_CONTROLS_SPACING
        );
    }

    #[test]
    fn tablet_keeps_separate_app_menus_with_larger_touch_widths() {
        let laptop = ShellLayout::from_tablet_mode(Some(false));
        let tablet = ShellLayout::from_tablet_mode(Some(true));
        assert!(menu_bar_item_width("File", tablet) > menu_bar_item_width("File", laptop));
    }

    #[test]
    fn home_search_prefers_name_prefixes_before_identifier_matches() {
        let applications = vec![
            HomeApplication {
                app_id: String::from("org.example.search-tool"),
                name: String::from("Utilities"),
                icon: String::new(),
            },
            HomeApplication {
                app_id: String::from("org.example.editor"),
                name: String::from("Search Notes"),
                icon: String::new(),
            },
            HomeApplication {
                app_id: String::from("org.example.search"),
                name: String::from("Search"),
                icon: String::new(),
            },
        ];

        let filtered = filter_home_applications(&applications, "search");
        assert_eq!(
            filtered
                .iter()
                .map(|application| application.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Search", "Search Notes", "Utilities"]
        );
    }

    #[test]
    fn home_grid_uses_four_direction_selection_stride() {
        let columns = home_grid_columns(1280.0);
        assert_eq!(columns, 6);
        assert_eq!(home_grid_columns(2560.0), 6);
        assert_eq!(home_grid_columns(480.0), 3);
        assert_eq!(home_grid_columns(320.0), 2);
        assert_eq!(next_home_selection_index(0, 20, columns, KeyCode::Right), 1);
        assert_eq!(
            next_home_selection_index(0, 20, columns, KeyCode::Down),
            columns
        );
        assert_eq!(
            next_home_selection_index(columns, 20, columns, KeyCode::Up),
            0
        );
    }

    #[test]
    fn home_search_list_uses_single_row_selection_stride() {
        assert_eq!(next_home_selection_index(2, 6, 1, KeyCode::Down), 3);
        assert_eq!(next_home_selection_index(2, 6, 1, KeyCode::Up), 1);
    }

    #[test]
    fn control_center_shadow_surface_keeps_the_requested_outer_margin() {
        let outsets = scarlet_ui::ElevationRole::Floating.paint_outsets();
        let (body_x, body_y) = control_center_body_position(
            1920.0,
            1080.0,
            32,
            Size::new(304.0, 300.0),
            ControlCenterPresentation::LaptopPopover,
        );

        assert_eq!(body_x as f32 - outsets.left, 1920.0 - 8.0 - 324.0);
        assert_eq!(body_x as f32 + 304.0 + outsets.right, 1920.0 - 8.0);
        assert_eq!(body_y as f32 - outsets.top, 32.0 + 8.0);
    }

    #[test]
    fn tablet_mode_uses_larger_status_targets_than_laptop() {
        let laptop = StatusItemTokens::for_layout(ShellLayout::from_tablet_mode(Some(false)));
        let tablet = StatusItemTokens::for_layout(ShellLayout::from_tablet_mode(Some(true)));
        assert_eq!(laptop.logical_height, 24.0);
        assert_eq!(laptop.font_size, 13.0);
        assert!(laptop.font_size * 1.2 + laptop.horizontal_padding * 2.0 <= laptop.logical_height);
        assert!(
            laptop.logical_height + laptop.bar_padding * 2.0
                <= ShellLayout::LAPTOP_STATUS_BAR_HEIGHT as f32
        );
        assert!(tablet.logical_height > laptop.logical_height);
        assert!(tablet.font_size > laptop.font_size);
        assert!(
            tablet.logical_height + tablet.bar_padding * 2.0
                <= ShellLayout::TABLET_STATUS_BAR_HEIGHT as f32
        );
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
            let clock = passive_clock_control("12:34", tokens, None);
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
        assert!(popup_height <= (800 - tablet.status_bar_height() - 8) as f32);
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
    fn status_bar_hides_application_context_during_shell_navigation() {
        let tree = MenuTree {
            items: vec![
                TaskMenuItem {
                    id: String::from("system_scarlet"),
                    title: String::from("Scarlet"),
                    enabled: true,
                    shortcut: None,
                    children: vec![],
                },
                TaskMenuItem {
                    id: String::from("system_app"),
                    title: String::from("Active App"),
                    enabled: true,
                    shortcut: None,
                    children: vec![],
                },
                TaskMenuItem {
                    id: String::from("file"),
                    title: String::from("File"),
                    enabled: true,
                    shortcut: None,
                    children: vec![],
                },
            ],
        };

        assert_eq!(
            status_bar_menu_items(&tree, sws::ShellPresentation::Workspace).len(),
            3
        );
        for presentation in [
            sws::ShellPresentation::Overview,
            sws::ShellPresentation::Home,
        ] {
            let items = status_bar_menu_items(&tree, presentation);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].id, "system_scarlet");
        }
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
