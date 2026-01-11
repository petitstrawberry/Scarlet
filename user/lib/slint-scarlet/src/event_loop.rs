//! Event loop for processing SWS events and dispatching to Slint windows

use std::collections::BTreeMap;
use std::rc::Rc;
use std::vec::Vec;
use slint::platform::PlatformError;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::platform::WindowAdapter;
use slint::platform::software_renderer::PremultipliedRgbaColor;
use slint::PhysicalSize;
use sws_client::Connection;
use sws_client::event::{abs_code, event_type, key_code, Event as SwsEvent, InputEvent as SwsInputEvent};
use crate::window_adapter::ScarletWindowAdapter;
use crate::use_csd_titlebar;
use scarlet_ui::{Canvas, Color, Event as UiEvent, MouseButton, Rect as UiRect, Window as UiWindow, View};

#[derive(Debug, Clone, Copy, Default)]
struct PointerState {
    x: f32,
    y: f32,
    pending_move: bool,
    pressed_in_content: bool,
    last_content_x: f32,
    last_content_y: f32,
}

fn content_pos_from_surface(x: f32, y: f32) -> Option<slint::LogicalPosition> {
    if !use_csd_titlebar() {
        return Some(slint::LogicalPosition::new(x, y));
    }

    let titlebar_h = UiWindow::titlebar_height() as f32;
    if y < titlebar_h {
        None
    } else {
        Some(slint::LogicalPosition::new(x, y - titlebar_h))
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct DecorationState {
    maximized: bool,
}

/// Event loop that processes SWS events and dispatches them to windows
pub struct EventLoop {
    windows: Vec<Rc<ScarletWindowAdapter>>,
    pointer: BTreeMap<u32, PointerState>,
    decorations: BTreeMap<u32, UiWindow>,
    decoration_state: BTreeMap<u32, DecorationState>,
    pixel_buffers: BTreeMap<u32, Vec<PremultipliedRgbaColor>>,
}

impl EventLoop {
    /// Create a new event loop
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            pointer: BTreeMap::new(),
            decorations: BTreeMap::new(),
            decoration_state: BTreeMap::new(),
            pixel_buffers: BTreeMap::new(),
        }
    }
    
    /// Add a window to the event loop
    pub fn add_window(&mut self, window: Rc<ScarletWindowAdapter>) {
        // Keep a matching ScarletUI decoration window (for titlebar/border).
        let surface_id = window.surface_id();
        let surface_size = window.surface_size();
        self.decorations
            .entry(surface_id)
            .or_insert_with(|| UiWindow::new("Slint", surface_size.width, surface_size.height)
                .background(Color::rgb(40, 40, 40)));
        self.decoration_state.entry(surface_id).or_default();
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
                self.decoration_state.remove(&surface_id);
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
                        height.saturating_sub(UiWindow::titlebar_height())
                    } else {
                        height
                    };
                    win.set_content_size(PhysicalSize::new(width, content_h));
                    win.window().dispatch_event(WindowEvent::Resized {
                        size: slint::LogicalSize::new(width as f32, content_h as f32),
                    });
                }

                if let Some(deco) = self.decorations.get_mut(&surface_id) {
                    deco.set_size(width, height);
                }
            }
            SwsEvent::Error { code: _ } => {
                // For now, ignore.
            }
        }

        Ok(())
    }

    fn handle_input_event(&mut self, connection: &mut Connection, ev: SwsInputEvent) {
        let Some(win) = self.windows.iter().find(|w| w.surface_id() == ev.surface_id) else {
            return;
        };

        let state = self.pointer.entry(ev.surface_id).or_default();
        let surface_id = ev.surface_id;

        match (ev.type_, ev.code) {
            (event_type::EV_ABS, abs_code::ABS_X) => {
                state.x = ev.value as f32;
                state.pending_move = true;
            }
            (event_type::EV_ABS, abs_code::ABS_Y) => {
                state.y = ev.value as f32;
                state.pending_move = true;
            }
            (event_type::EV_SYN, _) => {
                if state.pending_move {
                    // Update ScarletUI decorations hover state.
                    if use_csd_titlebar() {
                        if let Some(deco) = self.decorations.get_mut(&surface_id) {
                            let mut ui_ev = UiEvent::mouse_move(state.x as i32, state.y as i32);
                            let frame = UiRect::new(0, 0, deco.width(), deco.height());
                            let _ = deco.on_event_capture(&mut ui_ev, frame);
                        }
                    }

                    if let Some(pos) = content_pos_from_surface(state.x, state.y) {
                        state.last_content_x = pos.x;
                        state.last_content_y = pos.y;
                        win.window().dispatch_event(WindowEvent::PointerMoved { position: pos });
                    }
                    state.pending_move = false;
                }
            }
            (event_type::EV_KEY, key_code::BTN_LEFT) => {
                if ev.value != 0 {
                    // First, let ScarletUI decorations handle the press.
                    if use_csd_titlebar() {
                        if let Some(deco) = self.decorations.get_mut(&surface_id) {
                            let mut ui_ev = UiEvent::mouse_down(state.x as i32, state.y as i32, MouseButton::Left);
                            let frame = UiRect::new(0, 0, deco.width(), deco.height());
                            let consumed = deco.on_event_capture(&mut ui_ev, frame);

                            if consumed {
                                // Apply requested actions.
                                if deco.is_close_requested() {
                                    let _ = connection.destroy_surface(surface_id);
                                }
                                if deco.take_move_requested() {
                                    let _ = connection.request_move_window(surface_id);
                                }
                                if deco.take_minimize_requested() {
                                    let _ = connection.minimize_window(surface_id);
                                }
                                if deco.take_maximize_toggle_requested() {
                                    let deco_state = self.decoration_state.entry(surface_id).or_default();
                                    if deco_state.maximized {
                                        let _ = connection.restore_window(surface_id);
                                        deco_state.maximized = false;
                                    } else {
                                        let _ = connection.maximize_window(surface_id);
                                        deco_state.maximized = true;
                                    }
                                }

                                state.pressed_in_content = false;
                                return;
                            }
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
                    // First, let ScarletUI decorations handle the release.
                    if use_csd_titlebar() {
                        if let Some(deco) = self.decorations.get_mut(&surface_id) {
                            let mut ui_ev = UiEvent::mouse_up(state.x as i32, state.y as i32, MouseButton::Left);
                            let frame = UiRect::new(0, 0, deco.width(), deco.height());
                            let consumed = deco.on_event_capture(&mut ui_ev, frame);

                            if consumed {
                                if deco.is_close_requested() {
                                    let _ = connection.destroy_surface(surface_id);
                                }
                                if deco.take_minimize_requested() {
                                    let _ = connection.minimize_window(surface_id);
                                }
                                if deco.take_maximize_toggle_requested() {
                                    let deco_state = self.decoration_state.entry(surface_id).or_default();
                                    if deco_state.maximized {
                                        let _ = connection.restore_window(surface_id);
                                        deco_state.maximized = false;
                                    } else {
                                        let _ = connection.maximize_window(surface_id);
                                        deco_state.maximized = true;
                                    }
                                }
                                state.pressed_in_content = false;
                                return;
                            }
                        }
                    }

                    if state.pressed_in_content {
                        // If the release happens in the titlebar, still release at last known content position.
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
                .is_some_and(|deco| deco.needs_draw());

        if !slint_dirty && !deco_dirty {
            return;
        }

        // Render using Slint's software renderer
        if let Some(surf) = connection.surface_mut(surface_id) {
            let renderer_borrowed = renderer.borrow_mut();
            surf.with_buffer(|buffer, width, height| {
                // NOTE: Do NOT call ScarletUI `Window::draw()` here.
                // It fills the entire window background (O(w*h)), which is expensive.
                // Slint will overwrite the content area; we only redraw border/titlebar selectively.

                let y_offset = if use_csd_titlebar() {
                    UiWindow::titlebar_height().min(height)
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

                if use_csd_titlebar() {
                    let mut canvas = Canvas::new(buffer, width, height);
                    if let Some(deco) = self.decorations.get(&surface_id) {
                        // Titlebar changes only when hovered/pressed/resized.
                        if deco_dirty {
                            deco.draw_titlebar_only(&mut canvas);
                        }
                        // Border pixels are overwritten by content blit; redraw border after Slint.
                        if slint_dirty {
                            deco.draw_border_only(&mut canvas);
                        }
                    }
                }
            });
        }

        // Commit only when we actually changed pixels.
        let _ = connection.commit(surface_id);

        if deco_dirty {
            if let Some(deco) = self.decorations.get_mut(&surface_id) {
                deco.clear_needs_draw();
            }
        }
    }
}
