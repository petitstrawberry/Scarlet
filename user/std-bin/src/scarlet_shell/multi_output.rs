//! One set of desktop services, with independent visual state per output.
use super::*;
use scarlet_ui::scene::{SceneBuilder, SceneWindowKey};
use scarlet_ui::state::Listenable;
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};
use sws_protocol::outputs::{OutputEvent, OutputSnapshot};

const SCENES: &[&str] = &["main", CONTROL_CENTER_SCENE_KEY, HOME_BACKDROP_SCENE_KEY,
    OVERVIEW_DRAWER_SCENE_KEY, HOME_SCENE_KEY, CONSOLE_CHROME_SCENE_KEY, volume_osd::SCENE_KEY,
    SESSION_CONFIRMATION_SCENE_KEY];

pub(super) fn key(output: u32, local: &str) -> String {
    if output == 0 { local.into() } else { format!("output-{output}/{local}") }
}

#[derive(Clone)]
pub(super) struct Endpoint {
    pub id: u32,
    pub workspace: State<Option<sws::WorkspaceState>>,
    pub commands: State<Vec<WorkspaceCommand>>,
    pub open_menu: State<Option<usize>>,
    pub menu_surfaces: State<Vec<u32>>,
    pub session_confirmation: State<Option<SessionAction>>,
    pub menu_tree: State<MenuTree>,
    pub active_window: State<u32>,
    pub screen_width: State<f32>,
    pub layout: State<ShellLayout>,
    pub windows: State<Vec<ShellWindow>>,
    pub overview_page: State<usize>,
}
impl Endpoint {
    pub(super) fn of(app: &ShellApp) -> Self {
        Self { id: app.output_id.get(), workspace: app.workspace_state.clone(),
            commands: app.workspace_commands.clone(), open_menu: app.open_menu_index.clone(),
            menu_surfaces: app.menu_surfaces.clone(), menu_tree: app.menu_tree.clone(),
            session_confirmation: app.session_confirmation.clone(),
            active_window: app.active_window_id.clone(), screen_width: app.screen_width.clone(),
            layout: app.shell_layout.clone(), windows: app.windows.clone(), overview_page: app.overview_page.clone() }
    }
}

#[derive(Clone)]
pub(super) struct SessionBus {
    pub outputs: State<Option<OutputSnapshot>>,
    pub endpoints: State<Vec<Endpoint>>,
    pub windows: State<Vec<ShellWindow>>,
    pub panel_output: State<u32>,
    topology: Arc<AtomicU64>,
    changed: Arc<AtomicU64>,
}
static SESSION: OnceLock<SessionBus> = OnceLock::new();
pub(super) fn session() -> Option<&'static SessionBus> { SESSION.get() }

impl SessionBus {
    pub fn topology_generation(&self) -> u32 { self.topology.load(Ordering::Acquire) as u32 }
    pub fn observe(&self, state: &dyn Listenable) {
        let changed = self.changed.clone();
        state.subscribe_any(Arc::new(move || { changed.fetch_add(1, Ordering::Relaxed); }));
    }
    pub fn receive(&self, event: OutputEvent) {
        match event {
            OutputEvent::Snapshot(snapshot) => {
                if self.outputs.get().as_ref() != Some(&snapshot) {
                    let generation = snapshot.generation;
                    self.topology.store(u64::from(generation), Ordering::Release);
                    self.outputs.set(Some(snapshot));
                }
            }
            OutputEvent::Workspace { output, state } => {
                if let Some(endpoint) = self.endpoints.get().iter().find(|endpoint| endpoint.id == output) {
                    set_state_if_changed(&endpoint.workspace, Some(state));
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Default)]
struct WorkspaceMenus {
    // Only a focus event advances recency. Title and menu edits can arrive
    // after focus has moved and must not select that window again.
    history: Vec<AppMenuMetadata>,
    known_windows: std::collections::BTreeSet<u32>,
}

impl WorkspaceMenus {
    fn receive(&mut self, mut update: AppMenuMetadata) {
        let position = self.history.iter().position(|entry| entry.window_id == update.window_id);
        if let Some(position) = position {
            if update.menus.is_empty() {
                update.menus.clone_from(&self.history[position].menus);
            }
            if !update.focused {
                self.history[position] = update;
                return;
            }
            self.history.remove(position);
        }
        if update.focused {
            self.history.push(update);
        } else {
            self.history.insert(0, update);
        }
    }

    fn update_windows(&mut self, windows: &[ShellWindow]) {
        let current: std::collections::BTreeSet<_> = windows.iter().map(|window| window.window_id).collect();
        self.history.retain(|entry| {
            !self.known_windows.contains(&entry.window_id) || current.contains(&entry.window_id)
        });
        self.known_windows = current;
    }

    fn selected<'a>(
        &'a self,
        state: &sws::WorkspaceState,
        windows: &[ShellWindow],
    ) -> Option<&'a AppMenuMetadata> {
        let workspace = state.workspaces.iter().find(|workspace| workspace.id == state.active_workspace)?;
        self.history.iter().rev().find(|entry| {
            workspace.window_ids.contains(&entry.window_id)
                && !windows.iter().any(|window| window.window_id == entry.window_id && window.minimized)
        })
    }
}

#[derive(Clone)]
pub(super) struct ShellSession {
    primary: ShellApp,
    secondary: Vec<ShellApp>,
    bus: SessionBus,
    topology_generation: u32,
    model_generation: u64,
    filter_queries: BTreeMap<u32, String>,
    scene_owners: BTreeMap<u32, u32>,
    menus: WorkspaceMenus,
    menus_changed: bool,
}

impl ShellSession {
    pub fn new(primary: ShellApp) -> Self {
        let make_id = scarlet_ui::state::generate_state_id;
        let bus = SessionBus {
            outputs: State::new(make_id(), None), endpoints: State::new(make_id(), vec![Endpoint::of(&primary)]),
            windows: State::new(make_id(), Vec::new()), panel_output: State::new(make_id(), 0),
            changed: Arc::new(AtomicU64::new(1)),
            topology: Arc::new(AtomicU64::new(0)),
        };
        bus.observe(&bus.outputs); bus.observe(&bus.windows); bus.observe(&primary.home_applications);
        bus.observe(&primary.workspace_state); bus.observe(&primary.input_environment_snapshot);
        let _ = SESSION.set(bus.clone());
        Self { primary, secondary: Vec::new(), bus, topology_generation: 0, model_generation: 0, filter_queries: BTreeMap::new(), scene_owners: BTreeMap::new(), menus: WorkspaceMenus::default(), menus_changed: false }
    }
    fn app(&self, id: u32) -> Option<&ShellApp> {
        if id == 0 { Some(&self.primary) } else { self.secondary.iter().find(|app| app.output_id.get() == id) }
    }
    fn app_mut(&mut self, id: u32) -> Option<&mut ShellApp> {
        if id == 0 { Some(&mut self.primary) } else { self.secondary.iter_mut().find(|app| app.output_id.get() == id) }
    }
    fn split_key(&self, key: &SceneWindowKey) -> Option<(u32, SceneWindowKey)> {
        if let Some(rest) = key.as_str().strip_prefix("output-") {
            let (id, local) = rest.split_once('/')?;
            Some((id.parse().ok()?, local.into()))
        } else { Some((0, key.clone())) }
    }
    fn local_context(&self, context: &WindowContext) -> Option<(u32, WindowContext)> {
        let (id, local) = self.split_key(&context.scene_key)?;
        let mut context = context.clone(); context.scene_key = local;
        Some((id, context))
    }
    fn owner(&self, window: u32) -> u32 {
        if let Some(output) = self.scene_owners.get(&window) { return *output; }
        self.bus.endpoints.get().iter().find_map(|endpoint| {
            (endpoint.menu_surfaces.get().contains(&window)
                || endpoint.workspace.get().is_some_and(|state| state.workspaces.iter().any(|workspace| workspace.window_ids.contains(&window))))
                .then_some(endpoint.id)
        }).unwrap_or(0)
    }
    fn reconcile(&mut self) {
        if self.bus.topology.load(Ordering::Acquire) == u64::from(self.topology_generation) { return; }
        let Some(snapshot) = self.bus.outputs.get() else { return; };
        if snapshot.generation == self.topology_generation { return; }
        self.topology_generation = snapshot.generation;
        self.scene_owners.retain(|_, owner| snapshot.outputs.iter().any(|output| output.id == *owner));
        self.filter_queries.retain(|owner, _| snapshot.outputs.iter().any(|output| output.id == *owner));
        self.secondary.retain(|app| {
            if snapshot.outputs.iter().any(|output| output.id == app.output_id.get()) { return true; }
            for scene in SCENES { dismiss_window(key(app.output_id.get(), scene)); }
            false
        });
        let mut added = Vec::new();
        for descriptor in &snapshot.outputs {
            if self.app(descriptor.id).is_some() { continue; }
            let mut app = ShellApp::new_output(ShellLayout::from_tablet_mode(Some(false)), descriptor.id);
            // Providers and catalogs have a single owner. Per-output selection,
            // windows, menus, and workspace state remain independent.
            app.clock = self.primary.clock.clone();
            app.status_snapshot = self.primary.status_snapshot.clone();
            app.console_audio = self.primary.console_audio.clone();
            app.recent_app_ids = self.primary.recent_app_ids.clone();
            app.home_applications = self.primary.home_applications.clone();
            app.control_center_session = self.primary.control_center_session.clone();
            app.control_center_volume = self.primary.control_center_volume.clone();
            app.control_center_brightness = self.primary.control_center_brightness.clone();
            app.display_brightness_percent = self.primary.display_brightness_percent.clone();
            app.input_environment_snapshot = self.primary.input_environment_snapshot.clone();
            app.control_center_action = self.primary.control_center_action.clone();
            app.full_app_menus = self.primary.full_app_menus.clone();
            self.bus.observe(&app.workspace_state); self.bus.observe(&app.home_query);
            added.push(descriptor.id);
            self.secondary.push(app);
        }
        self.bus.endpoints.set(core::iter::once(&self.primary).chain(&self.secondary).map(Endpoint::of).collect());
        if let Ok(connection) = sws::Connection::connect("/tmp/sws.sock") {
            for id in added.iter().copied() {
                if let Ok(state) = connection.output_workspace(id) { self.bus.receive(OutputEvent::Workspace { output: id, state }); }
            }
        }
        for app in core::iter::once(&self.primary).chain(&self.secondary) {
            let Some(config) = snapshot.configuration.iter().find(|config| config.id == app.output_id.get()) else { continue; };
            let Some(output) = snapshot.outputs.iter().find(|output| output.id == config.id) else { continue; };
            if let Ok((width, height)) = output.mode.logical_size(config.scale_milli) {
                set_state_if_changed(&app.screen_width, width as f32);
                set_state_if_changed(&app.screen_height, height as f32);
            }
            app.update_workarea(app.screen_width.get() as u32, app.screen_height.get() as u32, app.shell_layout.get());
        }
        for id in added {
            for scene in ["main", HOME_BACKDROP_SCENE_KEY, HOME_SCENE_KEY, OVERVIEW_DRAWER_SCENE_KEY] { open_window(key(id, scene)); }
        }
        self.bus.changed.fetch_add(1, Ordering::Relaxed);
    }
    fn receive_menu(&mut self, update: AppMenuMetadata) {
        let output = self.owner(update.window_id);
        let is_shell_scene = self.scene_owners.contains_key(&update.window_id);
        if let Some(app) = self.app_mut(output) {
            if update.focused {
                app.dismiss_control_center_for_focus(update.window_id, &update.app_name);
            } else {
                app.record_recent_window(update.window_id);
            }
        }
        // Metadata can arrive before the corresponding workspace snapshot.
        // Keep it by window ID and resolve its output when the model settles.
        if !is_shell_scene && update.window_id != 0 && !update.app_name.is_empty()
            && !is_shell_menu_name(&update.app_name)
        {
            self.menus.receive(update);
            self.menus_changed = true;
        }
    }

    fn synchronize_menus(&mut self) {
        if !core::mem::take(&mut self.menus_changed) { return; }
        for app in core::iter::once(&mut self.primary).chain(&mut self.secondary) {
            let Some(state) = app.workspace_state.get() else { continue; };
            let windows = app.windows.get();
            if let Some(entry) = self.menus.selected(&state, &windows) {
                let (menus, changed) = app.resolve_menu_titles(entry.window_id, &entry.menus);
                app.update_menu_for_app(entry.window_id, &entry.app_name, &entry.title, &menus, changed);
            } else {
                app.update_menu_for_app(0, "", "", "", false);
            }
        }
    }

    fn synchronize_models(&mut self) {
        let generation = self.bus.changed.load(Ordering::Relaxed);
        let changed = generation != self.model_generation;
        if !changed { return; }
        self.model_generation = generation;
        let catalog = changed.then(|| self.primary.home_applications.get());
        let all_windows = self.bus.windows.get();
        self.menus.update_windows(&all_windows);
        self.menus_changed = true;
        let topology = self.bus.outputs.get();
        let physical_tablet = self.primary.input_environment_snapshot.get().is_some_and(|environment| environment.tablet_mode == Some(true));
        for app in core::iter::once(&self.primary).chain(&self.secondary) {
            if let Some(snapshot) = &topology {
                if let Some(output) = snapshot.outputs.iter().find(|output| output.id == app.output_id.get()) {
                    if let Some(config) = snapshot.configuration.iter().find(|config| config.id == output.id) {
                        let layout = ShellLayout::from_tablet_mode(Some(config.experience.tablet_experience(output.internal, physical_tablet)));
                        if app.shell_layout.get() != layout {
                            app.open_menu_index.set(None);
                            app.shell_layout.set(layout);
                            app.update_workarea(app.screen_width.get() as u32, app.screen_height.get() as u32, layout);
                        }
                    }
                }
            }
            let workspace = app.workspace_state.get();
            let filtered = all_windows.iter().filter(|window| workspace.as_ref().is_some_and(|state| state.workspaces.iter().any(|workspace| workspace.window_ids.contains(&window.window_id)))).cloned().collect();
            set_state_if_changed(&app.windows, filtered);
            if app.output_id.get() == 0 { continue; } // Primary's existing filter owns its query.
            let query = app.home_query.get();
            if changed || self.filter_queries.get(&app.output_id.get()) != Some(&query) {
                let applications = catalog.clone().unwrap_or_else(|| self.primary.home_applications.get());
                let next = filter_home_applications(&applications, &query);
                let previous = app.home_filtered_applications.get();
                set_state_if_changed(&app.home_selected, home_catalog::refreshed_selection(&previous, &next, app.home_selected.get(), true));
                set_state_if_changed(&app.home_filtered_applications, next);
                set_state_if_changed(&app.home_hovered, None);
                self.filter_queries.insert(app.output_id.get(), query);
            }
        }
    }
}

impl View for ShellSession {
    fn create_element(&self) -> Box<dyn scarlet_ui::element::Element> { self.primary.create_element() }
    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[derive(Clone)]
struct ShellScenes(Vec<ShellApp>);
impl Scene for ShellScenes {
    fn build(self, builder: &mut SceneBuilder) {
        for app in self.0 { app.scenes().build(builder); }
    }
}
impl Application for ShellSession {
    fn scenes(&self) -> impl Scene { ShellScenes(core::iter::once(self.primary.clone()).chain(self.secondary.iter().cloned()).collect()) }
    fn scene_view(&self, scene: &SceneWindowKey) -> Option<Box<dyn View>> { let (id, local) = self.split_key(scene)?; self.app(id)?.scene_view(&local) }
    fn scene_listenables(&self, scene: &SceneWindowKey) -> Option<Vec<&dyn Listenable>> {
        let (id, local) = self.split_key(scene)?; let app = self.app(id)?;
        let mut dependencies = app.scene_listenables(&local).unwrap_or_else(|| app.listenables());
        if id == 0 && local.as_str() == "main" { dependencies.push(&self.bus.outputs); }
        Some(dependencies)
    }
    fn init(&mut self) { self.primary.init(); }
    fn on_idle(&mut self) {
        self.reconcile(); self.synchronize_models();
        let mut updates = Vec::new();
        if !self.primary.pending_app_menus.get().is_empty() { self.primary.pending_app_menus.update(|pending| updates = core::mem::take(pending)); }
        for update in updates { self.receive_menu(update); }
        self.synchronize_menus();
        self.primary.sync_control_center_scene();
        for app in &mut self.secondary { app.sync_control_center_scene(); }
    }
    fn on_window_created(&mut self, context: &WindowContext, window: &mut dyn PlatformWindow) {
        if let Some((id, local)) = self.local_context(context) {
            self.scene_owners.insert(context.platform_window_id as u32, id);
            if let Some(app) = self.app_mut(id) { app.on_window_created(&local, window); }
        }
    }
    fn on_window_sync(&mut self, context: &WindowContext, window: &mut dyn PlatformWindow) {
        if let Some((id, local)) = self.local_context(context) { if let Some(app) = self.app_mut(id) { app.on_window_sync(&local, window); } }
    }
    fn on_window_close_requested(&mut self, context: &WindowContext) -> bool {
        self.local_context(context).and_then(|(id, local)| self.app_mut(id).map(|app| app.on_window_close_requested(&local))).unwrap_or(true)
    }
    fn on_frame_presented(&mut self, context: &WindowContext) {
        if let Some((id, local)) = self.local_context(context) { if let Some(app) = self.app_mut(id) { app.on_frame_presented(&local); } }
    }
    fn on_focus_changed_with_title(&mut self, window: u32, app_name: &str, title: &str, menus: &str) {
        if !self.primary.full_app_menus.get() {
            self.receive_menu(AppMenuMetadata {
                focused: true,
                window_id: window,
                app_name: app_name.into(),
                title: title.into(),
                menus: menus.into(),
            });
        }
    }
    fn on_active_app_changed_with_title(&mut self, window: u32, app_name: &str, title: &str, menus: &str) {
        if !self.primary.full_app_menus.get() {
            self.receive_menu(AppMenuMetadata {
                focused: false,
                window_id: window,
                app_name: app_name.into(),
                title: title.into(),
                menus: menus.into(),
            });
        }
    }
    fn exit_when_all_windows_closed(&self) -> bool { false }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_window_menu_follows_workspace_membership_without_another_focus_event() {
        fn state(id: u32, windows: Vec<u32>) -> sws::WorkspaceState {
            sws::WorkspaceState {
                generation: 1,
                active_workspace: id,
                normal_workspace: id,
                presentation: sws::ShellPresentation::Workspace,
                workspaces: vec![sws::WorkspaceSnapshot {
                    id,
                    window_ids: windows,
                    tablet_layout: sws::TabletLayout::Empty,
                }],
            }
        }

        let mut menus = WorkspaceMenus::default();
        for (window_id, app_name) in [(10, "Files"), (20, "Settings"), (30, "Vellum")] {
            menus.receive(AppMenuMetadata {
                focused: true,
                window_id,
                app_name: app_name.into(),
                title: app_name.into(),
                menus: format!("{app_name} File;Edit"),
            });
        }

        let mut source = state(1, vec![10, 30]);
        let mut destination = state(2, vec![20]);
        assert_eq!(menus.selected(&source, &[]).unwrap().window_id, 30);
        assert_eq!(menus.selected(&destination, &[]).unwrap().window_id, 20);

        source.workspaces[0].window_ids.retain(|id| *id != 30);
        destination.workspaces[0].window_ids.push(30);
        assert_eq!(menus.selected(&source, &[]).unwrap().window_id, 10);
        let moved = menus.selected(&destination, &[]).unwrap();
        assert_eq!(moved.window_id, 30);
        assert_eq!(moved.menus, "Vellum File;Edit");
    }
}
