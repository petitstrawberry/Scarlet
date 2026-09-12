//! Console Home: ordinary ScarletUI views backed by shell catalog and status snapshots.

use super::app_artwork::AppArtwork;
use scarlet_ui::views::ScrollbarVisibility;
use std::{any::Any, sync::Arc};

#[path = "app_tile.rs"]
pub mod app_tile;

use scarlet_ui::element::{
    Element, ElementRenderObject, LayoutConstraints, RenderElement, UpdateResult,
};
use scarlet_ui::prelude::*;
use scarlet_ui::view::ViewKey;
use scarlet_ui::views::containers::ViewTuple;
use scarlet_ui::{Color, Icon, IconSize, KeyCode, KeyEvent, State};

// Match the existing Home's translucent wallpaper tint and ScarletUI's dark
// semantic palette. Application accents come from the shared launcher icons.
const BACKGROUND: Color = Color::rgba_f32(0.30, 0.34, 0.41, 0.78);
const CARD: Color = Color::rgba_f32(0.22, 0.22, 0.22, 0.72);
pub const FROST: Color = Color::rgba_f32(28.0 / 255.0, 28.0 / 255.0, 32.0 / 255.0, 0.48);
const FLOATING: Color = FROST;
const FLOATING_ACTIVE: Color = Color::rgba_f32(56.0 / 255.0, 56.0 / 255.0, 59.0 / 255.0, 0.58);
pub const MATERIAL_BLUR_RADIUS: f32 = 8.0;
const TEXT: Color = Color::WHITE;
const MUTED: Color = Color::rgba_f32(235.0 / 255.0, 235.0 / 255.0, 245.0 / 255.0, 0.6);
const ACCENT: Color = Color::rgba_f32(0.88, 0.25, 0.25, 1.0);
const LINE: Color = Color::rgba_f32(1.0, 1.0, 1.0, 0.10);
const RADIUS: f32 = 20.0;
const SECTION_INSET: f32 = 12.0;
const SECTION_HEADER_HEIGHT: f32 = 27.0;
const SECTION_GAP: f32 = 10.0;
const SHELF_CHROME_HEIGHT: f32 = SECTION_INSET * 2.0 + SECTION_HEADER_HEIGHT + SECTION_GAP;
const ACTION_HEIGHT: f32 = 44.0;
const ACTION_GAP: f32 = 8.0;
const WORKSPACE_BUTTON_WIDTH: f32 = 84.0;

#[derive(Clone)]
pub struct ApplicationTile {
    pub app_id: String,
    pub name: String,
    pub icon: Icon,
    pub color: Color,
    pub artwork: AppArtwork,
}

#[derive(Clone)]
pub struct WorkspaceTile {
    pub id: u32,
    pub window_count: usize,
}

#[derive(Clone, Default)]
pub struct ConsoleSnapshot {
    pub width: f32,
    pub height: f32,
    pub bar_height: f32,
    pub applications: Vec<ApplicationTile>,
    pub recent_ids: Vec<String>,
    pub workspaces: Vec<WorkspaceTile>,
    pub active_workspace: u32,
    pub volume: Option<u8>,
    pub muted: Option<bool>,
    pub launch_error: Option<String>,
}

impl ConsoleSnapshot {
    fn recent(&self) -> Vec<ApplicationTile> {
        self.recent_ids
            .iter()
            .filter_map(|id| {
                self.applications
                    .iter()
                    .find(|app| &app.app_id == id)
                    .cloned()
            })
            .collect()
    }

    fn count(&self, region: Region) -> usize {
        match region {
            Region::Library => self.applications.len(),
            Region::Recent => self.recent().len(),
            Region::Quick => 3,
            Region::Workspaces => self.workspaces.len(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    Reboot,
    PowerOff,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsoleAction {
    Launch(String),
    Workspace(u32),
    CycleWorkspace(i32),
    Back,
    Settings,
    Volume(u8),
    Mute,
    Power(PowerAction),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Region {
    #[default]
    Recent,
    Library,
    Quick,
    Workspaces,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConsoleState {
    region: Region,
    index: usize,
    app_focus: Option<(Region, usize)>,
    recent_index: Option<usize>,
    library_index: Option<usize>,
    quick_index: usize,
    control_open: bool,
    control_index: usize,
    armed: Option<PowerAction>,
}

impl ConsoleState {
    pub fn did_launch(&mut self) {
        self.close_controls();
        // A successful launch becomes the first recent app. Follow its new
        // position when Home is shown again instead of selecting a different app.
        self.focus(Region::Recent, 0);
    }

    pub fn close_controls(&mut self) {
        self.control_open = false;
        self.armed = None;
    }

    fn normalize(&mut self, snapshot: &ConsoleSnapshot) {
        if snapshot.count(self.region) == 0 {
            self.region = if self.region == Region::Recent && !snapshot.applications.is_empty() {
                Region::Library
            } else {
                Region::Quick
            };
        }
        self.index = self
            .index
            .min(snapshot.count(self.region).saturating_sub(1));
    }

    fn focus(&mut self, region: Region, index: usize) {
        self.region = region;
        self.index = index;
        match region {
            Region::Recent | Region::Library => {
                self.app_focus = Some((region, index));
                match region {
                    Region::Recent => self.recent_index = Some(index),
                    Region::Library => self.library_index = Some(index),
                    _ => unreachable!(),
                }
            }
            Region::Quick => self.quick_index = index,
            Region::Workspaces => {}
        }
    }

    fn remembered_app_index(&self, region: Region) -> Option<usize> {
        match region {
            Region::Recent => self.recent_index,
            Region::Library => self.library_index,
            _ => None,
        }
    }

    fn return_to_apps(&mut self, snapshot: &ConsoleSnapshot) {
        let (region, index) = self.app_focus.unwrap_or((Region::Recent, 0));
        if snapshot.count(region) > 0 {
            self.focus(region, index.min(snapshot.count(region) - 1));
        } else if !snapshot.applications.is_empty() {
            self.focus(Region::Library, 0);
        }
    }

    fn move_home_focus(&mut self, key: KeyCode, snapshot: &ConsoleSnapshot) {
        match self.region {
            Region::Recent | Region::Library => match key {
                KeyCode::Left => self.focus(self.region, self.index.saturating_sub(1)),
                KeyCode::Right => self.focus(
                    self.region,
                    (self.index + 1).min(snapshot.count(self.region) - 1),
                ),
                KeyCode::Up
                    if self.region == Region::Library && snapshot.count(Region::Recent) > 0 =>
                {
                    self.focus(
                        Region::Recent,
                        self.remembered_app_index(Region::Recent)
                            .unwrap_or(self.index)
                            .min(snapshot.count(Region::Recent) - 1),
                    );
                }
                KeyCode::Down
                    if self.region == Region::Recent && snapshot.count(Region::Library) > 0 =>
                {
                    self.focus(
                        Region::Library,
                        self.remembered_app_index(Region::Library)
                            .unwrap_or(self.index)
                            .min(snapshot.count(Region::Library) - 1),
                    );
                }
                KeyCode::Down if self.region == Region::Library => {
                    self.app_focus = Some((self.region, self.index));
                    self.focus(Region::Quick, self.quick_index);
                }
                _ => {}
            },
            Region::Quick => match key {
                KeyCode::Up => self.return_to_apps(snapshot),
                KeyCode::Left => self.focus(Region::Quick, self.index.saturating_sub(1)),
                KeyCode::Right if self.index + 1 < snapshot.count(Region::Quick) => {
                    self.focus(Region::Quick, self.index + 1);
                }
                KeyCode::Right if !snapshot.workspaces.is_empty() => {
                    let active = snapshot
                        .workspaces
                        .iter()
                        .position(|ws| ws.id == snapshot.active_workspace)
                        .unwrap_or(0);
                    self.focus(Region::Workspaces, active);
                }
                _ => {}
            },
            Region::Workspaces => match key {
                KeyCode::Left if self.index == 0 => {
                    self.focus(Region::Quick, snapshot.count(Region::Quick) - 1)
                }
                KeyCode::Left => self.index -= 1,
                KeyCode::Right => self.index = (self.index + 1).min(snapshot.workspaces.len() - 1),
                KeyCode::Up => self.return_to_apps(snapshot),
                _ => {}
            },
        }
    }

    fn move_focus(&mut self, key: KeyCode, snapshot: &ConsoleSnapshot) {
        self.normalize(snapshot);
        if self.control_open {
            let count: usize = if self.armed.is_some() { 2 } else { 7 };
            let step = if matches!(key, KeyCode::Up | KeyCode::Down) {
                2
            } else {
                1
            };
            self.control_index = if matches!(key, KeyCode::Left | KeyCode::Up) {
                self.control_index.saturating_sub(step)
            } else {
                (self.control_index + step).min(count - 1)
            };
            return;
        }
        self.move_home_focus(key, snapshot);
    }

    fn activate(&mut self, snapshot: &ConsoleSnapshot) -> Option<ConsoleAction> {
        self.normalize(snapshot);
        if self.control_open {
            return self.activate_control(snapshot);
        }
        match self.region {
            Region::Recent | Region::Library => {
                let apps = match self.region {
                    Region::Recent => snapshot.recent(),
                    _ => snapshot.applications.clone(),
                };
                apps.get(self.index)
                    .map(|app| ConsoleAction::Launch(app.app_id.clone()))
            }
            Region::Quick => match self.index {
                0 => {
                    self.open_controls();
                    self.control_index = 5;
                    None
                }
                1 => {
                    self.open_controls();
                    None
                }
                _ => Some(ConsoleAction::Settings),
            },
            Region::Workspaces => snapshot
                .workspaces
                .get(self.index)
                .map(|ws| ConsoleAction::Workspace(ws.id)),
        }
    }

    fn open_controls(&mut self) {
        self.control_open = true;
        self.control_index = 0;
        self.armed = None;
    }

    fn activate_control(&mut self, snapshot: &ConsoleSnapshot) -> Option<ConsoleAction> {
        if let Some(power) = self.armed.take() {
            let confirmed = self.control_index == 1;
            self.control_index = 0;
            return confirmed.then_some(ConsoleAction::Power(power));
        }
        match self.control_index {
            0 => snapshot
                .volume
                .map(|volume| ConsoleAction::Volume(volume.saturating_sub(5))),
            1 => snapshot
                .volume
                .map(|volume| ConsoleAction::Volume(volume.saturating_add(5).min(100))),
            2 => snapshot.muted.map(|_| ConsoleAction::Mute),
            3 => Some(ConsoleAction::Settings),
            4 | 5 => {
                self.armed = Some(if self.control_index == 4 {
                    PowerAction::Reboot
                } else {
                    PowerAction::PowerOff
                });
                self.control_index = 0;
                None
            }
            _ => {
                self.control_open = false;
                None
            }
        }
    }

    pub fn handle_key(
        &mut self,
        event: KeyEvent,
        snapshot: &ConsoleSnapshot,
    ) -> (bool, Option<ConsoleAction>) {
        // Printable shortcuts use Char only: SWS emits both Pressed and Char.
        let action = match event {
            KeyEvent::Char { c: 'm' | 'M' } => {
                if self.control_open {
                    self.control_open = false;
                    self.armed = None;
                } else {
                    self.open_controls();
                }
                None
            }
            KeyEvent::Char { c: 'q' | 'Q' } if !self.control_open => {
                Some(ConsoleAction::CycleWorkspace(-1))
            }
            KeyEvent::Char { c: 'e' | 'E' } if !self.control_open => {
                Some(ConsoleAction::CycleWorkspace(1))
            }
            KeyEvent::Pressed { keycode, modifiers }
                if !modifiers.control && !modifiers.alt && !modifiers.super_key =>
            {
                match keycode {
                    KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                        self.move_focus(keycode, snapshot);
                        None
                    }
                    KeyCode::Enter => self.activate(snapshot),
                    KeyCode::Escape => {
                        if self.armed.take().is_some() {
                            self.control_index = 0;
                            None
                        } else if self.control_open {
                            self.control_open = false;
                            None
                        } else {
                            Some(ConsoleAction::Back)
                        }
                    }
                    _ => return (false, None),
                }
            }
            _ => return (false, None),
        };
        (true, action)
    }
}

/// Full-screen scrolling with an unobscured area for keyboard selection.
#[derive(Clone, Copy, Debug)]
pub struct ConsoleLayout {
    pub width: f32,
    pub height: f32,
    pub padding: f32,
    pub gap: f32,
    pub app_safe_height: f32,
    pub main_width: f32,
    pub action_bar_height: f32,
    pub action_widths: [f32; 3],
    pub shelf_height: f32,
    pub bottom_inset: f32,
}

impl ConsoleLayout {
    pub fn chrome_height(self) -> f32 {
        self.action_bar_height + self.bottom_inset
    }
    pub fn chrome_top(self) -> f32 {
        self.height - self.chrome_height()
    }
    pub fn resolve(snapshot: &ConsoleSnapshot) -> Self {
        let width = snapshot.width.round().max(320.0);
        let height = snapshot.height.round().max(320.0);
        let padding = (width.min(height) * 0.023).clamp(12.0, 28.0).round();
        let gap = (height * 0.017).clamp(10.0, 18.0).round();
        let bottom_inset = 12.0;
        let main_width = width - padding * 2.0;
        let action_bar_height = ACTION_HEIGHT + gap;
        let app_safe_height =
            (height - snapshot.bar_height - bottom_inset - action_bar_height).max(1.0);
        let action_widths = if width < 600.0 {
            [ACTION_HEIGHT; 3]
        } else if width < 800.0 {
            [108.0, 90.0, 116.0]
        } else {
            [108.0, 150.0, 116.0]
        };
        let shelf_count = if snapshot.recent().is_empty() {
            1.0
        } else {
            2.0
        };
        let available = app_safe_height - padding * 2.0;
        let fitted_shelf = (available - (shelf_count - 1.0) * gap) / shelf_count;
        let fitted_width =
            (fitted_shelf - SHELF_CHROME_HEIGHT - app_tile::NAME_HEIGHT) * app_tile::IMAGE_RATIO;
        // Preserve a readable minimum while scrolling. Exceptionally short
        // outputs must still fit one complete selected card below its heading.
        let single_shelf_width = ((available - SHELF_CHROME_HEIGHT - app_tile::NAME_HEIGHT)
            * app_tile::IMAGE_RATIO)
            .floor()
            .max(1.0);
        let tile_width = fitted_width
            .floor()
            .clamp(256.0, 360.0)
            .min(main_width - SECTION_INSET * 2.0)
            .min(single_shelf_width);
        let shelf_height =
            tile_width / app_tile::IMAGE_RATIO + app_tile::NAME_HEIGHT + SHELF_CHROME_HEIGHT;
        Self {
            width,
            height,
            padding,
            gap,
            app_safe_height,
            main_width,
            action_bar_height,
            action_widths,
            shelf_height,
            bottom_inset,
        }
    }
}

struct FloatingControlsGeometry {
    actions: [Rect; 3],
    workspace: Option<Rect>,
    workspace_start: usize,
    workspace_content_width: f32,
}

impl FloatingControlsGeometry {
    fn resolve(layout: ConsoleLayout, snapshot: &ConsoleSnapshot, focus: &ConsoleState) -> Self {
        let y = layout.chrome_top() + layout.gap / 2.0;
        let inset = layout.padding + SECTION_INSET;
        let mut x = inset;
        let actions = layout.action_widths.map(|width| {
            let rect = Rect::from_xywh(x, y, width, ACTION_HEIGHT);
            x += width + ACTION_GAP;
            rect
        });
        let quick_width = layout.action_widths.iter().sum::<f32>() + 2.0 * ACTION_GAP;
        let available = (layout.main_width - 2.0 * SECTION_INSET - quick_width - 32.0).max(1.0);
        let count = snapshot.workspaces.len();
        let workspace_width = available
            .min((count as f32 * (WORKSPACE_BUTTON_WIDTH + ACTION_GAP) - ACTION_GAP).max(0.0));
        let selected = if focus.region == Region::Workspaces {
            focus.index
        } else {
            snapshot
                .workspaces
                .iter()
                .position(|ws| ws.id == snapshot.active_workspace)
                .unwrap_or(0)
        };
        let capacity = ((workspace_width + ACTION_GAP) / (WORKSPACE_BUTTON_WIDTH + ACTION_GAP))
            .floor()
            .max(1.0) as usize;
        let workspace_start = selected / capacity * capacity;
        let workspace_content_width = (count.saturating_sub(workspace_start) as f32
            * (WORKSPACE_BUTTON_WIDTH + ACTION_GAP)
            - ACTION_GAP)
            .max(0.0);
        // Keep a partial final page anchored to the right edge.
        let workspace_width = workspace_width.min(workspace_content_width);
        let workspace = (count > 0).then(|| {
            Rect::from_xywh(
                layout.width - inset - workspace_width,
                y,
                workspace_width,
                ACTION_HEIGHT,
            )
        });
        Self {
            actions,
            workspace,
            workspace_start,
            workspace_content_width,
        }
    }
}

fn boxed(view: impl View + Clone + 'static) -> Box<dyn View> {
    Box::new(view)
}

#[derive(Clone)]
struct ViewList(Vec<Box<dyn View>>);

impl ViewTuple for ViewList {
    fn create_elements(&self) -> Vec<Box<dyn Element>> {
        self.0.iter().map(|view| view.create_element()).collect()
    }
    fn clone_views(&self) -> Vec<Box<dyn View>> {
        self.0.clone()
    }
    fn collect_listenables<'a>(
        &'a self,
        collector: &mut Vec<&'a dyn scarlet_ui::state::Listenable>,
    ) {
        for view in &self.0 {
            collector.extend(view.listenables());
        }
    }
}

fn row(views: Vec<Box<dyn View>>, gap: f32) -> impl View + Clone {
    HStack::new(ViewList(views))
        .spacing(gap)
        .alignment(Alignment::TopLeading)
}

fn column(views: Vec<Box<dyn View>>, gap: f32) -> impl View + Clone {
    VStack::new(ViewList(views))
        .spacing(gap)
        .alignment(Alignment::TopLeading)
}

fn text(label: impl Into<String>, size: f32, color: Color) -> Text {
    Text::new(label.into()).font_size(size).color(color)
}

/// Positions native floating controls without an invisible full-screen hit
/// target. Empty space between controls belongs to the ScrollView below.
#[derive(Clone)]
struct FloatingLayer {
    size: Size,
    children: Vec<(Point, Size, Box<dyn View>)>,
}

impl FloatingLayer {
    fn in_bottom_window(mut self, layout: ConsoleLayout) -> Self {
        for (position, _, _) in &mut self.children {
            position.y -= layout.chrome_top();
        }
        self.size.height = layout.chrome_height();
        self
    }
}

impl View for FloatingLayer {
    fn create_element(&self) -> Box<dyn Element> {
        Box::new(RenderElement::with_view_children(
            self.clone(),
            |view| FloatingLayerLayout {
                size: view.size,
                positions: view
                    .children
                    .iter()
                    .map(|(position, _, _)| *position)
                    .collect(),
            },
            |view| {
                view.children
                    .iter()
                    .map(|(_, _, child)| child.clone_view())
                    .collect()
            },
        ))
    }

    fn listenables(&self) -> Vec<&dyn scarlet_ui::state::Listenable> {
        self.children
            .iter()
            .flat_map(|(_, _, child)| child.listenables())
            .collect()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct FloatingLayerLayout {
    size: Size,
    positions: Vec<Point>,
}

impl ElementRenderObject for FloatingLayerLayout {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        constraints.constrain(self.size)
    }

    fn layout_with_children(
        &mut self,
        constraints: LayoutConstraints,
        children: &mut [Box<dyn Element>],
    ) -> Size {
        let size = self.layout(constraints);
        for (child, position) in children.iter_mut().zip(&self.positions) {
            child.layout(LayoutConstraints::new(0.0, size.width, 0.0, size.height));
            child.set_position(*position);
        }
        size
    }

    fn size(&self) -> Size {
        self.size
    }
    fn hit_test(&self, _point: Point) -> bool {
        false
    }
    fn render(&mut self) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn update(&mut self, view: &dyn View) -> UpdateResult {
        let Some(view) = view.as_any().downcast_ref::<FloatingLayer>() else {
            return UpdateResult::Replaced;
        };
        let positions: Vec<_> = view
            .children
            .iter()
            .map(|(position, _, _)| *position)
            .collect();
        if self.size == view.size && self.positions == positions {
            return UpdateResult::NoChange;
        }
        self.size = view.size;
        self.positions = positions;
        UpdateResult::Updated
    }
}

#[derive(Clone)]
struct ConsoleView {
    snapshot: ConsoleSnapshot,
    state: State<ConsoleState>,
    action: Arc<dyn Fn(ConsoleAction)>,
    part: ConsolePart,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConsolePart {
    Full,
    Applications,
    Chrome,
}

impl ConsoleView {
    fn select(&self, region: Region, index: usize, activate: bool) {
        let mut next = self.state.get();
        next.focus(region, index);
        let action = activate.then(|| next.activate(&self.snapshot)).flatten();
        if self.state.get() != next {
            self.state.set(next);
        }
        if let Some(action) = action {
            (self.action)(action);
        }
    }

    fn action_button(
        &self,
        label: &str,
        icon: Option<Icon>,
        region: Region,
        index: usize,
        width: f32,
        active: bool,
    ) -> impl View + Clone + use<> {
        let focus = self.state.get();
        let selected = focus.region == region && focus.index == index;
        let click = self.clone();
        let frame_click = self.clone();
        let mut button = Button::new(label)
            .header_style()
            .font_size(if region == Region::Workspaces {
                16.0
            } else {
                18.0
            })
            .text_color(TEXT)
            .on_click(move || click.select(region, index, true));
        if let Some(icon) = icon {
            button = button
                .icon(icon)
                .icon_size(IconSize::Pixels(20))
                .icon_color(if region == Region::Quick && index == 0 {
                    ACCENT
                } else {
                    TEXT
                });
        }
        // Button keeps its intrinsic size. Center that actual control inside
        // the shared 44px target instead of pinning it to the frame's origin.
        Surface::floating(
            ZStack::new((button,))
                .alignment(Alignment::Center)
                .frame(width, ACTION_HEIGHT),
        )
        .elevation(ElevationRole::Flat)
        .fill(if region == Region::Workspaces {
            if active {
                ACCENT.with_opacity(0.16)
            } else {
                Color::WHITE.with_opacity(if selected { 0.10 } else { 0.0 })
            }
        } else if selected || active {
            FLOATING_ACTIVE
        } else {
            FLOATING
        })
        .bordered(false)
        .corner_radius(10.0)
        .border_rounded(
            if selected && region == Region::Workspaces {
                TEXT
            } else if selected || active {
                ACCENT
            } else {
                LINE
            },
            if selected { 2.0 } else { 1.0 },
            10.0,
        )
        .on_click(move || frame_click.select(region, index, true))
    }

    fn floating_controls(&self, layout: ConsoleLayout) -> FloatingLayer {
        let compact = layout.width < 800.0;
        let volume = if self.snapshot.muted == Some(true) {
            String::from("Muted")
        } else {
            self.snapshot.volume.map_or(String::from("Volume —"), |v| {
                if compact {
                    format!("{v}%")
                } else {
                    format!("Volume {v}%")
                }
            })
        };
        let labels = ["Power", volume.as_str(), "Settings"];
        let icons = [Icon::Power, Icon::Volume2, Icon::Settings];
        let geometry = FloatingControlsGeometry::resolve(layout, &self.snapshot, &self.state.get());
        let mut children: Vec<_> = geometry
            .actions
            .iter()
            .enumerate()
            .map(|(index, rect)| {
                (
                    rect.origin,
                    rect.size,
                    boxed(self.action_button(
                        if layout.width < 600.0 {
                            ""
                        } else {
                            labels[index]
                        },
                        Some(icons[index]),
                        Region::Quick,
                        index,
                        rect.size.width,
                        false,
                    )),
                )
            })
            .collect();
        let start = geometry.workspace_start;
        let remaining_width = geometry.workspace_content_width;
        let workspace_width = geometry.workspace.map_or(0.0, |rect| rect.size.width);
        let workspaces = self
            .snapshot
            .workspaces
            .iter()
            .enumerate()
            .skip(start)
            .map(|(index, ws)| {
                let active = ws.id == self.snapshot.active_workspace;
                boxed(self.action_button(
                    &format!("WS {}", index + 1),
                    None,
                    Region::Workspaces,
                    index,
                    WORKSPACE_BUTTON_WIDTH,
                    active,
                ))
            })
            .collect();
        let workspace_rail = scarlet_ui::views::Keyed::new(
            ScrollView::new(row(workspaces, ACTION_GAP))
                .horizontal()
                .scrollbar_visibility(ScrollbarVisibility::Never)
                .content_size(remaining_width, ACTION_HEIGHT)
                .frame(workspace_width, ACTION_HEIGHT),
            ViewKey::from(start as u64),
        );
        if let Some(rect) = geometry.workspace {
            children.push((
                rect.origin,
                rect.size,
                boxed(
                    Surface::floating(workspace_rail)
                        .elevation(ElevationRole::Flat)
                        .fill(FLOATING)
                        .bordered(false)
                        .corner_radius(10.0),
                ),
            ));
        }
        FloatingLayer {
            size: Size::new(layout.width, layout.height),
            children,
        }
    }

    fn rail(
        &self,
        title: &str,
        region: Region,
        apps: Vec<ApplicationTile>,
        width: f32,
        height: f32,
        content_inset: f32,
    ) -> impl View + Clone + use<> {
        let inner = width - content_inset * 2.0;
        let tile_width = ((height - SHELF_CHROME_HEIGHT - app_tile::NAME_HEIGHT)
            * app_tile::IMAGE_RATIO)
            .round()
            .max(1.0)
            .min(inner);
        let card_height = tile_width / app_tile::IMAGE_RATIO + app_tile::NAME_HEIGHT;
        let gap = 10.0;
        let capacity = ((inner + gap) / (tile_width + gap)).floor().max(1.0) as usize;
        let focus = self.state.get();
        let selected = if focus.region == region {
            focus.index
        } else {
            focus
                .remembered_app_index(region)
                .unwrap_or(0)
                .min(apps.len().saturating_sub(1))
        };
        let mut cards = Vec::new();
        for (index, app) in apps.iter().enumerate() {
            let selected = focus.region == region && focus.index == index;
            let click = self.clone();
            cards.push(boxed(
                app_tile::build(app, tile_width, selected)
                    .on_click(move || click.select(region, index, true))
                    .key(app.app_id.clone()),
            ));
        }
        if cards.is_empty() {
            cards.push(boxed(
                text(
                    if region == Region::Recent {
                        "Apps used this session appear here"
                    } else {
                        "No applications available"
                    },
                    13.0,
                    MUTED,
                )
                .frame(inner, card_height),
            ));
        }
        let previous = self.clone();
        let next = self.clone();
        let previous_index = selected.saturating_sub(capacity);
        let next_index = (selected + capacity).min(apps.len().saturating_sub(1));
        let controls = row(
            vec![
                boxed(
                    text(title, 21.0, TEXT)
                        .frame((inner - 90.0).max(1.0), SECTION_HEADER_HEIGHT)
                        .clip(),
                ),
                boxed(
                    text("‹", 22.0, MUTED)
                        .alignment(Alignment::Center)
                        .frame(32.0, 24.0)
                        .on_click(move || previous.select(region, previous_index, false)),
                ),
                boxed(
                    text("›", 22.0, MUTED)
                        .alignment(Alignment::Center)
                        .frame(32.0, 24.0)
                        .on_click(move || next.select(region, next_index, false)),
                ),
            ],
            8.0,
        );
        // Keep the shelf mounted while revealing keyboard selection. Focus
        // changes between shelves retain both horizontal offsets and artwork.
        let content_width = (cards.len() as f32 * (tile_width + gap) - gap).max(inner);
        // Pad the scrolling content, not the viewport: artwork should leave
        // the screen at its edge rather than at an inset clipping rectangle.
        let rail = ScrollView::new(
            row(cards, gap)
                .frame(content_width, card_height)
                .padding_insets(EdgeInsets::symmetric(0.0, content_inset)),
        )
        .horizontal()
        .scrollbar_visibility(ScrollbarVisibility::Never)
        .scroll_to_horizontal_range(
            selected as f32 * (tile_width + gap),
            selected as f32 * (tile_width + gap) + tile_width + content_inset * 2.0,
        )
        .content_size(content_width + content_inset * 2.0, card_height)
        .frame(width, card_height);
        column(
            vec![
                boxed(controls.padding_insets(EdgeInsets::symmetric(0.0, content_inset))),
                boxed(rail),
            ],
            SECTION_GAP,
        )
        .padding_insets(EdgeInsets::symmetric(SECTION_INSET, 0.0))
        .frame(width, height)
    }

    fn controls(&self, width: f32, height: f32) -> impl View + Clone + use<> {
        let state = self.state.get();
        let inner = (width - 48.0).min(720.0);
        let labels = if state.armed.is_some() {
            vec![String::from("Cancel"), String::from("Confirm")]
        } else {
            vec![
                String::from("Volume −"),
                String::from("Volume +"),
                String::from(if self.snapshot.muted == Some(true) {
                    "Unmute"
                } else {
                    "Mute"
                }),
                String::from("Open Settings"),
                String::from("Restart"),
                String::from("Power Off"),
                String::from("Back"),
            ]
        };
        let buttons = labels
            .iter()
            .enumerate()
            .map(|(index, label)| {
                let enabled = state.armed.is_some()
                    || match index {
                        0 | 1 => self.snapshot.volume.is_some(),
                        2 => self.snapshot.muted.is_some(),
                        _ => true,
                    };
                let click = self.clone();
                let activate = move || {
                    let mut next = click.state.get();
                    next.control_index = index;
                    let action = next.activate_control(&click.snapshot);
                    click.state.set(next);
                    if let Some(action) = action {
                        (click.action)(action);
                    }
                };
                boxed(
                    ZStack::new((Button::new(label)
                        .header_style()
                        .font_size(20.0)
                        .text_color(if enabled { TEXT } else { MUTED })
                        .on_click(activate.clone()),))
                    .frame((inner - 14.0) / 2.0, 64.0)
                    .background(CARD)
                    .clip_radius(14.0)
                    .border_rounded(
                        if index == state.control_index {
                            ACCENT
                        } else {
                            LINE
                        },
                        2.0,
                        14.0,
                    )
                    .on_click(activate),
                )
            })
            .collect::<Vec<_>>();
        let title = match state.armed {
            Some(PowerAction::Reboot) => "Restart this system?",
            Some(PowerAction::PowerOff) => "Power off this system?",
            None => "Control Center",
        };
        let detail = if state.armed.is_some() {
            String::from("Save your work before continuing.")
        } else {
            self.snapshot
                .volume
                .map_or(String::from("Audio service unavailable"), |v| {
                    format!("Volume {v}%  ·  Enter to select, Esc to return")
                })
        };
        let mut rows = vec![boxed(
            column(
                vec![
                    boxed(text(title, 30.0, TEXT)),
                    boxed(text(detail, 15.0, MUTED).frame(inner, 26.0).clip()),
                ],
                8.0,
            )
            .frame(inner, 80.0),
        )];
        for pair in buttons.chunks(2) {
            rows.push(boxed(row(pair.to_vec(), 14.0).frame(inner, 80.0)));
        }
        let content_height = rows.len() as f32 * 80.0;
        Surface::section(
            ScrollView::new(column(rows, 0.0))
                .scrollbar_visibility(ScrollbarVisibility::Never)
                .content_size(width - 48.0, content_height)
                .scroll_to_index(Some(state.control_index / 2 + 1), 80.0)
                .frame(width - 48.0, (height - 48.0).max(1.0))
                .padding(24.0),
        )
        .fill(Color::rgb(34, 34, 36))
        .border_color(LINE)
        .corner_radius(RADIUS)
        .frame(width, height)
    }

    fn content(&self) -> impl View + Clone + use<> {
        let layout = ConsoleLayout::resolve(&self.snapshot);
        let shelf_height = layout.shelf_height;
        let shelf_extent = shelf_height + layout.gap;
        let recent = self.snapshot.recent();
        let has_recent = !recent.is_empty();
        let shelf_count = if has_recent { 2.0 } else { 1.0 };
        let mut sections = Vec::new();
        if has_recent {
            sections.push(boxed(self.rail(
                "Recently Used",
                Region::Recent,
                recent,
                layout.width,
                shelf_height,
                layout.padding + SECTION_INSET,
            )));
        }
        sections.push(boxed(self.rail(
            "Library",
            Region::Library,
            self.snapshot.applications.clone(),
            layout.width,
            shelf_height,
            layout.padding + SECTION_INSET,
        )));
        let content_height = shelf_count * shelf_height + (shelf_count - 1.0) * layout.gap;
        // Keep revealing the last application shelf while focus is on the
        // floating controls. Entering controls must not jump the app scroll.
        let focus = self.state.get();
        let region = match focus.region {
            Region::Recent | Region::Library => focus.region,
            _ => focus.app_focus.map_or(
                if has_recent {
                    Region::Recent
                } else {
                    Region::Library
                },
                |(region, _)| region,
            ),
        };
        let top_clearance = self.snapshot.bar_height + layout.padding;
        let bottom_clearance = layout.action_bar_height + layout.bottom_inset + layout.padding;
        let focus_top = top_clearance
            + if has_recent && region == Region::Library {
                shelf_extent
            } else {
                0.0
            };
        let app_scroll = ScrollView::new(column(
            vec![
                boxed(Spacer::new().frame(layout.width, top_clearance)),
                boxed(column(sections, layout.gap)),
                boxed(Spacer::new().frame(layout.width, bottom_clearance)),
            ],
            0.0,
        ))
        .scrollbar_visibility(ScrollbarVisibility::Never)
        .content_size(
            layout.width,
            content_height + top_clearance + bottom_clearance,
        )
        // Reveal the selected shelf within the unobscured area while keeping
        // the actual viewport full-screen for manual scrolling behind chrome.
        .scroll_to_vertical_range(
            focus_top - top_clearance,
            focus_top + shelf_height + bottom_clearance,
        )
        .frame(layout.width, layout.height)
        .background(BACKGROUND);
        let key_view = self.clone();
        let mut layers = Vec::new();
        if self.part != ConsolePart::Chrome {
            layers.push(boxed(app_scroll));
        }
        if self.part == ConsolePart::Full {
            layers.push(boxed(self.floating_controls(layout)));
        } else if self.part == ConsolePart::Chrome && !focus.control_open {
            layers.push(boxed(
                self.floating_controls(layout).in_bottom_window(layout),
            ));
        }
        if self.part != ConsolePart::Chrome
            && let Some(error) = &self.snapshot.launch_error
        {
            // Errors remain visible when needed; ordinary navigation has no
            // permanent guide or footer occupying the app area.
            let (text_width, _) = scarlet_ui::graphics::measure_text_sized(error, 13.0);
            let width = (text_width as f32 + 32.0).min(layout.width - layout.padding * 2.0);
            layers.push(boxed(FloatingLayer {
                size: Size::new(layout.width, layout.height),
                children: vec![(
                    Point::new((layout.width - width) / 2.0, layout.chrome_top() - 44.0),
                    Size::new(width, 36.0),
                    boxed(
                        Surface::floating(
                            text(error, 13.0, ACCENT)
                                .alignment(Alignment::Center)
                                .frame(width, 36.0)
                                .clip(),
                        )
                        .fill(Color::rgb_f32(0.13, 0.13, 0.14))
                        .elevation(ElevationRole::Flat)
                        .corner_radius(10.0)
                        .bordered(false),
                    ),
                )],
            }));
        }
        if self.state.get().control_open && self.part != ConsolePart::Chrome {
            let close = self.state.clone();
            layers.push(boxed(
                Spacer::new()
                    .frame(layout.width, layout.height)
                    .background(Color::BLACK.with_opacity(0.42))
                    .on_click(move || close.update(ConsoleState::close_controls)),
            ));
            let pane_width = if layout.width >= 900.0 {
                440.0
            } else {
                layout.width - 2.0 * layout.padding
            };
            let pane = column(
                vec![
                    boxed(
                        Spacer::new().frame(pane_width, self.snapshot.bar_height + layout.padding),
                    ),
                    boxed(self.controls(
                        pane_width,
                        layout.height - self.snapshot.bar_height - 2.0 * layout.padding,
                    )),
                ],
                0.0,
            );
            layers.push(boxed(row(
                vec![
                    boxed(
                        Spacer::new()
                            .frame(layout.width - pane_width - layout.padding, layout.height),
                    ),
                    boxed(pane),
                ],
                0.0,
            )));
        }
        let height = if self.part == ConsolePart::Chrome {
            layout.chrome_height()
        } else {
            layout.height
        };
        ZStack::new(ViewList(layers))
            .alignment(Alignment::TopLeading)
            .frame(layout.width, height)
            // Retain the tint and floating chrome in the same ordered graph
            // as the ScrollView's cached content. Warm scrolling composites
            // the overlapping layers without repainting application artwork
            // or dropping the front controls.
            .repaint_boundary()
            .on_key(move |event| {
                let mut next = key_view.state.get();
                let (handled, action) = next.handle_key(event, &key_view.snapshot);
                if key_view.state.get() != next {
                    key_view.state.set(next);
                }
                if let Some(action) = action {
                    (key_view.action)(action);
                }
                handled
            })
    }
}

pub fn build_console_view(
    snapshot: ConsoleSnapshot,
    state: State<ConsoleState>,
    action: impl Fn(ConsoleAction) + 'static,
) -> impl View + Clone {
    build_console_part(snapshot, state, action, ConsolePart::Full)
}

pub fn build_console_part(
    snapshot: ConsoleSnapshot,
    state: State<ConsoleState>,
    action: impl Fn(ConsoleAction) + 'static,
    part: ConsolePart,
) -> impl View + Clone {
    // Catalog refreshes can remove the selected item between input events.
    // Clamp before building a horizontal page so available apps remain visible.
    let mut next = state.get();
    // Preserve the initial app region while the catalog worker is still
    // loading. Once apps arrive, an empty Recent shelf falls back to Library.
    if snapshot.count(next.region) > 0 || !snapshot.applications.is_empty() {
        next.normalize(&snapshot);
    }
    if state.get() != next {
        state.set(next);
    }
    ConsoleView {
        snapshot,
        state,
        action: Arc::new(action),
        part,
    }
    .content()
}

pub fn record_recent(recent: &mut Vec<String>, app_id: &str) {
    recent.retain(|id| id != app_id);
    recent.insert(0, app_id.to_owned());
    recent.truncate(20);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> ConsoleSnapshot {
        ConsoleSnapshot {
            width: 1280.0,
            height: 720.0,
            bar_height: 32.0,
            applications: (0..15)
                .map(|index| ApplicationTile {
                    app_id: format!("app-{index}"),
                    name: format!("App {index}"),
                    icon: Icon::Terminal,
                    color: CARD,
                    artwork: AppArtwork::default(),
                })
                .collect(),
            recent_ids: vec![
                String::from("app-2"),
                String::from("missing-app"),
                String::from("app-1"),
            ],
            volume: Some(98),
            muted: Some(false),
            ..ConsoleSnapshot::default()
        }
    }

    fn key(keycode: KeyCode) -> KeyEvent {
        KeyEvent::Pressed {
            keycode,
            modifiers: scarlet_ui::event::KeyModifiers::empty(),
        }
    }

    #[test]
    fn navigation_launches_the_selected_app_beyond_the_first_page() {
        let snapshot = snapshot();
        let mut state = ConsoleState::default();
        assert_eq!(
            state.activate(&snapshot),
            Some(ConsoleAction::Launch(String::from("app-2")))
        );
        state.handle_key(key(KeyCode::Down), &snapshot);
        for _ in 0..10 {
            state.handle_key(key(KeyCode::Right), &snapshot);
        }
        let action = state.handle_key(key(KeyCode::Enter), &snapshot).1;
        assert_eq!(
            action,
            Some(ConsoleAction::Launch(
                snapshot.applications[10].app_id.clone()
            ))
        );
    }

    #[test]
    fn empty_history_starts_in_library_and_launch_returns_to_recent_apps() {
        let mut snapshot = snapshot();
        snapshot.recent_ids = vec![String::from("missing-app")];
        let mut state = ConsoleState::default();
        assert_eq!(
            state.activate(&snapshot),
            Some(ConsoleAction::Launch(String::from("app-0")))
        );
        assert_eq!(state.region, Region::Library);
        state.handle_key(key(KeyCode::Up), &snapshot);
        assert_eq!(state.region, Region::Library);

        record_recent(&mut snapshot.recent_ids, "app-7");
        state.did_launch();
        assert_eq!(state.region, Region::Recent);
        assert_eq!(
            state.activate(&snapshot),
            Some(ConsoleAction::Launch(String::from("app-7")))
        );
        state.handle_key(key(KeyCode::Down), &snapshot);
        assert_eq!(state.region, Region::Library);
        state.handle_key(key(KeyCode::Up), &snapshot);
        assert_eq!(state.region, Region::Recent);
        state.handle_key(key(KeyCode::Up), &snapshot);
        assert_eq!(state.region, Region::Recent);
    }

    #[test]
    fn stale_recent_ids_and_removed_selection_are_safe() {
        let mut snapshot = snapshot();
        assert_eq!(snapshot.recent().len(), 2);
        let mut state = ConsoleState {
            region: Region::Library,
            index: 100,
            ..ConsoleState::default()
        };
        snapshot.applications.clear();
        state.handle_key(key(KeyCode::Right), &snapshot);
        assert_eq!(state.region, Region::Quick);
        assert_eq!(state.index, 2);
        assert_eq!(
            state.handle_key(key(KeyCode::Enter), &snapshot).1,
            Some(ConsoleAction::Settings)
        );
    }

    #[test]
    fn application_shelves_keep_independent_positions_when_focus_moves_between_them() {
        let snapshot = snapshot();
        let mut state = ConsoleState::default();
        state.focus(Region::Recent, 1);
        state.focus(Region::Library, 7);
        state.move_focus(KeyCode::Up, &snapshot);
        assert_eq!((state.region, state.index), (Region::Recent, 1));
        assert_eq!(state.remembered_app_index(Region::Library), Some(7));
        state.move_focus(KeyCode::Left, &snapshot);
        state.move_focus(KeyCode::Down, &snapshot);
        assert_eq!((state.region, state.index), (Region::Library, 7));
        assert_eq!(state.remembered_app_index(Region::Recent), Some(0));
        state.move_focus(KeyCode::Down, &snapshot);
        state.move_focus(KeyCode::Up, &snapshot);
        assert_eq!((state.region, state.index), (Region::Library, 7));
        state.did_launch();
        state.move_focus(KeyCode::Down, &snapshot);
        assert_eq!((state.region, state.index), (Region::Library, 7));

        let mut shorter = snapshot;
        shorter.applications.truncate(3);
        state.move_focus(KeyCode::Up, &shorter);
        state.move_focus(KeyCode::Down, &shorter);
        assert_eq!((state.region, state.index), (Region::Library, 2));
    }

    #[test]
    fn bottom_navigation_is_consistent_across_shapes_and_restores_the_app() {
        for (width, height) in [
            (1280.0, 720.0),
            (900.0, 900.0),
            (640.0, 480.0),
            (320.0, 480.0),
        ] {
            let mut snapshot = snapshot();
            snapshot.width = width;
            snapshot.height = height;
            snapshot.workspaces = vec![
                WorkspaceTile {
                    id: 41,
                    window_count: 1,
                },
                WorkspaceTile {
                    id: 42,
                    window_count: 1,
                },
            ];
            snapshot.active_workspace = 41;
            let mut state = ConsoleState::default();
            for key in [KeyCode::Right, KeyCode::Down, KeyCode::Down] {
                state.move_focus(key, &snapshot);
            }
            assert_eq!((state.region, state.index), (Region::Quick, 0));
            assert_eq!(state.activate(&snapshot), None);
            assert!(state.control_open);
            assert_eq!(state.control_index, 5, "Power opens the power controls");
            assert_eq!(state.armed, None, "opening Power must not arm shutdown");
            state.close_controls();
            state.move_focus(KeyCode::Right, &snapshot);
            assert_eq!(state.activate(&snapshot), None);
            assert!(state.control_open);
            assert_eq!(state.control_index, 0, "Volume opens the audio controls");
            state.close_controls();
            state.move_focus(KeyCode::Right, &snapshot);
            assert_eq!(state.activate(&snapshot), Some(ConsoleAction::Settings));
            state.move_focus(KeyCode::Up, &snapshot);
            assert_eq!((state.region, state.index), (Region::Library, 1));
            state.move_focus(KeyCode::Down, &snapshot);
            assert_eq!((state.region, state.index), (Region::Quick, 2));
            state.move_focus(KeyCode::Right, &snapshot);
            assert_eq!(
                state.activate(&snapshot),
                Some(ConsoleAction::Workspace(41))
            );
            state.move_focus(KeyCode::Right, &snapshot);
            assert_eq!(
                state.activate(&snapshot),
                Some(ConsoleAction::Workspace(42))
            );
            state.move_focus(KeyCode::Up, &snapshot);
            assert_eq!((state.region, state.index), (Region::Library, 1));
            for _ in 0..2 {
                state.move_focus(KeyCode::Left, &snapshot);
            }
            assert_eq!(
                (state.region, state.index),
                (Region::Library, 0),
                "Left must stay in the app row"
            );
            state.move_focus(KeyCode::Down, &snapshot);
            state.move_focus(KeyCode::Right, &snapshot);
            state.move_focus(KeyCode::Left, &snapshot);
            assert_eq!((state.region, state.index), (Region::Quick, 2));
            snapshot.applications.clear();
            state.move_focus(KeyCode::Up, &snapshot);
            assert_eq!(
                state.region,
                Region::Quick,
                "empty catalogs keep system actions reachable"
            );
        }
    }

    #[test]
    fn recent_history_is_bounded_unique_and_orders_by_latest_use() {
        let mut recent = Vec::new();
        for index in 0..30 {
            record_recent(&mut recent, &format!("app-{index}"));
        }
        record_recent(&mut recent, "app-20");
        assert_eq!(recent.len(), 20);
        assert_eq!(recent[0], "app-20");
        assert_eq!(recent.iter().filter(|id| *id == "app-20").count(), 1);
    }

    #[test]
    fn printable_shortcuts_run_once_and_modal_keys_do_not_switch_workspaces() {
        let snapshot = snapshot();
        let mut state = ConsoleState::default();
        assert!(!state.handle_key(key(KeyCode::Char('m')), &snapshot).0);
        state.handle_key(KeyEvent::Char { c: 'm' }, &snapshot);
        assert!(state.control_open);
        assert!(!state.handle_key(KeyEvent::Char { c: 'q' }, &snapshot).0);
        state.handle_key(key(KeyCode::Escape), &snapshot);
        assert_eq!(
            state.handle_key(KeyEvent::Char { c: 'e' }, &snapshot).1,
            Some(ConsoleAction::CycleWorkspace(1))
        );
        assert_eq!(
            state.handle_key(key(KeyCode::Tab), &snapshot),
            (false, None)
        );
    }

    #[test]
    fn controls_use_real_audio_state_and_clamp_volume() {
        let mut snapshot = snapshot();
        let mut state = ConsoleState::default();
        state.open_controls();
        state.control_index = 1;
        assert_eq!(state.activate(&snapshot), Some(ConsoleAction::Volume(100)));
        snapshot.volume = None;
        snapshot.muted = None;
        assert_eq!(state.activate(&snapshot), None);
        state.control_index = 2;
        assert_eq!(state.activate(&snapshot), None);
    }

    #[test]
    fn power_requires_a_separate_confirm_selection_and_escape_cancels() {
        let snapshot = snapshot();
        let mut state = ConsoleState::default();
        state.open_controls();
        state.control_index = 5;
        assert_eq!(state.activate(&snapshot), None);
        assert_eq!(state.armed, Some(PowerAction::PowerOff));
        // Repeated Enter lands on Cancel, never on Confirm.
        assert_eq!(state.activate(&snapshot), None);
        assert_eq!(state.armed, None);
        state.control_index = 4;
        state.activate(&snapshot);
        state.handle_key(key(KeyCode::Right), &snapshot);
        assert_eq!(
            state.activate(&snapshot),
            Some(ConsoleAction::Power(PowerAction::Reboot))
        );
        state.control_index = 5;
        state.activate(&snapshot);
        state.handle_key(key(KeyCode::Escape), &snapshot);
        assert_eq!(state.armed, None);
        assert!(state.control_open);
    }

    #[test]
    fn floating_actions_and_application_safe_area_fit_every_output_shape() {
        for (width, height) in [
            (1280.0, 720.0),
            (1440.0, 900.0),
            (2560.0, 1080.0),
            (1024.0, 768.0),
            (900.0, 900.0),
            (640.0, 480.0),
            (320.0, 480.0),
            (320.0, 320.0),
            (1920.0, 320.0),
        ] {
            let layout = ConsoleLayout::resolve(&ConsoleSnapshot {
                width,
                height,
                bar_height: 32.0,
                ..snapshot()
            });
            assert_eq!(
                layout.app_safe_height + layout.action_bar_height + layout.bottom_inset + 32.0,
                height
            );
            assert_eq!(layout.main_width + 2.0 * layout.padding, width);
            assert!(
                layout.shelf_height <= layout.app_safe_height - 2.0 * layout.padding + 0.01,
                "one complete selected shelf must fit even on a short output"
            );
            let quick_width = layout.action_widths.iter().sum::<f32>() + 2.0 * ACTION_GAP;
            assert!(
                quick_width + 32.0 + WORKSPACE_BUTTON_WIDTH
                    <= layout.main_width - 2.0 * SECTION_INSET,
                "at least one workspace target must fit beside all system actions"
            );
            assert_eq!(layout.action_bar_height - layout.gap, 44.0);
        }
    }

    #[test]
    fn workspace_removal_and_resize_keep_focus_valid() {
        let mut snapshot = snapshot();
        snapshot.workspaces = (1..=12)
            .map(|id| WorkspaceTile {
                id,
                window_count: 1,
            })
            .collect();
        snapshot.active_workspace = 12;
        let mut state = ConsoleState::default();
        for key in [
            KeyCode::Down,
            KeyCode::Down,
            KeyCode::Right,
            KeyCode::Right,
            KeyCode::Right,
        ] {
            state.move_focus(key, &snapshot);
        }
        assert_eq!(
            state.activate(&snapshot),
            Some(ConsoleAction::Workspace(12))
        );
        snapshot.width = 320.0;
        snapshot.workspaces.truncate(1);
        state.move_focus(KeyCode::Right, &snapshot);
        assert_eq!(state.activate(&snapshot), Some(ConsoleAction::Workspace(1)));
        snapshot.workspaces.clear();
        state.move_focus(KeyCode::Left, &snapshot);
        assert_eq!((state.region, state.index), (Region::Quick, 0));
    }
}

/// The same geometry drives compositor materials and native pointer targeting.
pub fn chrome_regions(
    snapshot: ConsoleSnapshot,
    state: State<ConsoleState>,
) -> Vec<scarlet_ui::platform::SurfaceRegion> {
    if state.get().control_open {
        return Vec::new();
    }
    let layout = ConsoleLayout::resolve(&snapshot);
    let geometry = FloatingControlsGeometry::resolve(layout, &snapshot, &state.get());
    geometry
        .actions
        .into_iter()
        .chain(geometry.workspace)
        .map(|mut rect| {
            rect.origin.y -= layout.chrome_top();
            scarlet_ui::platform::SurfaceRegion {
                rect,
                corner_radius: 10.0,
                blur_radius: MATERIAL_BLUR_RADIUS,
                accepts_input: true,
            }
        })
        .collect()
}
