//! Application - manages windows and the event loop

use crate::view::{Window, View, Size};
use crate::event::{Event, MouseButton};
use crate::graphics::{Canvas, Rect, Point};
use scarlet_std::vec::Vec;
use sws_client::{Connection, Event as SwsEvent, InputEvent};

/// Managed window with surface binding
struct ManagedWindow {
    window: Window,
    surface_id: u32,
}

impl ManagedWindow {
    fn new(window: Window, surface_id: u32) -> Self {
        Self { window, surface_id }
    }
}

/// Application context that manages windows and the event loop
///
/// Application owns the event loop and handles all event dispatch,
/// layout, and drawing automatically. Users only need to define
/// their view hierarchy and call `run()`.
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{Application, Window, VStack, Label, Button};
///
/// let mut app = Application::new().expect("Failed to connect");
///
/// let window = Window::new("Demo", 400, 300)
///     .content(VStack::new()
///         .child(Label::new("Hello!"))
///         .child(Button::new("Click", || {}))
///     );
///
/// app.add_window(window);
/// app.run();
/// ```
pub struct Application {
    connection: Connection,
    windows: Vec<ManagedWindow>,
    last_mouse: Point,
}

impl Drop for Application {
    fn drop(&mut self) {
        // Best-effort cleanup: ensure protocol-level destroy is sent for all managed surfaces.
        // Drop cannot fail, so we ignore errors (e.g. already destroyed).
        for managed in &self.windows {
            let _ = self.connection.destroy_surface(managed.surface_id);
        }
        self.windows.clear();
    }
}

impl Application {
    /// Create a new application and connect to SWS
    pub fn new() -> Result<Self, &'static str> {
        Self::with_socket_path("/tmp/sws.sock")
    }

    /// Create a new application with a custom socket path
    pub fn with_socket_path(path: &str) -> Result<Self, &'static str> {
        let connection = Connection::connect(path).map_err(|_| "Failed to connect to SWS")?;
        Ok(Self {
            connection,
            windows: Vec::new(),
            last_mouse: Point::ZERO,
        })
    }

    /// Add a window to the application
    ///
    /// The window will be displayed and managed by the application.
    pub fn add_window(&mut self, window: Window) -> Result<(), &'static str> {
        let width = window.width();
        let height = window.height();
        
        // Create surface
        let surface_id = self
            .connection
            .create_surface(width, height)
            .map_err(|_| "Failed to create surface")?;
        
        // Create managed window
        let mut managed = ManagedWindow::new(window, surface_id);
        
        // Initial layout
        managed.window.layout(Size::new(width, height));
        
        // Initial draw directly into the surface SHM buffer
        {
            let frame = Rect::new(0, 0, width, height);
            if let Some(surface) = self.connection.surface_mut(surface_id) {
                let mut canvas = Canvas::new(surface.buffer_mut(), width, height);
                managed.window.draw(&mut canvas, frame);
                managed.window.clear_needs_draw();
            }
        }

        // Commit to display
        self.connection.commit(surface_id).map_err(|_| "Failed to commit")?;
        
        self.windows.push(managed);
        Ok(())
    }

    /// Run the application event loop
    ///
    /// This method never returns. It handles all events, layout,
    /// and drawing automatically.
    pub fn run(&mut self) -> ! {
        loop {
            // 1. Dispatch socket I/O
            let _ = self.connection.dispatch();
            
            // 2. Collect pending events first to avoid borrow issues
            let mut events: Vec<SwsEvent> = Vec::new();
            while let Some(sws_event) = self.connection.poll_event() {
                events.push(sws_event);
            }
            
            // 3. Process collected events
            for sws_event in events {
                self.handle_sws_event(sws_event);
            }

            // 4. Handle close requests (send DESTROY_WINDOW to SWS)
            // Window is dropped when removed from self.windows, but the protocol-level
            // destroy must be sent explicitly via sws-client.
            let mut close_surface_ids: Vec<u32> = Vec::new();
            for managed in &self.windows {
                if managed.window.is_close_requested() {
                    close_surface_ids.push(managed.surface_id);
                }
            }
            for surface_id in close_surface_ids {
                // If the surface is already gone (e.g. server-side destroyed), ignore.
                let _ = self.connection.destroy_surface(surface_id);
            }

            // 5. Drop closed windows
            self.windows.retain(|w| !w.window.is_close_requested());
            
            // 6. Layout and draw windows
            for i in 0..self.windows.len() {
                let managed = &mut self.windows[i];
                let size = Size::new(managed.window.width(), managed.window.height());
                managed.window.layout(size);
                
                if managed.window.needs_draw() {
                    // Draw directly into surface SHM buffer
                    let width = managed.window.width();
                    let height = managed.window.height();
                    let frame = Rect::new(0, 0, width, height);

                    if let Some(surface) = self.connection.surface_mut(managed.surface_id) {
                        let mut canvas = Canvas::new(surface.buffer_mut(), width, height);
                        managed.window.draw(&mut canvas, frame);
                        managed.window.clear_needs_draw();
                        let _ = self.connection.commit(managed.surface_id);
                    }
                }
            }
        }
    }

    /// Handle a SWS event
    fn handle_sws_event(&mut self, sws_event: SwsEvent) {
        match sws_event {
            SwsEvent::Input(input) => {
                if let Some(event) = self.convert_input(&input) {
                    // Dispatch to first window (TODO: proper window targeting)
                    if !self.windows.is_empty() {
                        let width = self.windows[0].window.width();
                        let height = self.windows[0].window.height();
                        let frame = Rect::new(0, 0, width, height);
                        Self::dispatch_event_to_view(&mut self.windows[0].window, event, frame);
                    }
                }
            }
            SwsEvent::SurfaceDestroyed { surface_id } => {
                // Server destroyed the surface; drop the corresponding window.
                self.windows.retain(|w| w.surface_id != surface_id);
            }
            SwsEvent::Error { code: _ } => {
                // Handle error
            }
        }
    }

    /// Convert SWS InputEvent to UI Event
    fn convert_input(&mut self, input: &InputEvent) -> Option<Event> {
        use sws_client::event::{abs_code, event_type, key_code};

        match input.type_ {
            event_type::EV_KEY => {
                match input.code {
                    key_code::BTN_LEFT | key_code::BTN_RIGHT | key_code::BTN_MIDDLE => {
                        let button = match input.code {
                            key_code::BTN_LEFT => MouseButton::Left,
                            key_code::BTN_RIGHT => MouseButton::Right,
                            key_code::BTN_MIDDLE => MouseButton::Middle,
                            _ => MouseButton::Left,
                        };
                        if input.value != 0 {
                            Some(Event::mouse_down(self.last_mouse.x, self.last_mouse.y, button))
                        } else {
                            Some(Event::mouse_up(self.last_mouse.x, self.last_mouse.y, button))
                        }
                    }
                    _ => {
                        // Keyboard event
                        if input.value != 0 {
                            Some(Event::key_down(input.code))
                        } else {
                            Some(Event::key_up(input.code))
                        }
                    }
                }
            }
            event_type::EV_ABS => {
                match input.code {
                    abs_code::ABS_X => {
                        self.last_mouse.x = input.value;
                        Some(Event::mouse_move(self.last_mouse.x, self.last_mouse.y))
                    }
                    abs_code::ABS_Y => {
                        self.last_mouse.y = input.value;
                        Some(Event::mouse_move(self.last_mouse.x, self.last_mouse.y))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Dispatch event to a view using capture/bubble phases
    fn dispatch_event_to_view(view: &mut dyn View, mut event: Event, frame: Rect) {
        // Phase 1: CAPTURE (root → target)
        if view.on_event_capture(&mut event, frame) {
            view.set_needs_draw();
            return;
        }
        if event.is_stopped() {
            view.set_needs_draw();
            return;
        }
        
        // Phase 2: BUBBLE (target → root)
        if Self::dispatch_bubble(view, &mut event, frame) || event.is_stopped() {
            view.set_needs_draw();
        }
    }

    /// Dispatch event in bubble phase recursively
    fn dispatch_bubble(view: &mut dyn View, event: &mut Event, frame: Rect) -> bool {
        // First dispatch to children
        for (child, child_frame) in view.children_mut() {
            if child_frame.contains(event.x(), event.y()) {
                if Self::dispatch_bubble(child, event, child_frame) {
                    return true;
                }
                if event.is_stopped() {
                    return true;
                }
            }
        }
        
        // Then handle on this view
        view.on_event(event, frame)
    }
}
