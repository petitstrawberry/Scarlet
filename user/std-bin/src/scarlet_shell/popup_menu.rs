//! Native menu surfaces use the same retained paint as the Web menu host.
//! SWS supplies the desktop backdrop; the client paints tint, rim and labels.

use scarlet_ui::buffer::Buffer;
use scarlet_ui::color::Color;
use scarlet_ui::element::ElementRenderObject;
use scarlet_ui::geometry::{Point, Rect, Size};
use scarlet_ui::graphics;
use scarlet_ui::renderer::{CpuPaintRenderer, PaintCommand, PaintContext};
use scarlet_ui::views::menu::MenuRenderObject;
use scarlet_ui::views::{MenuAction, MenuItemContent};
use std::time::{Duration, Instant};
use sws_client::{
    self as sws,
    event::{Event, InputEvent, abs_code, event_type, key_code},
};
use sws_protocol::{WindowGeometry, surface_regions, touch, window_types};

use super::{
    MenuTree, OVERVIEW_MENU_INDEX, OverviewGeometry, ShellWindow, SwsScreenConnection,
    TaskMenuEntry, TaskMenuItem, build_menu_items, build_overview_items,
    connect_sws_with_screen_size_retry, menu_bar_popup_x, multi_output, overview_app_menu_indices,
    overview_page_capacity, overview_page_count, scale_i32, scale_u32, unscale_u32,
};

const SUBMENU_HOVER_DELAY: Duration = Duration::from_millis(140);

struct PopupMenuRenderer {
    menu: MenuRenderObject,
    rows: Vec<Option<MenuAction>>,
    painter: CpuPaintRenderer,
    origin: Point,
    size: Size,
    scale_milli: u32,
}

impl PopupMenuRenderer {
    /// Build a menu at least `min_width` wide that grows to fit its widest
    /// label/shortcut row, capped at `max_width` (the usable screen width).
    pub fn new(
        items: Vec<MenuItemContent>,
        item_height: f32,
        min_width: f32,
        max_width: f32,
        scale_milli: u32,
    ) -> Self {
        graphics::set_current_scale_milli(scale_milli);
        // Measure with the same metrics the menu paints with so labels and
        // right-aligned shortcuts never overlap or get clipped.
        let preferred = MenuRenderObject::new(items.clone(), item_height, min_width)
            .preferred_width()
            .ceil();
        let width = preferred.max(min_width).min(max_width.max(min_width));
        // The explicit row height determines the geometry at construction.
        // Layout would also allocate a legacy raster buffer that paint never uses.
        let rows = items
            .iter()
            .map(|item| match item.action_value() {
                action if item.enabled_value() && action != MenuAction::Separator => Some(action),
                _ => None,
            })
            .collect();
        let menu = MenuRenderObject::new(items, item_height, width);
        let size = menu.size();
        let outsets = menu.paint_outsets();
        let origin = Point::new(outsets.left.ceil(), outsets.top.ceil());
        let surface_size = Size::new(
            origin.x + size.width.ceil() + outsets.right.ceil(),
            origin.y + size.height.ceil() + outsets.bottom.ceil(),
        );
        let painter = CpuPaintRenderer::new(surface_size, scale_milli, Color::TRANSPARENT);
        Self {
            menu,
            rows,
            painter,
            origin,
            size,
            scale_milli,
        }
    }

    /// Visible menu extent; shadow padding does not affect popup anchoring.
    pub fn size(&self) -> Size {
        self.size
    }

    pub fn physical_size(&self) -> (u32, u32) {
        (
            self.painter.buffer().width(),
            self.painter.buffer().height(),
        )
    }

    fn pixels(&self, value: f32) -> u32 {
        (value * self.scale_milli.max(1) as f32 / 1000.0)
            .round()
            .max(0.0) as u32
    }

    pub fn window_geometry(&self) -> WindowGeometry {
        WindowGeometry {
            x: self.pixels(self.origin.x) as i32,
            y: self.pixels(self.origin.y) as i32,
            width: self.pixels(self.size.width).max(1),
            height: self.pixels(self.size.height).max(1),
        }
    }

    /// Derive compositor clipping and diffusion from shared menu paint rather
    /// than maintaining separate native corner and material constants.
    pub fn surface_regions(&self) -> Vec<surface_regions::SurfaceRegion> {
        let mut context = PaintContext::new();
        self.menu.paint(&mut context, self.origin);
        context
            .commands()
            .iter()
            .filter_map(|command| {
                let (rect, corner, blur) = match command {
                    PaintCommand::BackdropFilterRoundedRect {
                        rect,
                        corner_radius,
                        blur_radius,
                        ..
                    } => (*rect, *corner_radius, *blur_radius),
                    // Tint-only quality still needs rounded input geometry.
                    PaintCommand::FillRoundedRect {
                        rect,
                        corner_radius,
                        ..
                    } if rect.origin == self.origin && rect.size == self.size => {
                        (*rect, *corner_radius, 0.0)
                    }
                    _ => return None,
                };
                let width = self.pixels(rect.size.width).max(1);
                let height = self.pixels(rect.size.height).max(1);
                Some(surface_regions::SurfaceRegion {
                    x: self.pixels(rect.origin.x) as i32,
                    y: self.pixels(rect.origin.y) as i32,
                    width,
                    height,
                    corner_radius: self.pixels(corner).min(width.min(height) / 2),
                    blur_radius: self.pixels(blur).min(64),
                    flags: surface_regions::INPUT
                        | if blur > 0.0 {
                            surface_regions::BACKDROP
                        } else {
                            0
                        },
                })
            })
            .collect()
    }

    fn row_at(&self, point: Point) -> Option<usize> {
        self.menu
            .hit_test(point.x - self.origin.x, point.y - self.origin.y)
            .filter(|&row| self.selectable(row))
    }

    fn selectable(&self, row: usize) -> bool {
        self.rows.get(row).is_some_and(Option::is_some)
    }

    fn set_highlight(&mut self, hovered: Option<usize>) -> bool {
        if hovered != self.menu.hovered() {
            self.menu.set_hovered(hovered);
            true
        } else {
            false
        }
    }

    fn activate(&self, row: usize) {
        self.menu.invoke_item(row);
    }

    pub fn render(&mut self) {
        graphics::set_current_scale_milli(self.scale_milli);
        let mut context = PaintContext::new();
        self.menu.paint(&mut context, self.origin);
        self.painter.execute(&context);
    }

    fn buffer(&self) -> &Buffer {
        self.painter.buffer()
    }
}

#[derive(Clone, Copy)]
struct MenuDisplay {
    size: Size,
    origin: (i32, i32),
    scale: u32,
    touch: bool,
}

impl MenuDisplay {
    fn physical_position(self, point: Point) -> (i32, i32) {
        (
            scale_i32(
                self.origin.0.saturating_add(point.x.round() as i32),
                self.scale,
            ),
            scale_i32(
                self.origin.1.saturating_add(point.y.round() as i32),
                self.scale,
            ),
        )
    }

    fn logical(self, value: i32) -> f32 {
        value as f32 * 1000.0 / self.scale.max(1) as f32
    }

    fn renderer(self, items: Vec<MenuItemContent>, model: &MenuModel<'_>) -> PopupMenuRenderer {
        let available = (self.size.width - 16.0).max(1.0);
        let preferred = if model.overview() {
            420.0
        } else if model.scarlet_menu() {
            176.0
        } else {
            220.0
        };
        PopupMenuRenderer::new(
            items,
            OverviewGeometry::for_layout(model.endpoint.layout.get()).row_height,
            available.min(preferred),
            available,
            self.scale,
        )
    }
}

struct MenuPane {
    id: u32,
    position: Point,
    renderer: PopupMenuRenderer,
    capacity: (u32, u32),
    uploaded: (u32, u32),
    pointer: Point,
    pending_motion: bool,
    row: Option<usize>,
    dirty: bool,
}

impl MenuPane {
    fn create(
        conn: &sws::Connection,
        display: MenuDisplay,
        renderer: PopupMenuRenderer,
        position: Point,
        root: bool,
    ) -> Result<Self, sws::Error> {
        let (width, height) = renderer.physical_size();
        // Most adjacent menu-bar items fit in this backing. Its excess area
        // stays transparent and is excluded from hit testing and damage.
        let capacity = if root {
            (
                width.max(scale_u32(256.min(display.size.width as u32), display.scale)),
                height.max(scale_u32(
                    280.min(display.size.height as u32),
                    display.scale,
                )),
            )
        } else {
            (width, height)
        };
        let (x, y) = display.physical_position(position);
        let id = conn.create_surface_with_type_and_policies_at(
            "org.scarlet-os.popup.menu",
            "Menu",
            "",
            capacity.0,
            capacity.1,
            window_types::ALWAYS_ON_TOP,
            false,
            true,
            false,
            x,
            y,
        )?;
        let pane = Self {
            id,
            position,
            renderer,
            capacity,
            uploaded: (0, 0),
            pointer: Point::new(-1.0, -1.0),
            pending_motion: false,
            row: None,
            dirty: true,
        };
        let _ = conn.set_window_has_alpha_content(id, true);
        pane.configure_regions(conn);
        if display.touch {
            let _ = conn.set_touch_input(id, true);
        }
        Ok(pane)
    }

    fn configure_regions(&self, conn: &sws::Connection) {
        let _ = conn.set_window_geometry(self.id, self.renderer.window_geometry());
        let _ = conn.set_surface_regions(self.id, true, &self.renderer.surface_regions());
    }

    fn replace_root(
        &mut self,
        conn: &sws::Connection,
        display: MenuDisplay,
        renderer: PopupMenuRenderer,
        position: Point,
    ) -> Result<(), sws::Error> {
        let (width, height) = renderer.physical_size();
        if width > self.capacity.0 || height > self.capacity.1 {
            let capacity = (self.capacity.0.max(width), self.capacity.1.max(height));
            conn.resize_window(self.id, capacity.0, capacity.1)?;
            self.capacity = capacity;
            self.uploaded = (0, 0);
        }
        self.renderer = renderer;
        self.pointer = Point::new(-1.0, -1.0);
        self.pending_motion = false;
        self.row = None;
        self.dirty = true;
        self.configure_regions(conn);
        if position != self.position {
            let (x, y) = display.physical_position(position);
            conn.move_window(self.id, x, y)?;
            self.position = position;
        }
        Ok(())
    }

    fn visible_bounds(&self) -> Rect {
        let origin = Point::new(
            self.position.x + self.renderer.origin.x,
            self.position.y + self.renderer.origin.y,
        );
        Rect::new(origin, self.renderer.size())
    }

    fn repaint(&mut self, conn: &sws::Connection) {
        self.renderer.render();
        let buffer = self.renderer.buffer();
        let (width, height) = (buffer.width(), buffer.height());
        let damage = (width.max(self.uploaded.0), height.max(self.uploaded.1));
        let pixels = buffer.as_slice();
        // SAFETY: The byte view covers exactly the initialized BGRA pixels.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                pixels.as_ptr().cast::<u8>(),
                core::mem::size_of_val(pixels),
            )
        };
        let copied = conn.with_surface_mut(self.id, |surface| {
            surface.with_buffer(|dst, stride, rows| {
                let stride = stride as usize * 4;
                let source_stride = width as usize * 4;
                let damage_stride = stride.min(damage.0 as usize * 4);
                let copy_stride = source_stride.min(damage_stride);
                for y in 0..rows.min(damage.1) as usize {
                    let row = &mut dst[y * stride..y * stride + damage_stride];
                    let copied = if y < height as usize {
                        let source_y = y * source_stride;
                        row[..copy_stride]
                            .copy_from_slice(&bytes[source_y..source_y + copy_stride]);
                        copy_stride
                    } else {
                        0
                    };
                    row[copied..].fill(0);
                }
            });
        });
        if copied.is_some()
            && conn
                .commit_region(self.id, 0, 0, damage.0, damage.1)
                .is_ok()
        {
            self.uploaded = (width, height);
            self.dirty = false;
        }
    }
}

struct MenuModel<'a> {
    endpoint: &'a multi_output::Endpoint,
    tree: &'a MenuTree,
    index: usize,
    active_window: u32,
}

impl MenuModel<'_> {
    fn overview(&self) -> bool {
        self.index == OVERVIEW_MENU_INDEX
    }

    fn root(&self) -> Option<&TaskMenuItem> {
        self.tree.items.get(self.index)
    }

    fn scarlet_menu(&self) -> bool {
        self.index == 0 && self.root().is_some_and(|item| item.id == "system_scarlet")
    }

    fn items(&self, entries: &[TaskMenuEntry]) -> Vec<MenuItemContent> {
        build_menu_items(
            entries,
            self.active_window,
            self.endpoint,
            self.scarlet_menu(),
        )
    }

    fn root_items(
        &self,
        windows: &[ShellWindow],
        page: usize,
        height: u32,
    ) -> Vec<MenuItemContent> {
        if !self.overview() {
            return self
                .root()
                .map_or_else(Vec::new, |root| self.items(&root.children));
        }
        let capacity = overview_page_capacity(height, self.endpoint.layout.get());
        let count = overview_app_menu_indices(self.tree)
            .len()
            .saturating_add(windows.len());
        let page = page.min(overview_page_count(count, capacity) - 1);
        super::set_state_if_changed(&self.endpoint.overview_page, page);
        build_overview_items(
            windows,
            self.tree,
            page,
            capacity,
            self.endpoint.overview_page.clone(),
            self.endpoint.open_menu.clone(),
        )
    }

    fn submenu(&self, path: &[usize]) -> Option<&[TaskMenuEntry]> {
        let mut node = self.root()?;
        for &row in path {
            let TaskMenuEntry::Item(item) = node.children.get(row)? else {
                return None;
            };
            if !item.enabled || item.children.is_empty() {
                return None;
            }
            node = item;
        }
        Some(&node.children)
    }
}

fn submenu_origin(parent: Rect, row_y: f32, child: Size, screen: Size) -> Point {
    let right = parent.right() - 1.0;
    let left = parent.left() - child.width + 1.0;
    let x = if right + child.width <= screen.width
        || (left < 0.0 && screen.width - parent.right() >= parent.left())
    {
        right
    } else {
        left
    };
    Point::new(
        x.clamp(0.0, (screen.width - child.width).max(0.0)),
        row_y.clamp(0.0, (screen.height - child.height).max(0.0)),
    )
}

#[derive(Default)]
struct MenuCascade {
    panes: Vec<MenuPane>,
    // Each index is the row in one pane that owns the next pane.
    path: Vec<usize>,
    focus_observed: bool,
    touch: Option<(u32, u64)>,
    preview: Option<(usize, usize, Instant)>,
}

impl MenuCascade {
    fn publish_surfaces(&self, endpoint: &multi_output::Endpoint) {
        super::set_state_if_changed(
            &endpoint.menu_surfaces,
            self.panes.iter().map(|pane| pane.id).collect(),
        );
    }

    fn trim(&mut self, conn: &sws::Connection, count: usize) -> bool {
        self.preview = None;
        let mut removed = false;
        while self.panes.len() > count {
            let pane = self.panes.pop().unwrap();
            if self.touch.is_some_and(|(id, _)| id == pane.id) {
                self.touch = None;
            }
            let _ = conn.destroy_surface(pane.id);
            removed = true;
        }
        self.path.truncate(count.saturating_sub(1));
        if removed {
            self.focus_observed = false;
            self.refresh_highlights();
        }
        removed
    }

    fn focus_leaf(&self, conn: &sws::Connection) {
        if let Some(pane) = self.panes.last() {
            let _ = conn.focus_window(pane.id);
        }
    }

    fn refresh_highlights(&mut self) {
        for (depth, pane) in self.panes.iter_mut().enumerate() {
            pane.dirty |= pane
                .renderer
                .set_highlight(pane.row.or(self.path.get(depth).copied()));
        }
    }

    fn set_root(
        &mut self,
        conn: &sws::Connection,
        display: MenuDisplay,
        model: &MenuModel<'_>,
        items: Vec<MenuItemContent>,
    ) -> Result<(), sws::Error> {
        let trimmed = self.trim(conn, 1);
        self.touch = None;
        let renderer = display.renderer(items, model);
        let layout = model.endpoint.layout.get();
        let x = if model.overview() {
            8.0
        } else {
            menu_bar_popup_x(&model.tree.items, model.index, layout)
        };
        let position = Point::new(
            x.min((display.size.width - renderer.size().width).max(0.0)),
            display.logical(layout.physical_popup_y(display.scale)),
        );
        if let Some(root) = self.panes.first_mut() {
            root.replace_root(conn, display, renderer, position)?;
            if trimmed {
                self.focus_leaf(conn);
            }
        } else {
            self.panes
                .push(MenuPane::create(conn, display, renderer, position, true)?);
            self.focus_leaf(conn);
        }
        self.publish_surfaces(model.endpoint);
        Ok(())
    }

    fn select_row(
        &mut self,
        conn: &sws::Connection,
        display: MenuDisplay,
        model: &MenuModel<'_>,
        depth: usize,
        row: usize,
        activate: bool,
    ) {
        self.preview = None;
        let pane = &self.panes[depth];
        let Some(Some(action)) = pane.renderer.rows.get(row) else {
            return;
        };
        if *action != MenuAction::Submenu {
            if activate {
                pane.renderer.activate(row);
            } else if self.trim(conn, depth + 1) {
                self.focus_leaf(conn);
                self.publish_surfaces(model.endpoint);
            }
            return;
        }
        if self.path.get(depth) == Some(&row) {
            return;
        }
        let mut path = self.path[..depth].to_vec();
        path.push(row);
        let Some(entries) = model.submenu(&path) else {
            return;
        };
        let Some(anchor) = pane.renderer.menu.item_rect(row) else {
            return;
        };
        let bounds = pane.visible_bounds();
        let renderer = display.renderer(model.items(entries), model);
        let first_row_top = renderer.menu.item_rect(0).map_or(0.0, |row| row.top());
        let body_origin = submenu_origin(
            bounds,
            bounds.top() + anchor.top() - first_row_top,
            renderer.size(),
            display.size,
        );
        let position = Point::new(
            body_origin.x - renderer.origin.x,
            body_origin.y - renderer.origin.y,
        );
        if let Ok(child) = MenuPane::create(conn, display, renderer, position, false) {
            self.trim(conn, depth + 1);
            self.path = path;
            self.panes.push(child);
            self.focus_observed = false;
            self.refresh_highlights();
            self.focus_leaf(conn);
            self.publish_surfaces(model.endpoint);
        }
    }

    fn move_pointer(&mut self, depth: usize, point: Point, mouse: bool) {
        let pane = &mut self.panes[depth];
        let row = pane.renderer.row_at(point);
        if row == pane.row {
            return;
        }
        pane.row = row;
        self.refresh_highlights();
        if mouse {
            if let Some(row) = row {
                self.preview = Some((depth, row, Instant::now() + SUBMENU_HOVER_DELAY));
            } else if self.preview.is_some_and(|(level, _, _)| level == depth) {
                self.preview = None;
            }
        }
    }

    fn input(
        &mut self,
        conn: &sws::Connection,
        display: MenuDisplay,
        model: &MenuModel<'_>,
        event: InputEvent,
    ) {
        let Some(depth) = self
            .panes
            .iter()
            .position(|pane| pane.id == event.surface_id)
        else {
            return;
        };
        match (event.type_, event.code, event.value) {
            (event_type::EV_ABS, abs_code::ABS_X, value) => {
                self.panes[depth].pointer.x = display.logical(value);
                self.panes[depth].pending_motion = true;
            }
            (event_type::EV_ABS, abs_code::ABS_Y, value) => {
                self.panes[depth].pointer.y = display.logical(value);
                self.panes[depth].pending_motion = true;
            }
            (event_type::EV_SYN, _, _) if self.panes[depth].pending_motion => {
                self.panes[depth].pending_motion = false;
                let point = self.panes[depth].pointer;
                self.move_pointer(depth, point, true);
            }
            (event_type::EV_KEY, key_code::BTN_LEFT, 0) => {
                let pane = &self.panes[depth];
                if let Some(row) = pane.renderer.row_at(pane.pointer) {
                    self.select_row(conn, display, model, depth, row, true);
                }
            }
            (
                event_type::EV_KEY,
                key_code::KEY_ESC | key_code::KEY_LEFT | key_code::KEY_BACKSPACE,
                1,
            ) => {
                if self.panes.len() > 1 {
                    self.trim(conn, self.panes.len() - 1);
                    self.focus_leaf(conn);
                    self.publish_surfaces(model.endpoint);
                } else if event.code == key_code::KEY_ESC {
                    model.endpoint.open_menu.set(None);
                }
            }
            (event_type::EV_KEY, key_code::KEY_UP | key_code::KEY_DOWN, 1) => {
                let pane = &mut self.panes[depth];
                let count = pane.renderer.rows.len();
                let forward = event.code == key_code::KEY_DOWN;
                let start = pane
                    .row
                    .unwrap_or(if forward { count.saturating_sub(1) } else { 0 });
                pane.row = (1..=count)
                    .map(|step| (start + if forward { step } else { count - step }) % count)
                    .find(|&row| pane.renderer.selectable(row));
                self.preview = None;
                self.refresh_highlights();
            }
            (event_type::EV_KEY, key_code::KEY_ENTER | key_code::KEY_RIGHT, 1) => {
                if let Some(row) = self.panes[depth].row
                    && (event.code == key_code::KEY_ENTER
                        || self.panes[depth].renderer.rows[row] == Some(MenuAction::Submenu))
                {
                    self.select_row(conn, display, model, depth, row, true);
                }
            }
            _ => {}
        }
    }

    fn touch_frame(
        &mut self,
        conn: &sws::Connection,
        display: MenuDisplay,
        model: &MenuModel<'_>,
        surface_id: u32,
        frame: touch::Frame,
    ) {
        for change in frame.changes {
            let Some(depth) = self.panes.iter().position(|pane| pane.id == surface_id) else {
                break;
            };
            let contact = (surface_id, change.id);
            let point = Point::new(display.logical(change.x), display.logical(change.y));
            match change.phase {
                touch::Phase::Down if self.touch.is_none() => {
                    self.touch = Some(contact);
                    self.preview = None;
                }
                touch::Phase::Move if self.touch == Some(contact) => {
                    self.move_pointer(depth, point, false);
                }
                touch::Phase::Up if self.touch == Some(contact) => {
                    self.touch = None;
                    if let Some(row) = self.panes[depth].renderer.row_at(point) {
                        self.select_row(conn, display, model, depth, row, true);
                    }
                }
                touch::Phase::Cancel if self.touch == Some(contact) => self.touch = None,
                _ => {}
            }
        }
    }
}

pub(super) fn run(mut endpoint: multi_output::Endpoint) {
    let Ok(SwsScreenConnection {
        connection: conn,
        logical_width,
        logical_height,
        scale_milli,
        ..
    }) = connect_sws_with_screen_size_retry()
    else {
        println!("[StatusBar] Failed to connect to SWS for menu popups");
        return;
    };

    let mut display = MenuDisplay {
        size: Size::new(logical_width as f32, logical_height as f32),
        origin: (0, 0),
        scale: scale_milli,
        touch: conn
            .get_capabilities()
            .is_ok_and(|caps| caps.supports_touch_input()),
    };
    let mut cascade = MenuCascade::default();
    let mut previous = None;
    let mut endpoint_generation = 0;
    loop {
        if let Some(bus) = multi_output::session() {
            let endpoints = bus.endpoints.get();
            let generation = bus.topology_generation();
            let next = endpoints
                .iter()
                .find(|candidate| {
                    candidate.id != endpoint.id && candidate.open_menu.get().is_some()
                })
                .or_else(|| {
                    endpoints
                        .iter()
                        .find(|candidate| candidate.id == endpoint.id)
                })
                .or_else(|| endpoints.first());
            if let Some(next) = next
                && (next.id != endpoint.id || generation != endpoint_generation)
            {
                cascade.trim(&conn, 0);
                cascade.publish_surfaces(&endpoint);
                if next.id != endpoint.id {
                    endpoint.open_menu.set(None);
                }
                endpoint = next.clone();
                endpoint_generation = generation;
                previous = None;
                if let Ok(context) = conn.output_context(endpoint.id) {
                    display.origin = (context.rect.x, context.rect.y);
                    display.size = Size::new(context.rect.width as f32, context.rect.height as f32);
                    display.scale = context.scale_milli;
                }
            }
        }

        let open = endpoint.open_menu.get();
        let tree = endpoint.menu_tree.get();
        let active_window = endpoint.active_window.get();
        let overview = if open == Some(OVERVIEW_MENU_INDEX) {
            (endpoint.overview_page.get(), endpoint.windows.get())
        } else {
            (0, Vec::new())
        };
        let model = open.map(|index| MenuModel {
            endpoint: &endpoint,
            tree: &tree,
            index,
            active_window,
        });
        let changed =
            previous
                .as_ref()
                .is_none_or(|(old_open, old_tree, old_window, old_overview)| {
                    *old_open != open
                        || *old_tree != tree
                        || *old_window != active_window
                        || *old_overview != overview
                });
        if changed {
            if let Some(model) = &model {
                let items = model.root_items(&overview.1, overview.0, display.size.height as u32);
                if items.is_empty() || cascade.set_root(&conn, display, model, items).is_err() {
                    cascade.trim(&conn, 0);
                    cascade.publish_surfaces(&endpoint);
                    endpoint.open_menu.set(None);
                    previous = None;
                    continue;
                }
            } else if cascade.trim(&conn, 0) {
                cascade.publish_surfaces(&endpoint);
            }
        }

        if conn.dispatch().is_err() {
            cascade.trim(&conn, 0);
            cascade.publish_surfaces(&endpoint);
            endpoint.open_menu.set(None);
            return;
        }
        while let Some(event) = conn.poll_event() {
            match event {
                Event::FocusChanged {
                    window_id,
                    app_id,
                    app_name,
                    ..
                } => {
                    if cascade.panes.iter().any(|pane| pane.id == window_id) {
                        cascade.focus_observed = true;
                    } else if cascade.focus_observed
                        && app_id != "org.scarlet-os.desktop.taskbar"
                        && app_name != "TaskBar"
                        && app_name != "Menu"
                    {
                        endpoint.open_menu.set(None);
                    }
                }
                Event::Input(event) => {
                    if let Some(model) = &model {
                        cascade.input(&conn, display, model, event);
                    }
                }
                Event::TouchFrame { surface_id, frame } => {
                    if let Some(model) = &model {
                        cascade.touch_frame(&conn, display, model, surface_id, frame);
                    }
                }
                Event::ScreenSizeChanged { width, height } if multi_output::session().is_none() => {
                    display.size = Size::new(
                        unscale_u32(width, display.scale) as f32,
                        unscale_u32(height, display.scale) as f32,
                    );
                    endpoint.open_menu.set(None);
                }
                Event::OutputScaleChanged { scale_milli } if multi_output::session().is_none() => {
                    display.scale = scale_milli.max(1);
                    if let Ok((width, height)) = conn.get_screen_size() {
                        display.size = Size::new(
                            unscale_u32(width, display.scale) as f32,
                            unscale_u32(height, display.scale) as f32,
                        );
                    }
                    endpoint.open_menu.set(None);
                }
                _ => {}
            }
            if endpoint.open_menu.get() != open {
                break;
            }
        }

        if endpoint.open_menu.get() == open
            && (open != Some(OVERVIEW_MENU_INDEX) || endpoint.overview_page.get() == overview.0)
        {
            if let Some(model) = &model
                && let Some((depth, row, deadline)) = cascade.preview
                && Instant::now() >= deadline
            {
                cascade.select_row(&conn, display, model, depth, row, false);
            }
            for pane in &mut cascade.panes {
                if pane.dirty {
                    pane.repaint(&conn);
                }
            }
            std::thread::sleep(Duration::from_millis(if open.is_some() { 8 } else { 16 }));
        }
        if changed {
            previous = Some((open, tree, active_window, overview));
        }
    }
}
