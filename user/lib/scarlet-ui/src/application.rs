//! Application Trait - Main entry point for ScarletUI applications
//!
//! The Application trait provides the main loop and lifecycle management
//! for ScarletUI applications.

use crate::view::View;
use crate::geometry::Size;
use crate::event::Event;
use crate::platform::{PlatformWindow, SWSPlatformWindow};
use crate::pipeline::RenderingPipeline;
use crate::error::Result;
use crate::state::StateId;

/// Application trait - main entry point for ScarletUI apps
///
/// Applications implement this trait to define their UI and behavior.
/// The trait provides a default main loop that handles events,
/// rendering, and window management.
pub trait Application: View {
    /// Returns the body of the application
    ///
    /// This is where the application's UI is defined using Views.
    fn body(&self) -> impl View;

    /// Register all State instances used by this Application
    ///
    /// This method is called during Application initialization to register
    /// all State instances with the StateRegistry. The default implementation
    /// returns an empty vector.
    ///
    /// # Note
    ///
    /// This method is provided for future enhancement when automatic State
    /// registration via macros is implemented. Currently, States are
    /// automatically registered when first created through the View system.
    fn register_states(&self) -> alloc::vec::Vec<StateId> {
        alloc::vec::Vec::new()
    }

    /// Initialize the application
    ///
    /// Called once before the main loop starts.
    /// Override this to set up any initial state.
    fn init(&mut self) {
        // Default implementation: do nothing
    }

    /// Run the application main loop
    ///
    /// This sets up the window, runs the event loop, and renders the UI.
    /// The default implementation uses SWS as the platform backend.
    fn run(&mut self) -> Result<()>
    where
        Self: Sized,
    {
        // 1. Set up rendering pipeline
        let mut pipeline = RenderingPipeline::new();

        // 2. Create root element from body()
        let root_element = self.body().create_element();
        pipeline.set_root(root_element);

        // 3. Initialize the application
        self.init();

        // 4. Perform initial layout to determine window size and extract window properties
        let (app_id, window_title, window_size) = pipeline.layout_initial();

        // Debug: Dump element tree
        pipeline.element_tree().dump();

        // 5. Create platform window (default: SWS backend)
        let mut platform_window = SWSPlatformWindow::new(&app_id, &window_title, window_size)
            .map_err(|_| crate::error::Error::WindowCreationFailed)?;

        // 6. Main event loop
        loop {
            // 6.1 Poll events
            while let Some(event) = platform_window.poll_event() {
                match event {
                    Event::Quit => {
                        // Graceful shutdown
                        let _ = platform_window.close();
                        return Ok(());
                    }
                    Event::Resize { width, height } => {
                        let new_size = Size::new(width as f32, height as f32);
                        pipeline.resize(new_size);
                        let _ = platform_window.resize(width, height);
                    }
                    _ => {
                        // Other events are handled by the pipeline
                        // For now, this is a no-op
                    }
                }
            }

            // 6.2 Render frame
            if let Some(buffer) = pipeline.render() {
                platform_window.present(buffer);
            }

            // 6.3 Small sleep to prevent busy-waiting
            // In a real implementation, this would use proper frame timing
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}
