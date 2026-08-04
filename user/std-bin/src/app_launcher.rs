//! Resident Scarlet Desktop application launcher.
//!
//! The launcher process is started with the desktop session and keeps its
//! catalog and icon cache warm while no window is shown.  The desktop shell
//! asks it to show the window through sbus, so opening the launcher does not
//! fork a new application or repeat catalog initialization.

use std::process::ExitCode;
use std::string::String;
use std::thread;
use std::time::Duration;
use std::vec::Vec;

use sbus::{Argument, Message};
use sbus_client::Connection as SbusConnection;
use scarlet_desktop_config::{
    DESKTOP_FILE_MANAGER_BUS_NAME, DESKTOP_FILE_MANAGER_INTERFACE,
    DESKTOP_FILE_MANAGER_OBJECT_PATH, DESKTOP_FILE_MANAGER_SHOW_METHOD, DESKTOP_FILES_APP_ID,
    DESKTOP_LAUNCHER_BUS_NAME, DESKTOP_LAUNCHER_INTERFACE, DESKTOP_LAUNCHER_OBJECT_PATH,
    DESKTOP_LAUNCHER_SHOW_METHOD, DESKTOP_STEMD_BUS_NAME, DESKTOP_STEMD_INTERFACE,
    DESKTOP_STEMD_LAUNCH_OR_FOCUS_METHOD, DESKTOP_STEMD_LIST_APPLICATIONS_METHOD,
    DESKTOP_STEMD_OBJECT_PATH,
};
use scarlet_ui::prelude::*;
use scarlet_ui::{
    Alignment, Color, ColorPalette, GridView, Icon, IconSize, IconView, KeyCode, PlatformWindow,
    Spacer, Window, WindowPlacement, dismiss_window, hstack, vstack,
};
use scarlet_ui_macros::View;
use sws_protocol::window_types;

const APP_ID: &str = "org.scarlet-os.desktop.launcher";
const FILE_MANAGER_APP_ID: &str = DESKTOP_FILES_APP_ID;
const APP_TITLE: &str = "Applications";
const WINDOW_WIDTH: f32 = 820.0;
const WINDOW_HEIGHT: f32 = 540.0;
const GRID_COLUMNS: usize = 6;
const GRID_ROW_HEIGHT: f32 = 112.0;
const GRID_CELL_WIDTH: f32 = 116.0;
const SEARCH_ROW_HEIGHT: f32 = 72.0;
const SEARCH_VISIBLE_ROWS: usize = 5;
const SEARCH_LIST_HEIGHT: f32 = SEARCH_ROW_HEIGHT * SEARCH_VISIBLE_ROWS as f32;
const SERVICE_RETRY_DELAY: Duration = Duration::from_millis(100);
const CATALOG_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq, Eq)]
struct ApplicationEntry {
    app_id: String,
    name: String,
    icon: String,
}

#[derive(View, Clone)]
struct LauncherApp {
    applications: State<Vec<ApplicationEntry>>,
    filtered_applications: State<Vec<ApplicationEntry>>,
    catalog_revision: State<u64>,
    last_catalog_revision: State<u64>,
    query: State<String>,
    search_focused: State<bool>,
    last_query: State<String>,
    selected: State<Option<usize>>,
    hovered: State<Option<usize>>,
    status: State<String>,
    window_id: State<u32>,
    focus_ready: State<bool>,
}

impl LauncherApp {
    fn new() -> Self {
        let app = Self::default();
        app.status.set(String::from("Loading applications..."));
        app
    }

    fn launch_selected(&self) {
        let Some(index) = self.selected.get() else {
            return;
        };
        let Some(application) = self.filtered_applications.get().get(index).cloned() else {
            return;
        };
        self.launch_application(application);
    }

    fn launch_application(&self, application: ApplicationEntry) {
        if application.app_id == FILE_MANAGER_APP_ID {
            if show_file_manager() {
                dismiss_window("main");
            } else {
                self.status.set(String::from("Could not open Files"));
            }
            return;
        }

        match SbusConnection::connect().and_then(|mut connection| {
            connection.call_method(
                DESKTOP_STEMD_BUS_NAME,
                DESKTOP_STEMD_OBJECT_PATH,
                DESKTOP_STEMD_INTERFACE,
                DESKTOP_STEMD_LAUNCH_OR_FOCUS_METHOD,
                vec![
                    Argument::String(application.app_id.clone()),
                    Argument::Boolean(true),
                ],
            )
        }) {
            Ok(_) => dismiss_window("main"),
            Err(error) => self
                .status
                .set(format!("Could not launch {}: {error:?}", application.name)),
        }
    }

    fn move_selection(&self, keycode: KeyCode) {
        let applications = self.filtered_applications.get();
        if applications.is_empty() {
            self.selected.set(None);
            return;
        }

        let current = self
            .selected
            .get()
            .unwrap_or(0)
            .min(applications.len().saturating_sub(1));
        let last = applications.len().saturating_sub(1);
        let columns = if self.query.get().trim().is_empty() {
            GRID_COLUMNS
        } else {
            1
        };
        let next = match keycode {
            KeyCode::Left => current.saturating_sub(1),
            KeyCode::Right => current.saturating_add(1).min(last),
            KeyCode::Up => current.saturating_sub(columns),
            KeyCode::Down => current.saturating_add(columns).min(last),
            _ => current,
        };
        self.selected.set(Some(next));
        self.hovered.set(None);
    }

    fn handle_key(&self, event: KeyEvent) -> bool {
        match event {
            KeyEvent::Char { c } if !c.is_control() => {
                self.search_focused.set(true);
                self.query.update(|query| query.push(c));
                true
            }
            KeyEvent::Pressed { keycode, .. } => match keycode {
                KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                    self.move_selection(keycode);
                    true
                }
                KeyCode::Enter => {
                    self.launch_selected();
                    true
                }
                KeyCode::Escape => {
                    dismiss_window("main");
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn sync_filtered_applications(&mut self) {
        let query = self.query.get();
        let query_changed = query != self.last_query.get();
        let catalog_revision = self.catalog_revision.get();
        let catalog_changed = catalog_revision != self.last_catalog_revision.get();
        let selection_missing =
            self.selected.get().is_none() && !self.filtered_applications.get().is_empty();
        if !query_changed && !catalog_changed && !selection_missing {
            return;
        }

        let filtered = filter_applications(&self.applications.get(), &query);
        let changed = filtered != self.filtered_applications.get();

        if changed {
            self.filtered_applications.set(filtered.clone());
        }

        if query_changed || catalog_changed || selection_missing {
            let selected = if filtered.is_empty() {
                None
            } else if query_changed || selection_missing {
                Some(0)
            } else {
                self.selected
                    .get()
                    .filter(|index| *index < filtered.len())
                    .or(Some(0))
            };
            self.selected.set(selected);
            self.hovered.set(None);
            self.last_query.set(query);
            self.last_catalog_revision.set(catalog_revision);
        }
    }

    fn application_cell(
        &self,
        index: usize,
        application: ApplicationEntry,
        selected: Option<usize>,
    ) -> impl View + Clone + use<> {
        let palette = ColorPalette::default();
        let is_selected = selected == Some(index);
        let is_hovered = self.hovered.get() == Some(index);
        let background = if is_selected {
            palette.text().with_opacity(0.10)
        } else if is_hovered {
            palette.text().with_opacity(0.045)
        } else {
            Color::TRANSPARENT
        };
        let hover_state = self.hovered.clone();
        let exit_state = hover_state.clone();
        let app = self.clone();
        let icon = icon_for_desktop_name(&application.icon);

        vstack! {
            IconView::new(icon)
                .size(IconSize::ExtraLarge)
                .color(palette.primary()),
            Text::new(application.name.clone())
                .font_size(13.0)
                .alignment(Alignment::Center)
                .frame(f32::INFINITY, 34.0),
        }
        .spacing(4.0)
        .alignment(Alignment::Center)
        .frame(f32::INFINITY, GRID_ROW_HEIGHT)
        .padding(8.0)
        .background(background)
        .clip_radius(12.0)
        .on_hover(move || hover_state.set(Some(index)))
        .on_exit(move || {
            if exit_state.get() == Some(index) {
                exit_state.set(None);
            }
        })
        .on_click(move || app.launch_application(application.clone()))
    }

    fn application_list_row(
        &self,
        index: usize,
        application: ApplicationEntry,
        selected: Option<usize>,
    ) -> impl View + Clone + use<> {
        let palette = ColorPalette::default();
        let is_selected = selected == Some(index);
        let is_hovered = self.hovered.get() == Some(index);
        let background = if is_selected {
            palette.text().with_opacity(0.10)
        } else if is_hovered {
            palette.text().with_opacity(0.045)
        } else {
            Color::TRANSPARENT
        };
        let hover_state = self.hovered.clone();
        let exit_state = hover_state.clone();
        let app = self.clone();
        let icon = icon_for_desktop_name(&application.icon);

        hstack! {
            IconView::new(icon)
                .size(IconSize::Medium)
                .color(palette.primary()),
            hstack! {
                Text::new(application.name.clone()).font_size(15.0),
                Text::new(application.app_id.clone())
                    .font_size(13.0)
                    .color(palette.secondary())
                    .padding_insets(EdgeInsets { top: 0.0, left: 8.0, bottom: 0.0, right: 0.0 }),
            }
            .spacing(8.0),
            Spacer::new(),
            Text::new("Application")
                .font_size(13.0)
                .color(palette.secondary()),
        }
        .spacing(14.0)
        .alignment(Alignment::Center)
        .padding(12.0)
        .frame(f32::INFINITY, SEARCH_ROW_HEIGHT)
        .background(background)
        .clip_radius(10.0)
        .on_hover(move || hover_state.set(Some(index)))
        .on_exit(move || {
            if exit_state.get() == Some(index) {
                exit_state.set(None);
            }
        })
        .on_click(move || app.launch_application(application.clone()))
    }

    fn content(&self) -> impl View + Clone + use<> {
        let grid_app = self.clone();
        let list_app = self.clone();
        let key_app = self.clone();
        let submit_app = self.clone();
        let empty_app = self.clone();
        let palette = ColorPalette::default();
        let searching = !self.query.get().trim().is_empty();
        let section_title = if searching {
            "Search Results"
        } else {
            "All Applications"
        };
        let search = TextField::new(self.query.clone())
            .autofocus(self.search_focused.get())
            .placeholder("Search applications")
            .font_size(16.0)
            .padding(10.0)
            .background_color(palette.background().with_opacity(0.72))
            .border_color(palette.background_tertiary().with_opacity(0.7))
            .focused_border_color(palette.primary().with_opacity(0.8))
            .on_submit(move || submit_app.launch_selected())
            .on_cancel(|| dismiss_window("main"))
            .on_empty(move || empty_app.search_focused.set(false))
            .frame(560.0, 46.0);
        let grid = GridView::new(
            self.filtered_applications.clone(),
            self.selected.clone(),
            GRID_COLUMNS,
            GRID_ROW_HEIGHT,
            move |index, application, selected| {
                grid_app.application_cell(index, application, selected)
            },
        )
        .spacing(12.0)
        .minimum_cell_width(GRID_CELL_WIDTH);
        let list = ListView::new(
            self.filtered_applications.clone(),
            self.selected.clone(),
            SEARCH_ROW_HEIGHT,
            move |index, application, selected| {
                list_app.application_list_row(index, application, selected)
            },
        );
        // Keep the search viewport on an integral number of rows. Without an
        // explicit height, VStack can give the ScrollView more space than the
        // launcher surface has left, so the parent clips a row that the
        // ScrollView still considers visible.
        let results = if searching {
            Either::A(list.frame(f32::INFINITY, SEARCH_LIST_HEIGHT))
        } else {
            Either::B(grid)
        };

        vstack! {
            hstack! {
                IconView::new(Icon::Apps)
                    .size(IconSize::Medium)
                    .color(palette.primary()),
                Text::new(APP_TITLE).font_size(22.0),
                Spacer::new(),
                Text::new("Super + Space")
                    .font_size(11.0)
                    .color(palette.secondary()),
            }
            .spacing(6.0)
            .alignment(Alignment::Center)
            .padding(8.0),
            hstack! {
                IconView::new(Icon::Search)
                    .size(IconSize::Medium)
                    .color(palette.secondary()),
                search,
            }
            .spacing(10.0)
            .alignment(Alignment::Center)
            .padding(8.0),
            hstack! {
                Text::new(section_title)
                    .font_size(12.0)
                    .color(palette.primary()),
                Spacer::new(),
                Text::from_state(self.status.clone())
                    .font_size(11.0)
                    .color(palette.secondary()),
            }
            .alignment(Alignment::Center)
            .padding(8.0),
            results,
        }
        .padding(18.0)
        .frame(WINDOW_WIDTH, WINDOW_HEIGHT)
        .background(palette.window_background().with_opacity(0.94))
        .clip_radius(20.0)
        .on_key(move |event| key_app.handle_key(event))
    }
}

fn show_file_manager() -> bool {
    if request_file_manager_window() {
        return true;
    }

    let _ = SbusConnection::connect().and_then(|mut connection| {
        connection.call_method(
            DESKTOP_STEMD_BUS_NAME,
            DESKTOP_STEMD_OBJECT_PATH,
            DESKTOP_STEMD_INTERFACE,
            DESKTOP_STEMD_LAUNCH_OR_FOCUS_METHOD,
            vec![
                Argument::String(String::from(FILE_MANAGER_APP_ID)),
                Argument::Boolean(true),
            ],
        )
    });

    for _ in 0..20 {
        thread::sleep(Duration::from_millis(50));
        if request_file_manager_window() {
            return true;
        }
    }
    false
}

fn request_file_manager_window() -> bool {
    let Ok(mut connection) = SbusConnection::connect() else {
        return false;
    };
    connection
        .call_method(
            DESKTOP_FILE_MANAGER_BUS_NAME,
            DESKTOP_FILE_MANAGER_OBJECT_PATH,
            DESKTOP_FILE_MANAGER_INTERFACE,
            DESKTOP_FILE_MANAGER_SHOW_METHOD,
            Vec::new(),
        )
        .is_ok()
}

impl Application for LauncherApp {
    fn init(&mut self) {
        // Do not block application startup or the first window request on
        // SBUS. The catalog loader fills the state asynchronously; the grid
        // can be shown immediately and will rebuild when results arrive.
        let loader_app = self.clone();
        thread::spawn(move || catalog_loader(loader_app));
    }

    fn on_window_created(&mut self, _ctx: &WindowContext, window: &mut dyn PlatformWindow) {
        self.window_id.set(window.surface_id());
        // A newly created launcher can first receive a FocusChanged event for
        // the window that was focused before it opened. Do not treat that
        // event as a loss of focus until this window has received its own
        // focus notification.
        self.focus_ready.set(false);
    }

    fn on_focus_changed(&mut self, window_id: u32, _app_name: &str, _menu_titles: &str) {
        let own_window_id = self.window_id.get();
        if own_window_id == 0 {
            return;
        }
        if window_id == own_window_id {
            self.focus_ready.set(true);
        } else if self.focus_ready.get() {
            self.focus_ready.set(false);
            dismiss_window("main");
        }
    }

    fn on_idle(&mut self) {
        self.sync_filtered_applications();
    }

    fn scenes(&self) -> impl Scene {
        Window::new(APP_TITLE, self.content())
            .app_id(APP_ID)
            .size(Size::new(WINDOW_WIDTH, WINDOW_HEIGHT))
            .resizable(false)
            .decorated(false)
            .opaque(false)
            .background_color(Color::TRANSPARENT)
            .window_type(window_types::ALWAYS_ON_TOP)
            .placement(WindowPlacement::Centered)
            .scene_key("main")
            .open_at_launch(false)
    }

    fn exit_when_all_windows_closed(&self) -> bool {
        false
    }
}

fn catalog_loader(app: LauncherApp) {
    let mut previous = app.applications.get();
    let mut loaded_once = !previous.is_empty();

    loop {
        let (applications, status) = load_applications();
        if !loaded_once || applications != previous {
            app.applications.set(applications.clone());
            app.status.set(status);
            app.catalog_revision.update(|revision| {
                *revision = revision.saturating_add(1);
            });
            previous = applications;
            loaded_once = true;
        }
        thread::sleep(CATALOG_REFRESH_INTERVAL);
    }
}

fn filter_applications(applications: &[ApplicationEntry], query: &str) -> Vec<ApplicationEntry> {
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

fn load_applications() -> (Vec<ApplicationEntry>, String) {
    let result = SbusConnection::connect().and_then(|mut connection| {
        connection.call_method(
            DESKTOP_STEMD_BUS_NAME,
            DESKTOP_STEMD_OBJECT_PATH,
            DESKTOP_STEMD_INTERFACE,
            DESKTOP_STEMD_LIST_APPLICATIONS_METHOD,
            Vec::new(),
        )
    });

    let Ok(arguments) = result else {
        return (Vec::new(), String::from("Waiting for applications..."));
    };

    let mut applications = Vec::new();
    for chunk in arguments.chunks(3) {
        if chunk.len() != 3 {
            continue;
        }
        let (Argument::String(app_id), Argument::String(name), Argument::String(icon)) =
            (&chunk[0], &chunk[1], &chunk[2])
        else {
            continue;
        };
        if app_id == APP_ID || app_id.is_empty() || name.is_empty() {
            continue;
        }
        applications.push(ApplicationEntry {
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
    let status = if applications.is_empty() {
        String::from("No applications are registered with stemd")
    } else {
        format!("{} applications", applications.len())
    };
    (applications, status)
}

fn run_launcher_service(app: LauncherApp) {
    loop {
        let Ok(mut connection) = SbusConnection::connect() else {
            thread::sleep(SERVICE_RETRY_DELAY);
            continue;
        };
        if connection
            .register_service(DESKTOP_LAUNCHER_BUS_NAME)
            .is_err()
        {
            thread::sleep(SERVICE_RETRY_DELAY);
            continue;
        }
        println!("[launcher] registered as {DESKTOP_LAUNCHER_BUS_NAME}");

        loop {
            let message = match connection.receive_message() {
                Ok(message) => message,
                Err(error) => {
                    eprintln!("[launcher] sbus connection lost: {error:?}");
                    thread::sleep(SERVICE_RETRY_DELAY);
                    break;
                }
            };
            let Message::CallMethod {
                path,
                interface,
                method,
                ..
            } = message
            else {
                continue;
            };
            if path != DESKTOP_LAUNCHER_OBJECT_PATH || interface != DESKTOP_LAUNCHER_INTERFACE {
                continue;
            }
            if method != DESKTOP_LAUNCHER_SHOW_METHOD {
                let _ = connection.send_method_error(
                    0,
                    "org.scarlet.desktop.Launcher.UnknownMethod",
                    "Unknown Launcher method",
                );
                continue;
            }

            // Every invocation starts with a clean query and selection while
            // reusing the resident process and its warmed application catalog.
            app.query.set(String::new());
            app.search_focused.set(false);
            app.selected.set(None);
            app.hovered.set(None);
            open_window("main");
            let _ = connection.send_method_return(0, Vec::new());
        }
    }
}

fn icon_for_desktop_name(name: &str) -> Icon {
    match name {
        "applications-development" | "code" => Icon::Code,
        "file-description" => Icon::FileDescription,
        "file-music" => Icon::FileMusic,
        "folder" => Icon::Folder,
        "launcher" | "apps" => Icon::Apps,
        "preferences-system" => Icon::Settings,
        "preferences-system-time" => Icon::Clock,
        "text-editor" => Icon::FileText,
        "utilities-system-monitor" => Icon::ChartBar,
        "utilities-terminal" => Icon::Terminal,
        "video" | "multimedia-player" => Icon::Video,
        _ => Icon::Apps,
    }
}

fn main() -> ExitCode {
    println!("[launcher] starting resident launcher");
    let mut app = LauncherApp::new();
    let service_app = app.clone();
    thread::spawn(move || run_launcher_service(service_app));
    match app.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[launcher] error: {error}");
            ExitCode::FAILURE
        }
    }
}
