//! Event loop for processing SWS events and dispatching to Slint windows

use std::rc::Rc;
use std::vec::Vec;
use slint::platform::PlatformError;
use sws_client::{Connection, Event as SwsEvent};
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
                        self.process_window_events(window);
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
    fn process_window_events(&self, window: &Rc<ScarletWindowAdapter>) {
        // Render the window if needed
        self.render_window(window);
    }
    
    /// Render a window
    fn render_window(&self, window: &Rc<ScarletWindowAdapter>) {
        let renderer = window.renderer_ref();
        let surface = window.surface();
        
        // Get the surface buffer and render into it
        let mut surf = surface.borrow_mut();
        
        // Render using Slint's software renderer
        surf.with_buffer(|buffer| {
            let size = window.size();
            
            // Create a pixel buffer for the renderer
            // The SWS buffer is in BGRA format
            let mut renderer_borrowed = renderer.borrow_mut();
            
            // Render to buffer
            renderer_borrowed.render(buffer, size.width as usize);
        });
        
        // Commit the changes to the window server
        let _ = surf.commit();
    }
}
