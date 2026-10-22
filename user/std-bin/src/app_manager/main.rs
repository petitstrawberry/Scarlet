//! Install and remove finished native Scarlet applications.
mod model;

use model::AppInfo;
use scarlet_ui::prelude::*;
use scarlet_ui::views::Either;
use scarlet_ui::{ScrollView, Select, hstack, measure_text_sized, vstack};
use scarlet_ui_macros::View;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const APP_ID: &str = "org.scarlet-os.desktop.applications";
const APPS_ROOT: &str = "/applications";

enum Completion {
    List(std::result::Result<Vec<AppInfo>, String>),
    Inspect(std::result::Result<AppInfo, String>),
    Change {
        outcome: std::result::Result<String, String>,
        apps: std::result::Result<Vec<AppInfo>, String>,
    },
}
#[derive(Default)]
struct Runtime {
    completion: Option<Completion>,
}
#[derive(Clone, View)]
struct ApplicationsApp {
    apps: State<Vec<AppInfo>>,
    fixture_catalog: State<bool>,
    selected: State<usize>,
    source: State<String>,
    inspected: State<Option<AppInfo>>,
    remove_confirmation: State<Option<AppInfo>>,
    busy: State<bool>,
    status: State<String>,
    runtime: State<Arc<Mutex<Runtime>>>,
}
impl ApplicationsApp {
    fn new() -> Self {
        let app = Self::default();
        app.selected.set(usize::MAX);
        #[cfg(target_os = "scarlet")]
        app.refresh(false);
        #[cfg(not(target_os = "scarlet"))]
        app.status.set("Live application management requires Scarlet. You can inspect a finished .app directory on this host.".into());
        app
    }
    fn spawn(&self, message: &str, work: impl FnOnce() -> Completion + Send + 'static) {
        if self.busy.get() {
            return;
        }
        self.busy.set(true);
        self.status.set(message.into());
        let runtime = self.runtime.get();
        if let Err(error) = std::thread::Builder::new()
            .name("app-operation".into())
            .spawn(move || {
                runtime.lock().unwrap().completion = Some(work());
            })
        {
            self.busy.set(false);
            self.status
                .set(format!("Cannot start application operation: {error}"));
        }
    }
    fn refresh(&self, reconcile: bool) {
        self.remove_confirmation.set(None);
        self.spawn(if reconcile { "Reloading the application catalog…" } else { "Reading installed applications…" }, move || {
            if reconcile {
                let outcome = reload_catalog().map(|count| format!("Catalog reloaded: {count} applications. Running instances are unchanged"));
                Completion::Change { outcome, apps: model::installed(Path::new(APPS_ROOT)) }
            } else { Completion::List(model::installed(Path::new(APPS_ROOT))) }
        });
    }
    fn inspect(&self) {
        if self.busy.get() {
            return;
        }
        self.inspected.set(None);
        let path = PathBuf::from(self.source.get().trim());
        self.spawn("Checking the complete application package…", move || {
            Completion::Inspect(model::inspect(&path))
        });
    }
    fn install(&self) {
        if self.busy.get() || !cfg!(target_os = "scarlet") {
            return;
        }
        let Some(app) = self.inspected.get() else {
            return;
        };
        if Path::new(self.source.get().trim()) != app.path {
            self.inspected.set(None);
            self.status
                .set("The source path changed. Inspect the package again.".into());
            return;
        }
        self.spawn("Installing the inspected package…", move || {
            let outcome = model::install(Path::new(APPS_ROOT), &app, reload_catalog);
            Completion::Change {
                outcome,
                apps: model::installed(Path::new(APPS_ROOT)),
            }
        });
    }
    fn request_remove(&self) {
        if self.busy.get() {
            return;
        }
        self.remove_confirmation
            .set(self.apps.get().get(self.selected.get()).cloned());
    }
    fn remove(&self) {
        if self.busy.get() || !cfg!(target_os = "scarlet") {
            return;
        }
        let Some(app) = self.remove_confirmation.get() else {
            return;
        };
        self.remove_confirmation.set(None);
        self.spawn("Removing the selected application…", move || {
            let outcome = model::remove(Path::new(APPS_ROOT), &app, reload_catalog);
            Completion::Change {
                outcome,
                apps: model::installed(Path::new(APPS_ROOT)),
            }
        });
    }
    fn update_apps(&self, apps: Vec<AppInfo>) {
        let selected_id = self
            .apps
            .get()
            .get(self.selected.get())
            .map(|app| app.id.clone());
        let selected = selected_id
            .and_then(|id| apps.iter().position(|app| app.id == id))
            .unwrap_or(if apps.is_empty() { usize::MAX } else { 0 });
        self.apps.set(apps);
        self.selected.set(selected);
    }
    fn content(&self) -> impl View + Clone {
        let secondary = Color::rgb(88, 94, 104);
        let status = wrap_message(&self.status.get(), 664.0, 14.0);
        let status_height = status.lines().count().max(1) as f32 * 20.0;
        let apps = self.apps.get();
        let current = apps.get(self.selected.get());
        let busy = self.busy.get();
        let native = cfg!(target_os = "scarlet");
        let refresh = self.clone();
        let reload = self.clone();
        let inspect = self.clone();
        let install = self.clone();
        let remove = self.clone();
        let confirm = self.clone();
        let cancel = self.clone();
        let inspected = self.inspected.get();
        let inspected_matches = inspected
            .as_ref()
            .is_some_and(|app| app.path == Path::new(self.source.get().trim()));
        let confirmation = if let Some(app) = self.remove_confirmation.get() {
            Either::A(vstack! {
                Text::new(format!("Remove {}?", app.name)).font_size(18.0),
                Text::new(wrap_message(&format!("{}\n{}", app.id, app.path.display()), 632.0, 14.0)).font_size(14.0),
                Text::new("The package leaves /applications; its files stay in a recovery folder.\nPersonal data, running apps and legacy launcher entries are kept.").font_size(14.0),
                hstack! {
                    Button::new("Cancel").enabled(!busy).on_click(move || cancel.remove_confirmation.set(None)),
                    Button::new("Remove package").enabled(!busy && native).on_click(move || confirm.remove()),
                }.spacing(8.0),
            }.spacing(8.0).alignment(Alignment::Leading).padding(12.0).background(Color::rgb(255, 245, 234)))
        } else {
            Either::B(Spacer::new().frame_height(0.0))
        };
        let installed = if native || !apps.is_empty() {
            Either::A(vstack! {
                hstack! {
                    Text::new(format!("Installed packages · {}", apps.len())).font_size(18.0), Spacer::new(),
                    Button::new("Refresh").enabled(!busy && native).on_click(move || refresh.refresh(false)),
                    Button::new("Reload catalog").enabled(!busy && native).on_click(move || reload.refresh(true)),
                }.spacing(8.0),
                Select::new(apps.iter().map(|app| format!("{} — {}", app.name, app.id)).collect(), self.selected.clone())
                    .placeholder("No installed packages").width(648.0).enabled(!busy && !apps.is_empty()),
                Text::new(current.map(|app| wrap_message(&format!("{}\n{}\n{} · {} files · {}", app.id, app.path.display(), app.target, app.files, format_bytes(app.bytes)), 648.0, 14.0)).unwrap_or_else(|| "Install a finished .app package below to get started.".into())).font_size(14.0).color(secondary),
                Button::new("Remove package…").enabled(!busy && native && current.is_some()).on_click(move || remove.request_remove()),
                confirmation,
            }.spacing(10.0).alignment(Alignment::Leading).padding(16.0).frame_width(f32::INFINITY).background(Color::rgb(247, 248, 250)))
        } else {
            Either::B(vstack! {
                Text::new("Package inspector").font_size(18.0),
                Text::new("Inspect a Scarlet .app package on this host.\nInstallation and the live application catalog are available in Scarlet.").font_size(14.0).color(secondary),
            }.spacing(6.0).alignment(Alignment::Leading).padding(16.0).frame_width(f32::INFINITY).background(Color::rgb(247, 248, 250)))
        };
        let result = if let Some(app) = inspected.filter(|_| inspected_matches) {
            Either::A(vstack! {
                Text::new(app.name).font_size(20.0),
                Text::new(wrap_message(&app.id, 648.0, 14.0)).font_size(14.0).color(secondary),
                Text::new(format!("{}   ·   {} files   ·   {}", app.target, app.files, format_bytes(app.bytes))).font_size(14.0),
                hstack! {
                    Text::new("Package structure checked").font_size(14.0).color(secondary), Spacer::new(),
                    Button::new("Install package").enabled(!busy && native).on_click(move || install.install()),
                }.spacing(8.0),
            }.spacing(6.0).alignment(Alignment::Leading).padding(16.0).background(Color::rgb(239, 246, 244)))
        } else {
            Either::B(
                Text::new(
                    "Choose a finished .app directory to check its contents before installing.",
                )
                .font_size(14.0)
                .color(secondary)
                .padding(4.0),
            )
        };
        let content_height = if native || !apps.is_empty() {
            if inspected_matches { 740.0 } else { 580.0 }
        } else if inspected_matches {
            600.0
        } else {
            460.0
        };
        let content_height = content_height - 40.0
            + status_height
            + if self.remove_confirmation.get().is_some() {
                240.0
            } else {
                0.0
            };
        ScrollView::new(vstack! {
            Text::new(if self.fixture_catalog.get() { "FIXTURE CATALOG · host preview · management disabled" } else if native { "Packages in /applications" } else { "Host preview · package inspection only" }).font_size(14.0).color(secondary),
            installed,
            vstack! {
                Text::new(if native { "Install a package" } else { "Inspect a package" }).font_size(18.0),
                hstack! {
                    TextField::new(self.source.clone()).placeholder("/path/to/application.app").frame(f32::INFINITY, 34.0),
                    Button::new(if busy { "Working…" } else { "Inspect" }).enabled(!busy && !self.source.get().trim().is_empty()).on_click(move || inspect.inspect()),
                }.spacing(8.0),
                result,
            }.spacing(12.0).alignment(Alignment::Leading),
            Divider::new(),
            Text::new(status).font_size(14.0).frame_height(status_height),
        }.spacing(20.0).alignment(Alignment::Leading).padding(24.0))
            .content_size(0.0, content_height)
            .frame(f32::INFINITY, f32::INFINITY)
    }
}
// Status messages include recoverable filesystem paths; keep the complete
// message visible instead of clipping the instruction or recovery location.
fn wrap_message(text: &str, width: f32, font_size: f32) -> String {
    let mut output = String::new();
    let mut line = String::new();
    for original in text.split('\n') {
        if !output.is_empty() {
            output.push('\n');
        }
        for word in original.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty() && measure_text_sized(&candidate, font_size).0 as f32 > width {
                output.push_str(&line);
                output.push('\n');
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            for ch in word.chars() {
                let mut candidate = line.clone();
                candidate.push(ch);
                if !line.is_empty() && measure_text_sized(&candidate, font_size).0 as f32 > width {
                    output.push_str(&line);
                    output.push('\n');
                    line.clear();
                }
                line.push(ch);
            }
        }
        output.push_str(&line);
        line.clear();
    }
    output
}
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}
#[cfg(target_os = "scarlet")]
fn reload_catalog() -> std::result::Result<usize, String> {
    use scarlet_desktop_config::{
        DESKTOP_STEMD_BUS_NAME, DESKTOP_STEMD_INTERFACE, DESKTOP_STEMD_OBJECT_PATH,
        DESKTOP_STEMD_RELOAD_APPLICATIONS_METHOD,
    };
    let response = sbus_client::Connection::connect()
        .and_then(|mut connection| {
            connection.call_method_timeout(
                DESKTOP_STEMD_BUS_NAME,
                DESKTOP_STEMD_OBJECT_PATH,
                DESKTOP_STEMD_INTERFACE,
                DESKTOP_STEMD_RELOAD_APPLICATIONS_METHOD,
                vec![],
                5_000,
            )
        })
        .map_err(|error| format!("Application catalog service: {error:?}"))?;
    match response.as_slice() {
        [sbus::Argument::String(count)] => count
            .parse()
            .map_err(|_| "Invalid application count from catalog service".into()),
        _ => Err("Unexpected application catalog response".into()),
    }
}
#[cfg(not(target_os = "scarlet"))]
fn reload_catalog() -> std::result::Result<usize, String> {
    Err("Application catalog management requires Scarlet".into())
}
impl Application for ApplicationsApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new(
            "main",
            Window::new("Applications", self.content())
                .app_id(APP_ID)
                .size(Size::new(760.0, 560.0))
                .min_size(Size::new(740.0, 520.0))
                .resizable(true),
        )
    }
    fn debug_logging(&self) -> bool {
        false
    }
    fn on_idle(&mut self) {
        let completion = self.runtime.get().lock().unwrap().completion.take();
        if let Some(completion) = completion {
            self.busy.set(false);
            match completion {
                Completion::List(Ok(apps)) => {
                    self.status
                        .set(format!("{} installed packages", apps.len()));
                    self.update_apps(apps);
                }
                Completion::List(Err(error)) => {
                    self.status.set(format!("Cannot refresh packages: {error}"))
                }
                Completion::Inspect(Ok(app)) => {
                    self.status.set("Package structure is valid. Install only packages from a source you trust.".into());
                    self.inspected.set(Some(app));
                }
                Completion::Inspect(Err(error)) => self
                    .status
                    .set(format!("Package inspection failed: {error}")),
                Completion::Change { outcome, apps } => {
                    let mut status = outcome.unwrap_or_else(|error| error);
                    match apps {
                        Ok(apps) => self.update_apps(apps),
                        Err(error) => {
                            status.push_str(&format!(". Cannot refresh packages: {error}"))
                        }
                    }
                    self.status.set(status);
                }
            }
        }
    }
    fn on_window_close_requested(&mut self, _: &WindowContext) -> bool {
        if self.busy.get() {
            self.status
                .set("Wait for the current operation to finish before closing.".into());
            false
        } else {
            true
        }
    }
}
fn main() {
    let mut app = ApplicationsApp::new();
    let args: Vec<String> = std::env::args().collect();
    #[cfg(not(target_os = "scarlet"))]
    if args.get(1).map(String::as_str) == Some("--preview-catalog") {
        let root = args
            .get(2)
            .expect("--preview-catalog requires an explicit fixture directory");
        match model::installed(Path::new(root)) {
            Ok(apps) => {
                app.update_apps(apps);
                app.fixture_catalog.set(true);
            }
            Err(error) => {
                eprintln!("Fixture catalog: {error}");
                std::process::exit(1);
            }
        }
        app.status.set("Fixture catalog loaded from the explicit path. Install, removal and catalog reload remain disabled.".into());
        if let Some(path) = args.get(3) {
            app.source.set(path.clone());
        }
    } else if let Some(path) = args.get(1) {
        app.source.set(path.clone());
    }
    #[cfg(target_os = "scarlet")]
    if let Some(path) = args.get(1) {
        app.source.set(path.clone());
    }
    if let Err(error) = app.run() {
        eprintln!("Applications: {error}");
        std::process::exit(1);
    }
}
