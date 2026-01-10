//! Application - manages windows and the event loop

use crate::view::{Window, View, Size};
use crate::event::{Event, MouseButton};
use crate::graphics::{Canvas, Rect, Point};
use crate::Color;
use scarlet_std::boxed::Box;
use scarlet_std::vec::Vec;
use sws_client::{Connection, Event as SwsEvent, InputEvent};
use std::sync::{Arc, Mutex};
use core::time::Duration;

/// Application-level delegate for lifecycle decisions.
///
/// This is inspired by AppKit's `NSApplicationDelegate`.
pub trait ApplicationDelegate {
    /// Called after the last window is closed.
    ///
    /// Return `true` to terminate the process, `false` to keep running.
    ///
    /// AppKit equivalent: `applicationShouldTerminateAfterLastWindowClosed`.
    fn application_should_terminate_after_last_window_closed(&mut self) -> bool {
        false
    }

    /// Called immediately before terminating the process.
    fn application_will_terminate(&mut self) {}
}

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

    // Mouse capture: while the left button is down, keep routing pointer
    // move/up events to the surface where the press started.
    mouse_capture_surface_id: Option<u32>,

    terminate_after_last_window_closed: bool,
    delegate: Option<Box<dyn ApplicationDelegate>>,

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

            mouse_capture_surface_id: None,

            // AppKit default is to keep the app running with no windows.
            terminate_after_last_window_closed: false,
            delegate: None,

            command_queue: Arc::new(Mutex::new(Vec::new())),
            popup_surface_id: None,
            popup_follow_parent_move: false,
            main_resized_large: false,
        })
    }

    /// Configure whether the application terminates when the last window is closed.
    ///
    /// Default: `false` (AppKit-like behavior).
    pub fn set_terminate_after_last_window_closed(&mut self, enabled: bool) {
        self.terminate_after_last_window_closed = enabled;
    }

    /// Install an application delegate for lifecycle decisions.
    pub fn set_delegate<D: ApplicationDelegate + 'static>(&mut self, delegate: D) {
        self.delegate = Some(Box::new(delegate));
    }

    fn should_terminate_after_last_window_closed(&mut self) -> bool {
        if let Some(d) = self.delegate.as_mut() {
            d.application_should_terminate_after_last_window_closed()
        } else {
            self.terminate_after_last_window_closed
        }
    }

    fn terminate(&mut self) -> ! {
        if let Some(d) = self.delegate.as_mut() {
            d.application_will_terminate();
        }
        std::task::exit(0)
    }

    pub fn handle(&self) -> ApplicationHandle {
        ApplicationHandle {
            command_queue: self.command_queue.clone(),
        }
    }

    /// Collect dirty rects from subviews that need redraw
    fn collect_dirty_rects(view: &dyn View, parent_frame: Rect, rects: &mut Vec<Rect>) {
        view.visit_children(&mut |child, rel| {
            let child_frame = Rect::new(
                parent_frame.x + rel.x,
                parent_frame.y + rel.y,
                rel.width,
                rel.height,
            );
            if child.needs_draw() {
                rects.push(child_frame);
            }
            // Always recurse to find all dirty children
            Self::collect_dirty_rects(child, child_frame, rects);
            false
        });
    }

    /// Union multiple rects into a bounding box
    fn union_rects(rects: &[Rect]) -> Option<Rect> {
        if rects.is_empty() {
            return None;
        }
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;
        for r in rects {
            min_x = min_x.min(r.x);
            min_y = min_y.min(r.y);
            max_x = max_x.max(r.x + r.width as i32);
            max_y = max_y.max(r.y + r.height as i32);
        }
        if max_x <= min_x || max_y <= min_y {
            return None;
        }
        Some(Rect::new(min_x, min_y, (max_x - min_x) as u32, (max_y - min_y) as u32))
    }

    /// Recursively clear needs_draw flags on all views
    fn clear_all_needs_draw(view: &mut dyn View, _frame: Rect) {
        view.clear_needs_draw();
        view.visit_children_mut(&mut |child, _rel| {
            Self::clear_all_needs_draw(child, _rel);
            false
        });
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
        let mut last_time_ms = 0u64;

        // Scratch buffers to avoid per-frame heap allocations.
        let mut scratch_surface_ids: Vec<u32> = Vec::new();
        let mut scratch_move_surface_ids: Vec<u32> = Vec::new();
        let mut scratch_dirty_rects: Vec<Rect> = Vec::new();
        
        loop {
            // Get current time in milliseconds (approximate)
            let current_time_ms = last_time_ms + 16; // Approximate 60 FPS
            last_time_ms = current_time_ms;
            
            // Process timers
            crate::timer::process_timers(current_time_ms);
            
            // Process main thread queue
            crate::timer::process_main_thread_queue();
            
            // 1. Dispatch socket I/O
            let _ = self.connection.dispatch();

            // 2. Process all pending events without allocating a Vec.
            while let Some(sws_event) = self.connection.poll_event() {
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
            scratch_surface_ids.clear();
            for managed in &self.windows {
                if managed.window.is_close_requested() {
                    scratch_surface_ids.push(managed.surface_id);
                }
            }
            for surface_id in scratch_surface_ids.drain(..) {
                // If the surface is already gone (e.g. server-side destroyed), ignore.
                let _ = self.connection.destroy_surface(surface_id);
                if self.popup_surface_id == Some(surface_id) {
                    self.popup_surface_id = None;
                }
            }
            self.windows.retain(|w| !w.window.is_close_requested());

            // 4b. Handle move requests (send REQUEST_MOVE_WINDOW to SWS)
            scratch_move_surface_ids.clear();
            for i in 0..self.windows.len() {
                if self.windows[i].window.take_move_requested() {
                    scratch_move_surface_ids.push(self.windows[i].surface_id);
                }
            }
            for surface_id in scratch_move_surface_ids.drain(..) {
                let _ = self.connection.request_move_window(surface_id);
            }

            if self.windows.is_empty() && self.should_terminate_after_last_window_closed() {
                self.terminate();
            }
            
            // 6. Layout and draw windows
            let mut did_draw = false;
            for i in 0..self.windows.len() {
                let managed = &mut self.windows[i];
                let size = Size::new(managed.window.width(), managed.window.height());
                managed.window.layout(size);
                
                let width = managed.window.width();
                let height = managed.window.height();
                let full_frame = Rect::new(0, 0, width, height);
                
                // Check if window itself needs full redraw
                if managed.window.needs_draw() {
                    // Full redraw
                    if let Some(surface) = self.connection.surface_mut(managed.surface_id) {
                        let mut canvas = Canvas::new(surface.buffer_mut(), width, height);
                        managed.window.draw(&mut canvas, full_frame);
                        if self.layout_debug {
                            Self::draw_layout_debug(&managed.window, &mut canvas, full_frame, 0);
                        }
                    }
                    Self::clear_all_needs_draw(&mut managed.window, full_frame);
                    let _ = self.connection.commit(managed.surface_id);
                    did_draw = true;
                } else {
                    // Collect dirty rects from subviews
                    scratch_dirty_rects.clear();
                    Self::collect_dirty_rects(&managed.window, full_frame, &mut scratch_dirty_rects);
                    
                    if !scratch_dirty_rects.is_empty() {
                        // Redraw the whole window (since we can't easily do partial view draws)
                        // but only commit the dirty region
                        if let Some(surface) = self.connection.surface_mut(managed.surface_id) {
                            let mut canvas = Canvas::new(surface.buffer_mut(), width, height);
                            managed.window.draw(&mut canvas, full_frame);
                            if self.layout_debug {
                                Self::draw_layout_debug(&managed.window, &mut canvas, full_frame, 0);
                            }
                        }
                        Self::clear_all_needs_draw(&mut managed.window, full_frame);
                        
                        // Commit only the dirty region
                        if let Some(dirty_rect) = Self::union_rects(&scratch_dirty_rects) {
                            // Clamp damage to the surface bounds and never send empty regions.
                            let x0 = dirty_rect.x.max(0);
                            let y0 = dirty_rect.y.max(0);
                            let x1 = (dirty_rect.x.saturating_add(dirty_rect.width as i32))
                                .min(width as i32);
                            let y1 = (dirty_rect.y.saturating_add(dirty_rect.height as i32))
                                .min(height as i32);

                            if x1 > x0 && y1 > y0 {
                                let _ = self.connection.commit_region(
                                    managed.surface_id,
                                    x0 as u32,
                                    y0 as u32,
                                    (x1 - x0) as u32,
                                    (y1 - y0) as u32,
                                );
                            }
                        }
                        did_draw = true;
                    }
                }
            }

            // 7. Frame rate limiting: cap at ~60fps to reduce flicker and CPU usage.
            // Always sleep at least 1ms to yield CPU, and ensure minimum 16ms between frames.
            if did_draw {
                // We drew this frame, sleep briefly to cap frame rate
                let _ = scarlet_std::thread::sleep(Duration::from_millis(8));
            } else if !self.connection.has_events() {
                // No events and no draw, sleep longer
                let _ = scarlet_std::thread::sleep(Duration::from_millis(16));
            } else {
                // Events pending, yield briefly
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
                    // Determine which surface should receive this event.
                    // While captured, keep routing move/up to the capture surface.
                    let is_left_move_or_up_under_capture = matches!(
                        event.kind,
                        crate::event::EventKind::MouseMove
                            | crate::event::EventKind::MouseUp {
                                button: MouseButton::Left
                            }
                    );

                    let target_surface_id = if is_left_move_or_up_under_capture {
                        self.mouse_capture_surface_id.unwrap_or(input.surface_id)
                    } else {
                        input.surface_id
                    };

                    // Start capture on left mouse down.
                    if matches!(
                        event.kind,
                        crate::event::EventKind::MouseDown {
                            button: MouseButton::Left
                        }
                    ) {
                        self.mouse_capture_surface_id = Some(input.surface_id);
                    }

                    if let Some(index) = self
                        .windows
                        .iter()
                        .position(|w| w.surface_id == target_surface_id)
                    {
                        let (width, height) = {
                            let window = &self.windows[index].window;
                            (window.width(), window.height())
                        };
                        let frame = Rect::new(0, 0, width, height);

                        // While captured, ignore hit-test containment so dragging controls
                        // still receive move/up events even when the cursor leaves bounds.
                        let route_all = self.mouse_capture_surface_id == Some(target_surface_id)
                            && is_left_move_or_up_under_capture;
                        Self::dispatch_event_to_view(&mut self.windows[index].window, event, frame, route_all);
                    }

                    // End capture on left mouse up.
                    if matches!(
                        event.kind,
                        crate::event::EventKind::MouseUp {
                            button: MouseButton::Left
                        }
                    ) {
                        self.mouse_capture_surface_id = None;
                    }
                }
            }
            SwsEvent::SurfaceDestroyed { surface_id } => {
                // Server destroyed the surface; drop the corresponding window.
                self.windows.retain(|w| w.surface_id != surface_id);
                if self.popup_surface_id == Some(surface_id) {
                    self.popup_surface_id = None;
                }

                if self.windows.is_empty() && self.should_terminate_after_last_window_closed() {
                    self.terminate();
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
    fn dispatch_event_to_view(view: &mut dyn View, mut event: Event, frame: Rect, route_all: bool) {
        // Phase 1: CAPTURE (root → target)
        if view.on_event_capture(&mut event, frame) {
            return;
        }
        if event.is_stopped() {
            return;
        }
        
        // Phase 2: BUBBLE (target → root)
        let _ = Self::dispatch_bubble(view, &mut event, frame, route_all);
    }

    /// Dispatch event in bubble phase recursively
    fn dispatch_bubble(view: &mut dyn View, event: &mut Event, frame: Rect, route_all: bool) -> bool {
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

            if route_all || abs.contains(event.x(), event.y()) {
                if Self::dispatch_bubble(child, event, abs, route_all) {
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
        let handled = view.on_event(event, frame);
        if handled || event.is_stopped() {
            // Mark only the view that actually handled the event as dirty.
            // This keeps compositor damage regions small.
            view.set_needs_draw();
        }
        handled
    }
}
