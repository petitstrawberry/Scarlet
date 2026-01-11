//! Event loop for processing SWS events and dispatching to Slint windows

use std::collections::BTreeMap;
use std::rc::Rc;
use std::vec::Vec;
use slint::platform::PlatformError;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::platform::WindowAdapter;
use slint::platform::software_renderer::PremultipliedRgbaColor;
use sws_client::Connection;
use sws_client::event::{abs_code, event_type, key_code, Event as SwsEvent, InputEvent as SwsInputEvent};
use crate::window_adapter::ScarletWindowAdapter;

#[derive(Debug, Clone, Copy, Default)]
struct PointerState {
    x: f32,
    y: f32,
    pending_move: bool,
}

/// Event loop that processes SWS events and dispatches them to windows
pub struct EventLoop {
    windows: Vec<Rc<ScarletWindowAdapter>>,
    pointer: BTreeMap<u32, PointerState>,
}

impl EventLoop {
    /// Create a new event loop
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            pointer: BTreeMap::new(),
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
            for window in &self.windows {
                self.process_window_events(connection, window);
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
                self.handle_input_event(input);
            }
            SwsEvent::SurfaceDestroyed { surface_id } => {
                self.windows.retain(|w| w.surface_id() != surface_id);
                self.pointer.remove(&surface_id);
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
                    win.window().dispatch_event(WindowEvent::Resized {
                        size: slint::LogicalSize::new(width as f32, height as f32),
                    });
                }
            }
            SwsEvent::Error { code: _ } => {
                // For now, ignore.
            }
        }

        Ok(())
    }

    fn handle_input_event(&mut self, ev: SwsInputEvent) {
        let Some(win) = self.windows.iter().find(|w| w.surface_id() == ev.surface_id) else {
            return;
        };

        let state = self.pointer.entry(ev.surface_id).or_default();

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
                    win.window().dispatch_event(WindowEvent::PointerMoved {
                        position: slint::LogicalPosition::new(state.x, state.y),
                    });
                    state.pending_move = false;
                }
            }
            (event_type::EV_KEY, key_code::BTN_LEFT) => {
                let position = slint::LogicalPosition::new(state.x, state.y);
                if ev.value != 0 {
                    win.window().dispatch_event(WindowEvent::PointerPressed {
                        position,
                        button: PointerEventButton::Left,
                    });
                } else {
                    win.window().dispatch_event(WindowEvent::PointerReleased {
                        position,
                        button: PointerEventButton::Left,
                    });
                }
            }
            _ => {}
        }
    }
    
    /// Process events for a specific window
    fn process_window_events(&self, connection: &mut Connection, window: &Rc<ScarletWindowAdapter>) {
        // Render the window if needed
        self.render_window(connection, window);
    }
    
    /// Render a window
    fn render_window(&self, connection: &mut Connection, window: &Rc<ScarletWindowAdapter>) {
        let renderer = window.renderer_ref();

        let surface_id = window.surface_id();
        let _size = window.size();

        // Render using Slint's software renderer
        if let Some(surf) = connection.surface_mut(surface_id) {
            let renderer_borrowed = renderer.borrow_mut();
            surf.with_buffer(|buffer, width, height| {
                let pixel_count = (width as usize) * (height as usize);
                let mut pixels = std::vec![PremultipliedRgbaColor::default(); pixel_count];

                renderer_borrowed.render(&mut pixels[..], width as usize);

                // Convert RGBA (premultiplied) pixels into SWS BGRA byte buffer.
                // Note: channel order is BGRA on the wire.
                for (i, px) in pixels.iter().enumerate() {
                    let rgba: [u8; 4] = unsafe { core::mem::transmute(*px) };
                    let dst = i * 4;
                    if dst + 3 < buffer.len() {
                        buffer[dst] = rgba[2];
                        buffer[dst + 1] = rgba[1];
                        buffer[dst + 2] = rgba[0];
                        buffer[dst + 3] = rgba[3];
                    }
                }
            });
        }

        // Commit the changes to the window server
        let _ = connection.commit(surface_id);
    }
}
