//! Application - manages windows and the event loop

use crate::view::{Window, View, Size};
use crate::event::{Event, MouseButton};
use crate::graphics::{Canvas, Rect, Point};
use crate::Color;
use scarlet_std::vec::Vec;
use sws_client::{Connection, Event as SwsEvent, InputEvent};
use std::sync::{Arc, Mutex};
use core::time::Duration;

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
    layout_debug: bool,

    command_queue: Arc<Mutex<Vec<AppCommand>>>,
    popup_surface_id: Option<u32>,
    popup_follow_parent_move: bool,
    main_resized_large: bool,
}

#[derive(Clone)]
pub struct ApplicationHandle {
    command_queue: Arc<Mutex<Vec<AppCommand>>>,
}

impl ApplicationHandle {
    pub fn request_popup(&self) {
        self.command_queue.lock().push(AppCommand::CreatePopup);
    }

    pub fn toggle_popup_follow_parent_move(&self) {
        self.command_queue
            .lock()
            .push(AppCommand::TogglePopupFollowParentMove);
    }

    pub fn toggle_main_resize(&self) {
        self.command_queue.lock().push(AppCommand::ToggleMainResize);
    }
}

#[derive(Clone, Copy)]
enum AppCommand {
    CreatePopup,
    TogglePopupFollowParentMove,
    ToggleMainResize,
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
            layout_debug: false,

            command_queue: Arc::new(Mutex::new(Vec::new())),
            popup_surface_id: None,
            popup_follow_parent_move: false,
            main_resized_large: false,
        })
    }

    pub fn handle(&self) -> ApplicationHandle {
        ApplicationHandle {
            command_queue: self.command_queue.clone(),
        }
    }

    /// Enable or disable layout bounds visualization.
    ///
    /// When enabled, the framework draws rectangle outlines for the allocated
    /// frames of each view after the normal draw pass.
    pub fn set_layout_debug(&mut self, enabled: bool) {
        self.layout_debug = enabled;
        for w in &mut self.windows {
            w.window.set_needs_draw();
        }
    }

    fn debug_color(depth: u32) -> Color {
        match depth % 4 {
            0 => Color::RED,
            1 => Color::GREEN,
            2 => Color::BLUE,
            _ => Color::GRAY,
        }
    }

    fn draw_layout_debug(view: &dyn View, canvas: &mut Canvas, frame: Rect, depth: u32) {
        if frame.width > 0 && frame.height > 0 {
            canvas.stroke(frame, Self::debug_color(depth));
        }

        view.visit_children(&mut |child, rel| {
            let child_frame = Rect::new(frame.x + rel.x, frame.y + rel.y, rel.width, rel.height);
            Self::draw_layout_debug(child, canvas, child_frame, depth + 1);
            false
        });
    }

    /// Add a window to the application
    ///
    /// The window will be displayed and managed by the application.
    pub fn add_window(&mut self, window: Window) -> Result<(), &'static str> {
        let _ = self.add_window_inner(window)?;
        Ok(())
    }

    fn add_window_inner(&mut self, window: Window) -> Result<u32, &'static str> {
        let width = window.width();
        let height = window.height();

        let size_limits = window.get_size_limits();
        
        // Create surface
        let surface_id = self
            .connection
            .create_surface(width, height)
            .map_err(|_| "Failed to create surface")?;

        if size_limits != sws_client::WindowSizeLimits::NONE {
            self.connection
                .set_window_size_limits(surface_id, size_limits)
                .map_err(|_| "Failed to set window size limits")?;
        }
        
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
                if self.layout_debug {
                    Self::draw_layout_debug(&managed.window, &mut canvas, frame, 0);
                }
                managed.window.clear_needs_draw();
            }
        }

        // Commit to display
        self.connection.commit(surface_id).map_err(|_| "Failed to commit")?;
        
        self.windows.push(managed);
        Ok(surface_id)
    }

    /// Run the application event loop
    ///
    /// This method never returns. It handles all events, layout,
    /// and drawing automatically.
    pub fn run(&mut self) -> ! {
        loop {
            // 1. Dispatch socket I/O
            let _ = self.connection.dispatch();

            // 2. Drain all pending events without O(n^2) shifting
            let events: Vec<SwsEvent> = self.connection.drain_events();

            // 3. Process drained events
            for sws_event in events.iter().copied() {
                self.handle_sws_event(sws_event);
            }

            // 3b. Process application commands requested by UI callbacks.
            let commands = {
                let mut queue = self.command_queue.lock();
                core::mem::take(&mut *queue)
            };
            for cmd in commands {
                self.handle_command(cmd);
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

            // 4b. Handle move requests (send REQUEST_MOVE_WINDOW to SWS)
            let mut move_surface_ids: Vec<u32> = Vec::new();
            for i in 0..self.windows.len() {
                if self.windows[i].window.take_move_requested() {
                    move_surface_ids.push(self.windows[i].surface_id);
                }
            }
            for surface_id in move_surface_ids {
                let _ = self.connection.request_move_window(surface_id);
            }

            // 5. Drop closed windows (and destroy their surfaces)
            let mut closed_surface_ids: Vec<u32> = Vec::new();
            for w in &self.windows {
                if w.window.is_close_requested() {
                    closed_surface_ids.push(w.surface_id);
                }
            }
            for surface_id in closed_surface_ids {
                let _ = self.connection.destroy_surface(surface_id);
                if self.popup_surface_id == Some(surface_id) {
                    self.popup_surface_id = None;
                }
            }
            self.windows.retain(|w| !w.window.is_close_requested());
            
            // 6. Layout and draw windows
            let mut did_draw = false;
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
                        if self.layout_debug {
                            Self::draw_layout_debug(&managed.window, &mut canvas, frame, 0);
                        }
                        managed.window.clear_needs_draw();
                        let _ = self.connection.commit(managed.surface_id);
                        did_draw = true;
                    }
                }
            }

            // 7. Avoid a busy loop when idle.
            if events.is_empty() && !did_draw {
                let _ = scarlet_std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn handle_command(&mut self, cmd: AppCommand) {
        match cmd {
            AppCommand::CreatePopup => {
                if self.popup_surface_id.is_some() {
                    return;
                }
                if self.windows.is_empty() {
                    return;
                }

                let main_surface_id = self.windows[0].surface_id;

                // Create a small popup window.
                let popup_window = Window::new("Popup", 240, 140)
                    .background(Color::WHITE)
                    .content(
                        crate::view::Padding::new(crate::view::Label::new("Popup")
                            .color(Color::TEXT))
                            .all(10),
                    );

                let popup_surface_id = match self.add_window_inner(popup_window) {
                    Ok(id) => id,
                    Err(_) => return,
                };

                let _ = self
                    .connection
                    .set_window_parent(popup_surface_id, Some(main_surface_id));

                let flags = (if self.popup_follow_parent_move {
                    sws_client::TransientFlags::FOLLOW_PARENT_MOVE
                } else {
                    sws_client::TransientFlags::NONE
                }) | sws_client::TransientFlags::RAISE_WITH_PARENT;
                let _ = self
                    .connection
                    .set_window_transient_flags(popup_surface_id, flags);

                self.popup_surface_id = Some(popup_surface_id);
            }
            AppCommand::TogglePopupFollowParentMove => {
                self.popup_follow_parent_move = !self.popup_follow_parent_move;
                if let Some(popup_surface_id) = self.popup_surface_id {
                    let flags = (if self.popup_follow_parent_move {
                        sws_client::TransientFlags::FOLLOW_PARENT_MOVE
                    } else {
                        sws_client::TransientFlags::NONE
                    }) | sws_client::TransientFlags::RAISE_WITH_PARENT;
                    let _ = self
                        .connection
                        .set_window_transient_flags(popup_surface_id, flags);
                }
            }
            AppCommand::ToggleMainResize => {
                if self.windows.is_empty() {
                    return;
                }

                self.main_resized_large = !self.main_resized_large;
                let (w, h) = if self.main_resized_large { (640, 480) } else { (400, 300) };

                let surface_id = self.windows[0].surface_id;
                if self.connection.resize_window(surface_id, w, h).is_ok() {
                    self.windows[0].window.set_size(w, h);
                    self.windows[0].window.set_needs_draw();
                }
            }
        }
    }

    /// Handle a SWS event
    fn handle_sws_event(&mut self, sws_event: SwsEvent) {
        match sws_event {
            SwsEvent::Input(input) => {
                if let Some(event) = self.convert_input(&input) {
                    if let Some(index) = self
                        .windows
                        .iter()
                        .position(|w| w.surface_id == input.surface_id)
                    {
                        let (width, height) = {
                            let window = &self.windows[index].window;
                            (window.width(), window.height())
                        };
                        let frame = Rect::new(0, 0, width, height);
                        Self::dispatch_event_to_view(&mut self.windows[index].window, event, frame);
                    }
                }
            }
            SwsEvent::SurfaceDestroyed { surface_id } => {
                // Server destroyed the surface; drop the corresponding window.
                self.windows.retain(|w| w.surface_id != surface_id);
                if self.popup_surface_id == Some(surface_id) {
                    self.popup_surface_id = None;
                }
            }
            SwsEvent::SurfaceConfigure {
                surface_id,
                width,
                height,
            } => {
                if let Some(index) = self
                    .windows
                    .iter()
                    .position(|w| w.surface_id == surface_id)
                {
                    if self.connection.resize_window(surface_id, width, height).is_ok() {
                        // The server may clamp the requested size; use the post-resize surface size.
                        if let Some(surface) = self.connection.surface(surface_id) {
                            self.windows[index]
                                .window
                                .set_size(surface.width(), surface.height());
                        } else {
                            self.windows[index].window.set_size(width, height);
                        }
                        self.windows[index].window.set_needs_draw();
                    }
                }
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
        let mut consumed = false;
        view.visit_children_mut(&mut |child, child_frame| {
            if consumed {
                return true;
            }

            let abs = Rect::new(
                frame.x + child_frame.x,
                frame.y + child_frame.y,
                child_frame.width,
                child_frame.height,
            );

            if abs.contains(event.x(), event.y()) {
                if Self::dispatch_bubble(child, event, abs) {
                    consumed = true;
                    return true;
                }
                if event.is_stopped() {
                    consumed = true;
                    return true;
                }
            }

            false
        });

        if consumed {
            return true;
        }
        
        // Then handle on this view
        view.on_event(event, frame)
    }
}
