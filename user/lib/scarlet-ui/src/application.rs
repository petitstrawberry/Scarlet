//! Application - Event loop and window management
//!
//! Application manages the event loop, window lifecycle, and integrates with
//! the new View architecture (ViewRegistry, RenderTracker, BufferPool).

extern crate alloc;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::string::{String, ToString};

use crate::{View, ViewRegistry, RenderTracker, view::Window, view::render::RenderObject};
use crate::event::{Event, EventKind, MouseButton};
use crate::graphics::{Canvas, Point, Rect};
use crate::layout::{LayoutConstraints, Size};
use crate::context::{EventCtx, LayoutCtx, PaintCtx};
use crate::view::BufferPool;
use crate::view::id::ViewId;
use crate::composition::Compositor;
use sws_client::{Connection, Event as SwsEvent, InputEvent};
use scarlet_std::sync::Mutex;
use core::time::Duration;
use scarlet_std::thread;
use scarlet_std::task;

/// Application delegate for lifecycle decisions
pub trait ApplicationDelegate {
    fn application_should_terminate_after_last_window_closed(&mut self) -> bool {
        false
    }
    fn application_will_terminate(&mut self) {}
}

/// Managed window with surface binding
struct ManagedWindow {
    window: Window,
    surface_id: u32,
    is_maximized: bool,
    x: i32,
    y: i32,
}

/// Application - Event loop and window management
pub struct Application {
    connection: Connection,
    windows: Vec<ManagedWindow>,
    last_mouse: Point,
    mouse_capture_surface_id: Option<u32>,
    terminate_after_last_window_closed: bool,
    delegate: Option<Box<dyn ApplicationDelegate>>,
    app_id: Option<String>,
    layout_debug: bool,

    // View architecture
    registry: ViewRegistry,
    tracker: RenderTracker,
    buffer_pool: BufferPool,
    compositor: Compositor,

    // Command queue for async operations
    command_queue: Arc<Mutex<Vec<AppCommand>>>,
}

#[derive(Clone)]
pub struct ApplicationHandle {
    command_queue: Arc<Mutex<Vec<AppCommand>>>,
}

enum AppCommand {
    RequestPopup,
}

impl Application {
    pub fn new() -> Result<Self, &'static str> {
        let connection = Connection::connect_default()
            .map_err(|_| "Failed to connect to SWS")?;

        Ok(Self {
            connection,
            windows: Vec::new(),
            last_mouse: Point::new(0, 0),
            mouse_capture_surface_id: None,
            terminate_after_last_window_closed: false,
            delegate: None,
            app_id: None,
            layout_debug: false,

            registry: ViewRegistry::new(),
            tracker: RenderTracker::new(),
            buffer_pool: BufferPool::new(),
            compositor: Compositor::new(),

            command_queue: Arc::new(Mutex::new(Vec::new())),
        })
    }

    // Builder methods
    pub fn app_id(&mut self, app_id: &str) -> &mut Self {
        self.app_id = Some(app_id.to_string());
        self
    }

    pub fn set_terminate_after_last_window_closed(&mut self, enabled: bool) {
        self.terminate_after_last_window_closed = enabled;
    }

    pub fn set_delegate<D: ApplicationDelegate + 'static>(&mut self, delegate: D) {
        self.delegate = Some(Box::new(delegate));
    }

    pub fn set_layout_debug(&mut self, enabled: bool) {
        self.layout_debug = enabled;
    }

    pub fn handle(&self) -> ApplicationHandle {
        ApplicationHandle {
            command_queue: self.command_queue.clone(),
        }
    }

    // Window management
    pub fn add_window(&mut self, window: Window) -> Result<(), &'static str> {
        let width = window.width();
        let height = window.height();
        let app_id = window.get_app_id().unwrap_or("app");
        let title = window.title();

        // Create surface
        let surface_id = self.connection.create_surface(
            app_id,
            title,
            "",  // menu_titles
            width,
            height,
        ).map_err(|_| "Failed to create window")?;

        let window_id = window.id();

        self.windows.push(ManagedWindow {
            window,
            surface_id,
            is_maximized: false,
            x: 100,
            y: 100,
        });

        // Mark the new window as dirty so it gets rendered
        self.tracker.mark_dirty_layout(window_id);
        self.tracker.mark_dirty_paint(window_id);

        Ok(())
    }

    // Event loop
    pub fn run(&mut self) -> ! {
        loop {
            // SWS I/O
            let _ = self.connection.dispatch();

            // SWS events
            while let Some(sws_event) = self.connection.poll_event() {
                self.handle_sws_event(sws_event);
            }

            // Rendering
            self.render();

            // Termination check
            if self.should_terminate() {
                self.terminate();
            }

            // Frame rate limiting (~60 FPS)
            thread::sleep(Duration::from_millis(16));
        }
    }

    // SWS event handling
    fn handle_sws_event(&mut self, event: SwsEvent) {
        match event {
            SwsEvent::Input(input) => {
                if let Some(scarlet_event) = self.convert_input(&input) {
                    self.dispatch_event_to_window(input.surface_id, &scarlet_event);
                }
            }
            SwsEvent::SurfaceConfigure { surface_id, width, height } => {
                let window_id = if let Some(managed) = self.find_window_mut(surface_id) {
                    managed.window.set_size(width, height);
                    managed.window.id()
                } else {
                    return;
                };
                self.tracker.mark_dirty_layout(window_id);
            }
            SwsEvent::SurfaceDestroyed { surface_id } => {
                self.remove_window(surface_id);
            }
            _ => {}
        }
    }

    fn convert_input(&mut self, input: &InputEvent) -> Option<Event> {
        // Convert SWS InputEvent to ScarletUI Event
        // InputEvent has: surface_id, time, type_, code, value
        use sws_client::event::event_type::{EV_KEY, EV_REL, EV_ABS};
        use sws_client::event::abs_code::{ABS_X, ABS_Y};
        use sws_client::event::key_code::{BTN_LEFT, BTN_RIGHT, BTN_MIDDLE};

        match input.type_ {
            EV_ABS => {
                // Absolute position (mouse move)
                match input.code {
                    ABS_X => {
                        self.last_mouse.x = input.value;
                    }
                    ABS_Y => {
                        self.last_mouse.y = input.value;
                    }
                    _ => {}
                }
                Some(Event::mouse_move(self.last_mouse.x, self.last_mouse.y))
            }
            EV_KEY => {
                // Key or button event
                let mouse_button = match input.code {
                    BTN_LEFT => Some(MouseButton::Left),
                    BTN_RIGHT => Some(MouseButton::Right),
                    BTN_MIDDLE => Some(MouseButton::Middle),
                    _ => None,
                };

                if let Some(button) = mouse_button {
                    let kind = if input.value != 0 {
                        EventKind::MouseDown { button }
                    } else {
                        EventKind::MouseUp { button }
                    };
                    Some(Event::new(kind, self.last_mouse))
                } else {
                    // TODO: Keyboard events
                    None
                }
            }
            _ => None,
        }
    }

    fn dispatch_event_to_window(&mut self, surface_id: u32, event: &Event) {
        // First find the window and get its ID
        let window_id = if let Some(managed) = self.find_window_mut(surface_id) {
            managed.window.id()
        } else {
            return;
        };

        // Then use tracker to create context and dispatch
        let (windows, tracker) = (&mut self.windows, &mut self.tracker);
        if let Some(managed) = windows.iter_mut().find(|w| w.surface_id == surface_id) {
            let window = &mut managed.window;
            let mut event_ctx = EventCtx::new(window_id, event, tracker);
            let _ = window.event(&mut event_ctx, event);
        }
    }

    fn render(&mut self) {
        // Take dirty views from RenderTracker
        // DataContext now marks views dirty directly in the global tracker
        let mut dirty_layout = self.tracker.take_dirty_layout();
        let mut dirty_paint = self.tracker.take_dirty_paint();

        // For non-Window dirty views, find their parent Window and mark it dirty too
        // This ensures that when a child view (e.g., Text bound to DataContext) changes,
        // the entire Window gets redrawn
        for dirty_view_id in dirty_paint.iter().chain(dirty_layout.iter()) {
            // Check if this dirty view is a Window
            let is_window = self.windows.iter().any(|w| w.window.id() == *dirty_view_id);

            if !is_window {
                // This is a child view, mark all windows as dirty for now
                // TODO: Optimize by tracking which window contains this view
                for managed in &self.windows {
                    self.tracker.mark_dirty_paint(managed.window.id());
                }
                break; // All windows are now dirty, no need to check further
            }
        }

        // Add newly marked windows to dirty sets
        let additional_layout = self.tracker.take_dirty_layout();
        let additional_paint = self.tracker.take_dirty_paint();
        dirty_layout.extend(additional_layout);
        dirty_paint.extend(additional_paint);

        // Layout pass
        for managed in &mut self.windows {
            let window_id = managed.window.id();
            if dirty_layout.contains(&window_id) {
                Self::layout_window(managed);
            }
        }

        // Draw pass
        let connection = &mut self.connection;
        for managed in &mut self.windows {
            let window_id = managed.window.id();
            if dirty_paint.contains(&window_id) {
                Self::draw_window(managed, connection);
            }
        }
    }

    fn layout_window(managed: &mut ManagedWindow) {
        let window = &mut managed.window;
        let size = Size::new(window.width(), window.height());

        let mut layout_ctx = LayoutCtx::new(window.id());
        let _ = window.layout(&mut layout_ctx, LayoutConstraints::tight(size));
    }

    fn draw_window(managed: &mut ManagedWindow, connection: &mut Connection) {
        let window = &managed.window;
        let width = window.width();
        let height = window.height();

        // Get surface
        let surface = match connection.surface_mut(managed.surface_id) {
            Some(s) => s,
            None => return,
        };

        let mut canvas = Canvas::new(surface.buffer_mut(), width, height);

        let mut paint_ctx = PaintCtx::new(&mut canvas, window.id());
        let frame = Rect::new(0, 0, width, height);

        window.draw(&mut paint_ctx, frame);

        // Commit to SWS
        let _ = connection.commit(managed.surface_id);
    }

    fn find_window_mut(&mut self, surface_id: u32) -> Option<&mut ManagedWindow> {
        self.windows.iter_mut().find(|w| w.surface_id == surface_id)
    }

    fn remove_window(&mut self, surface_id: u32) {
        self.windows.retain(|w| w.surface_id != surface_id);
    }

    fn should_terminate(&self) -> bool {
        if self.windows.is_empty() && self.terminate_after_last_window_closed {
            true
        } else {
            false
        }
    }

    fn terminate(&mut self) -> ! {
        if let Some(ref mut delegate) = self.delegate {
            delegate.application_will_terminate();
        }
        task::exit(0);
    }
}

impl ApplicationHandle {
    pub fn request_popup(&self) {
        let mut queue = self.command_queue.lock();
        queue.push(AppCommand::RequestPopup);
    }
}
