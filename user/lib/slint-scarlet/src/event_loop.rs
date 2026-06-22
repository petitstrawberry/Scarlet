//! Event loop for processing SWS events and dispatching to Slint windows

use std::collections::BTreeMap;
use std::rc::Rc;
use std::vec::Vec;
use slint::platform::{PlatformError, WindowAdapter};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::platform::software_renderer::PremultipliedRgbaColor;
use slint::PhysicalSize;
use sws_client::Connection;
use sws_client::event::{abs_code, event_type, key_code, rel_code, Event as SwsEvent, InputEvent as SwsInputEvent};
use crate::window_adapter::ScarletWindowAdapter;
use crate::{use_csd_titlebar, TITLEBAR_HEIGHT_PX};
use scarlet_ui::{Canvas, Color};

// Constants for titlebar rendering (matching scarlet-ui WindowRenderObject)
const TITLEBAR_CONTROL_COUNT: u32 = 3;
const CLOSE_BUTTON_SIZE: u32 = 18;
const CLOSE_BUTTON_MARGIN: u32 = 8;

#[derive(Debug, Clone, Copy, Default)]
struct PointerState {
    x: f32,
    y: f32,
    pending_move: bool,
    pressed_in_content: bool,
    last_content_x: f32,
    last_content_y: f32,
    pending_wheel_dx: f32,
    pending_wheel_dy: f32,
}

/// Window decoration state for CSD
#[derive(Debug, Clone)]
struct DecorationState {
    // Button states: 0=none, 1=hover, 2=pressed
    close_button_state: u8,
    maximize_button_state: u8,
    minimize_button_state: u8,
    // Action flags (set on press, executed on release)
    close_requested: bool,
    minimize_requested: bool,
    maximize_toggle_requested: bool,
    maximized: bool,
    needs_redraw: bool,
}

impl Default for DecorationState {
    fn default() -> Self {
        Self {
            close_button_state: 0,
            maximize_button_state: 0,
            minimize_button_state: 0,
            close_requested: false,
            minimize_requested: false,
            maximize_toggle_requested: false,
            maximized: false,
            needs_redraw: true,
        }
    }
}

fn content_pos_from_surface(x: f32, y: f32) -> Option<slint::LogicalPosition> {
    if !use_csd_titlebar() {
        return Some(slint::LogicalPosition::new(x, y));
    }

    let titlebar_h = TITLEBAR_HEIGHT_PX as f32;
    if y < titlebar_h {
        None
    } else {
        Some(slint::LogicalPosition::new(x, y - titlebar_h))
    }
}

/// Event loop that processes SWS events and dispatches to windows
pub struct EventLoop {
    windows: Vec<Rc<ScarletWindowAdapter>>,
    pointer: BTreeMap<u32, PointerState>,
    decorations: BTreeMap<u32, DecorationState>,
    pixel_buffers: BTreeMap<u32, Vec<PremultipliedRgbaColor>>,
}

impl EventLoop {
    /// Create a new event loop
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            pointer: BTreeMap::new(),
            decorations: BTreeMap::new(),
            pixel_buffers: BTreeMap::new(),
        }
    }

    /// Add a window to the event loop
    pub fn add_window(&mut self, window: Rc<ScarletWindowAdapter>) {
        self.windows.push(window);
    }

    /// Run the event loop
    pub fn run(&mut self, connection: &mut Connection) -> Result<(), PlatformError> {
        loop {
            // Poll for events from the window server
            match connection.dispatch() {
                Ok(_) => {
                    // Drain and dispatch all pending events.
                    while let Some(ev) = connection.poll_event() {
                        self.handle_sws_event(connection, ev)?;
                    }
                }
                Err(e) => {
                    return Err(PlatformError::Other(
                        std::format!("Event dispatch error: {:?}", e).into()
                    ));
                }
            }

            // Render windows (software renderer) after processing input.
            // Clone the Rc to avoid borrowing self immutably while rendering mutably.
            let windows = self.windows.clone();
            for window in windows {
                self.process_window_events(connection, &window);
            }

            // Check if we should exit (e.g., all windows closed)
            if self.windows.is_empty() {
                break;
            }

            // Avoid a tight busy-loop.
            std::thread::sleep(core::time::Duration::from_millis(16));
        }

        Ok(())
    }

    fn handle_sws_event(&mut self, connection: &mut Connection, ev: SwsEvent) -> Result<(), PlatformError> {
        match ev {
            SwsEvent::Input(input) => {
                self.handle_input_event(connection, input);
            }
            SwsEvent::SurfaceDestroyed { surface_id } => {
                self.windows.retain(|w| w.surface_id() != surface_id);
                self.pointer.remove(&surface_id);
                self.decorations.remove(&surface_id);
            }
            SwsEvent::SurfaceConfigure {
                surface_id,
                width,
                height,
            } => {
                // Let the client resize its surface buffer.
                let _ = connection.resize_window(surface_id, width, height);

                // Update Slint's logical window size.
                if let Some(win) = self.windows.iter().find(|w| w.surface_id() == surface_id) {
                    win.set_surface_size(PhysicalSize::new(width, height));
                    let content_h = if use_csd_titlebar() {
                        height.saturating_sub(TITLEBAR_HEIGHT_PX)
                    } else {
                        height
                    };
                    win.set_content_size(PhysicalSize::new(width, content_h));
                    win.window().dispatch_event(WindowEvent::Resized {
                        size: slint::LogicalSize::new(width as f32, content_h as f32),
                    });

                    // Clear pixel buffer to force reallocation with new size
                    self.pixel_buffers.remove(&surface_id);
                }
            }
            SwsEvent::Error { code: _ } => {
                // For now, ignore.
            }
            SwsEvent::FocusChanged { .. } => {
                // Focus changed event - currently unused in slint-scarlet
                // Could be used to update window focus state in the future
            }
            SwsEvent::ActiveAppChanged { .. } => {
                // Active app changed - not used in slint apps
            }
            SwsEvent::ScreenSizeChanged { .. } => {
                // SurfaceConfigure carries the per-window resize request.
            }
            SwsEvent::OutputScaleChanged { .. } => {
                // slint-scarlet keeps its existing physical-size behavior for now.
            }
            SwsEvent::MenuItemActivated { .. } => {
                // Menu item activated - not used in slint apps
            }
            SwsEvent::TextInputPreedit { .. }
            | SwsEvent::TextInputCommit { .. }
            | SwsEvent::TextInputDeleteSurroundingText { .. }
            | SwsEvent::TextInputDone { .. }
            | SwsEvent::TextInputStatus { .. }
            | SwsEvent::ImeActivate(_)
            | SwsEvent::ImeDeactivate { .. }
            | SwsEvent::ImeContextState(_)
            | SwsEvent::ImeKeyEvent { .. }
            | SwsEvent::ImeReset { .. }
            | SwsEvent::ImeTrigger { .. } => {
                // Text-input/IME events are handled by clients that opt into
                // the SWS text-input protocol.
            }
        }

        Ok(())
    }

    fn handle_input_event(&mut self, connection: &mut Connection, ev: SwsInputEvent) {
        let Some(win) = self.windows.iter().find(|w| w.surface_id() == ev.surface_id) else {
            return;
        };

        let surface_id = ev.surface_id;

        match (ev.type_, ev.code) {
            (event_type::EV_ABS, abs_code::ABS_X) => {
                let state = self.pointer.entry(ev.surface_id).or_default();
                state.x = ev.value as f32;
                state.pending_move = true;
            }
            (event_type::EV_ABS, abs_code::ABS_Y) => {
                let state = self.pointer.entry(ev.surface_id).or_default();
                state.y = ev.value as f32;
                state.pending_move = true;
            }
            (event_type::EV_SYN, _) => {
                let state = self.pointer.entry(ev.surface_id).or_default();
                if state.pending_move {
                    // Update decoration hover state
                    if use_csd_titlebar() {
                        if let Some(deco) = self.decorations.get_mut(&surface_id) {
                            let mouse_x = state.x as i32;
                            let mouse_y = state.y as i32;
                            let mouse_pressed = false;

                            let width = win.surface_size().width as u32;
                            let old_close = deco.close_button_state;
                            let old_max = deco.maximize_button_state;
                            let old_min = deco.minimize_button_state;

                            update_decoration_button_states(deco, mouse_x, mouse_y, mouse_pressed, width);

                            if old_close != deco.close_button_state || old_max != deco.maximize_button_state
                                || old_min != deco.minimize_button_state {
                                deco.needs_redraw = true;
                            }
                        }
                    }

                    if let Some(pos) = content_pos_from_surface(state.x, state.y) {
                        state.last_content_x = pos.x;
                        state.last_content_y = pos.y;
                        win.window().dispatch_event(WindowEvent::PointerMoved { position: pos });
                    }
                    state.pending_move = false;
                }

                // Dispatch accumulated wheel scroll
                if state.pending_wheel_dx != 0.0 || state.pending_wheel_dy != 0.0 {
                    if let Some(pos) = content_pos_from_surface(state.x, state.y) {
                        win.window().dispatch_event(WindowEvent::PointerScrolled {
                            position: pos,
                            delta_x: state.pending_wheel_dx,
                            delta_y: state.pending_wheel_dy,
                        });
                    }
                    state.pending_wheel_dx = 0.0;
                    state.pending_wheel_dy = 0.0;
                }
            }
            (event_type::EV_KEY, key_code::BTN_LEFT) => {
                let state = self.pointer.entry(ev.surface_id).or_default();
                if ev.value != 0 {
                    // Press
                    // Check if click is in titlebar
                    if use_csd_titlebar() && state.y < TITLEBAR_HEIGHT_PX as f32 {
                        if let Some(deco) = self.decorations.get_mut(&surface_id) {
                            let mouse_x = state.x as i32;
                            let mouse_y = state.y as i32;
                            let width = win.surface_size().width as u32;

                            // Update button states with pressed = true
                            update_decoration_button_states(deco, mouse_x, mouse_y, true, width);
                            deco.needs_redraw = true;

                            // Check if clicking on close button
                            if deco.close_button_state == 2 {
                                deco.close_requested = true;
                            }
                            // Maximize button
                            else if deco.maximize_button_state == 2 {
                                deco.maximize_toggle_requested = true;
                            }
                            // Minimize button
                            else if deco.minimize_button_state == 2 {
                                deco.minimize_requested = true;
                            }
                            // Title bar drag (not on buttons) - request move immediately
                            else {
                                // Request SWS to handle interactive move
                                let _ = connection.request_move_window(surface_id);
                            }

                            state.pressed_in_content = false;
                            return;
                        }
                    }

                    // Otherwise, forward into Slint content.
                    if let Some(pos) = content_pos_from_surface(state.x, state.y) {
                        state.last_content_x = pos.x;
                        state.last_content_y = pos.y;
                        state.pressed_in_content = true;
                        win.window().dispatch_event(WindowEvent::PointerPressed {
                            position: pos,
                            button: PointerEventButton::Left,
                        });
                    }
                } else {
                    // Release - handle decoration button actions
                    if use_csd_titlebar() {
                        if let Some(deco) = self.decorations.get_mut(&surface_id) {
                            // Reset button states
                            let mouse_x = state.x as i32;
                            let mouse_y = state.y as i32;
                            let width = win.surface_size().width as u32;
                            update_decoration_button_states(deco, mouse_x, mouse_y, false, width);
                            deco.needs_redraw = true;

                            // Execute button actions (not move - SWS handles that)
                            if deco.close_requested {
                                let _ = connection.destroy_surface(surface_id);
                                deco.close_requested = false;
                                return;
                            }
                            if deco.minimize_requested {
                                let _ = connection.minimize_window(surface_id);
                                deco.minimize_requested = false;
                                return;
                            }
                            if deco.maximize_toggle_requested {
                                if deco.maximized {
                                    let _ = connection.restore_window(surface_id);
                                    deco.maximized = false;
                                } else {
                                    let _ = connection.maximize_window(surface_id);
                                    deco.maximized = true;
                                }
                                deco.maximize_toggle_requested = false;
                                return;
                            }
                        }
                    }

                    if state.pressed_in_content {
                        let pos = content_pos_from_surface(state.x, state.y)
                            .unwrap_or_else(|| slint::LogicalPosition::new(state.last_content_x, state.last_content_y));
                        win.window().dispatch_event(WindowEvent::PointerReleased {
                            position: pos,
                            button: PointerEventButton::Left,
                        });
                    }
                    state.pressed_in_content = false;
                }
            }
            (event_type::EV_REL, rel_code::REL_WHEEL) => {
                let state = self.pointer.entry(ev.surface_id).or_default();
                // REL_WHEEL value is typically ±1 per notch. Scale to pixels.
                state.pending_wheel_dy += ev.value as f32 * 20.0;
            }
            (event_type::EV_REL, rel_code::REL_HWHEEL) => {
                let state = self.pointer.entry(ev.surface_id).or_default();
                state.pending_wheel_dx += ev.value as f32 * 20.0;
            }
            _ => {}
        }
    }

    fn process_window_events(&mut self, connection: &mut Connection, window: &Rc<ScarletWindowAdapter>) {
        self.render_window(connection, window);
    }

    fn render_window(&mut self, connection: &mut Connection, window: &Rc<ScarletWindowAdapter>) {
        let renderer = window.renderer_ref();

        let surface_id = window.surface_id();
        let content_size = window.size();

        let slint_dirty = window.take_redraw_requested();
        let deco_dirty = use_csd_titlebar()
            && self
                .decorations
                .get(&surface_id)
                .map(|d| d.needs_redraw)
                .unwrap_or(false);

        if !slint_dirty && !deco_dirty {
            return;
        }

        // Ensure decoration state exists
        if use_csd_titlebar() && !self.decorations.contains_key(&surface_id) {
            self.decorations.entry(surface_id).or_default();
        }

        // Render using Slint's software renderer
        if let Some(surf) = connection.surface_mut(surface_id) {
            let renderer_borrowed = renderer.borrow_mut();
            surf.with_buffer(|buffer, width, height| {
                let y_offset = if use_csd_titlebar() {
                    TITLEBAR_HEIGHT_PX.min(height)
                } else {
                    0
                };
                let stride_bytes = width as usize * 4;

                if slint_dirty {
                    // Slint renders only the content area (below titlebar if enabled).
                    let content_h = content_size.height.min(height);
                    let pixel_count = (width as usize) * (content_h as usize);
                    let pixels = self.pixel_buffers.entry(surface_id).or_default();
                    pixels.resize(pixel_count, PremultipliedRgbaColor::default());

                    renderer_borrowed.render(&mut pixels[..], width as usize);

                    // Convert RGBA (premultiplied) into SWS BGRA and blit into the content region.
                    for y in 0..(content_h as usize) {
                        let dst_row = (y + y_offset as usize) * stride_bytes;
                        let src_row = y * width as usize;
                        if dst_row + stride_bytes > buffer.len() {
                            break;
                        }
                        for x in 0..(width as usize) {
                            let px = pixels[src_row + x];
                            let rgba: [u8; 4] = unsafe { core::mem::transmute(px) };
                            let dst = dst_row + x * 4;
                            buffer[dst] = rgba[2];
                            buffer[dst + 1] = rgba[1];
                            buffer[dst + 2] = rgba[0];
                            buffer[dst + 3] = rgba[3];
                        }
                    }
                }

                if deco_dirty {
                    if let Some(deco) = self.decorations.get(&surface_id) {
                        draw_titlebar(deco, "Slint App", buffer, width, height);
                    }
                    if let Some(deco) = self.decorations.get_mut(&surface_id) {
                        deco.needs_redraw = false;
                    }
                }
            });
        }

        // Commit only when we actually changed pixels.
        let _ = connection.commit(surface_id);
    }
}

/// Update decoration button states based on mouse position
fn update_decoration_button_states(deco: &mut DecorationState, mouse_x: i32, mouse_y: i32, mouse_pressed: bool, width: u32) {
    let close_rect = control_button_rect(width, 0);
    let maximize_rect = control_button_rect(width, 1);
    let minimize_rect = control_button_rect(width, 2);

    let point = scarlet_ui::geometry::Point {
        x: mouse_x as f32,
        y: mouse_y as f32,
    };

    // Update close button state
    deco.close_button_state = if close_rect.contains(point) {
        if mouse_pressed { 2 } else { 1 }
    } else {
        0
    };

    // Update maximize button state
    deco.maximize_button_state = if maximize_rect.contains(point) {
        if mouse_pressed { 2 } else { 1 }
    } else {
        0
    };

    // Update minimize button state
    deco.minimize_button_state = if minimize_rect.contains(point) {
        if mouse_pressed { 2 } else { 1 }
    } else {
        0
    };
}

/// Calculate control button rect (matching scarlet-ui WindowRenderObject)
fn control_button_rect(width: u32, index_from_right: u32) -> scarlet_ui::geometry::Rect {
    if width < TITLEBAR_CONTROL_COUNT {
        return scarlet_ui::geometry::Rect::zero();
    }

    let base_seg_w = CLOSE_BUTTON_SIZE + CLOSE_BUTTON_MARGIN * 2;
    let seg_w = if width >= base_seg_w * TITLEBAR_CONTROL_COUNT {
        base_seg_w
    } else {
        (width / TITLEBAR_CONTROL_COUNT).max(1)
    };
    let total_w = seg_w.saturating_mul(TITLEBAR_CONTROL_COUNT).min(width);
    let right_x0 = (width - total_w) as i32;
    let x = right_x0 + (total_w as i32) - (seg_w as i32) * (index_from_right as i32 + 1);
    scarlet_ui::geometry::Rect::from_xywh(x as f32, 0.0, seg_w as f32, TITLEBAR_HEIGHT_PX as f32)
}

/// Draw titlebar to buffer
fn draw_titlebar(deco: &DecorationState, title: &str, buffer: &mut [u8], width: u32, height: u32) {
    let mut canvas = Canvas::new(buffer, width, height);

    let titlebar_height = TITLEBAR_HEIGHT_PX.min(height);

    // Title bar base color (matching scarlet-ui: rgb(235, 235, 238))
    let base_color = Color::rgb(235u8, 235u8, 238u8);

    // Button colors based on hover/pressed state
    let close_color = get_button_color(deco.close_button_state);
    let maximize_color = get_button_color(deco.maximize_button_state);
    let minimize_color = get_button_color(deco.minimize_button_state);

    // Draw titlebar background
    for y in 0..titlebar_height {
        canvas.fill_rect(0, y as i32, width, 1, base_color);
    }

    // Draw button backgrounds
    let close_rect = control_button_rect(width, 0);
    let maximize_rect = control_button_rect(width, 1);
    let minimize_rect = control_button_rect(width, 2);

    for y in 0..titlebar_height {
        canvas.fill_rect(close_rect.origin.x as i32, y as i32, close_rect.size.width as u32, 1, close_color);
        canvas.fill_rect(maximize_rect.origin.x as i32, y as i32, maximize_rect.size.width as u32, 1, maximize_color);
        canvas.fill_rect(minimize_rect.origin.x as i32, y as i32, minimize_rect.size.width as u32, 1, minimize_color);
    }

    // Title text (matching scarlet-ui: rgb(20, 20, 24))
    canvas.draw_text_sized(10, 7, title, Color::rgb(20u8, 20u8, 24u8), 18.0);

    // Draw button icons (matching scarlet-ui WindowRenderObject)
    let icon_color = Color::rgb(30u8, 30u8, 34u8);

    // Close button: X mark (double-stroke lines)
    let cx = close_rect.origin.x + close_rect.size.width / 2.0;
    let cy = close_rect.origin.y + close_rect.size.height / 2.0;
    let size: i32 = 10;
    let half = size / 2;
    let x0 = cx as i32 - half;
    let x1 = cx as i32 + half - 1;
    let y0 = cy as i32 - half;
    let y1 = cy as i32 + half - 1;
    canvas.draw_line(x0, y0, x1, y1, icon_color);
    canvas.draw_line(x1, y0, x0, y1, icon_color);

    // Maximize button: square outline
    let mx = maximize_rect.origin.x + maximize_rect.size.width / 2.0;
    let my = maximize_rect.origin.y + maximize_rect.size.height / 2.0;
    let msize: i32 = 10;
    let mhalf = msize / 2;
    let mx0 = mx as i32 - mhalf;
    let my0 = my as i32 - mhalf;
    canvas.draw_rect(mx0, my0, msize as u32, msize as u32, icon_color);

    // Minimize button: horizontal line
    let nx = minimize_rect.origin.x + minimize_rect.size.width / 2.0;
    let ny = minimize_rect.origin.y + minimize_rect.size.height / 2.0 + 3.0;
    let nsize: i32 = 12;
    let nhalf = nsize / 2;
    canvas.draw_line(nx as i32 - nhalf, ny as i32, nx as i32 + nhalf, ny as i32, icon_color);

    // Draw border at bottom of titlebar
    let border_color = Color::rgb(180u8, 180u8, 185u8);
    canvas.draw_line(0, titlebar_height as i32 - 1, width as i32 - 1, titlebar_height as i32 - 1, border_color);

    // Draw window border (matching scarlet-ui)
    // Outer border: rgb(100, 100, 105)
    let window_border_color = Color::rgb(100u8, 100u8, 105u8);
    canvas.draw_rect(0, 0, width, height, window_border_color);

    // Inner highlight for depth: rgb(90, 90, 95)
    if width > 2 && height > 2 {
        canvas.draw_rect(
            1,
            1,
            width.saturating_sub(2),
            height.saturating_sub(2),
            Color::rgb(90u8, 90u8, 95u8),
        );
    }
}

/// Get button color based on state (matching scarlet-ui)
fn get_button_color(state: u8) -> Color {
    match state {
        0 => Color::rgb(235u8, 235u8, 238u8), // normal
        1 => Color::rgb(210u8, 210u8, 213u8), // hover
        2 => Color::rgb(190u8, 190u8, 193u8), // pressed
        _ => Color::rgb(235u8, 235u8, 238u8),
    }
}
