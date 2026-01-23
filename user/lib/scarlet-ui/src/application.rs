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
        // 1. Create root element from self
        let root_element = self.create_element();

        // 2. Set up rendering pipeline
        let mut pipeline = RenderingPipeline::new();
        pipeline.set_root(root_element);

        // 3. Initialize the application
        self.init();

        // 4. Perform initial layout to determine window size
        let window_size = pipeline.layout_initial();

        // 5. Default window properties
        // In a full implementation, these would be extracted from the Window View
        let app_id = "com.example.scarletui";
        let window_title = "ScarletUI Application";

        // 6. Create platform window (default: SWS backend)
        let mut platform_window = SWSPlatformWindow::new(app_id, window_title, window_size)
            .map_err(|_| crate::error::Error::WindowCreationFailed)?;

        // 7. Main event loop
        loop {
            // 7.1 Poll events
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

            // 7.2 Render frame
            if let Some(buffer) = pipeline.render() {
                platform_window.present(buffer);
            }

            // 7.3 Small sleep to prevent busy-waiting
            // In a real implementation, this would use proper frame timing
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}
