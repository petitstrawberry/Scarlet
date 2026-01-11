//! Event loop for processing SWS events and dispatching to Slint windows

use std::rc::Rc;
use std::vec::Vec;
use slint::platform::PlatformError;
use slint::platform::WindowAdapter;
use slint::platform::software_renderer::PremultipliedRgbaColor;
use sws_client::Connection;
use crate::window_adapter::ScarletWindowAdapter;

/// Event loop that processes SWS events and dispatches them to windows
pub struct EventLoop {
    windows: Vec<Rc<ScarletWindowAdapter>>,
}

impl EventLoop {
    /// Create a new event loop
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
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
                    // Process events for all windows
                    for window in &self.windows {
                        self.process_window_events(connection, window);
                    }
                }
                Err(e) => {
                    return Err(PlatformError::Other(
                        std::format!("Event dispatch error: {:?}", e).into()
                    ));
                }
            }
            
            // Check if we should exit (e.g., all windows closed)
            if self.windows.is_empty() {
                break;
            }
        }
        
        Ok(())
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
