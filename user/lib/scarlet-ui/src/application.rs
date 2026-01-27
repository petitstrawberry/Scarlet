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
use crate::menu_model;
use crate::state::StateId;
use crate::state::SubscriptionId;
use crate::element::{Element, ElementId, LayoutConstraints, UpdateResult, WindowSizeLimits};
use crate::geometry::{Point, Rect};
use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::String;
use core::any::Any;
use std::println;

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

    /// Handle focus change event from window server
    ///
    /// Called when another window gains focus. This allows applications
    /// like TaskBar to update their state based on the focused window.
    /// Default implementation does nothing.
    fn on_focus_changed(&mut self, _window_id: u32, _app_name: &str, _menu_titles: &str) {
        // Default: do nothing
    }

    /// Handle active application change event from window server
    ///
    /// Called when the active APPLICATION changes (normal window gains focus).
    /// This is separate from on_focus_changed because TaskBar/Desktop/etc
    /// can receive focus without changing the active application.
    /// This is used by TaskBar to update its menu bar.
    /// Default implementation does nothing.
    fn on_active_app_changed(&mut self, _window_id: u32, _app_name: &str, _menu_titles: &str) {
        // Default: do nothing
    }

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

    /// Enable or disable debug logging for this application
    fn debug_logging(&self) -> bool {
        false
    }

    /// Run the application main loop
    ///
    /// This sets up the window, runs the event loop, and renders the UI.
    /// The default implementation uses SWS as the platform backend.
    fn run(&mut self) -> Result<()>
    where
        Self: Sized + Clone,
    {
        crate::debug::set_enabled(self.debug_logging());

        // 1. Set up rendering pipeline
        let mut pipeline = RenderingPipeline::new();

        // 2. Create root element from body()
        let root_element = Box::new(ApplicationRootElement::new(self.clone()));
        pipeline.set_root(root_element);

        // 3. Initialize the application
        self.init();

        // 4. Perform initial layout to determine window size and extract window properties
        let (app_id, window_title, window_size, window_type, menu_bar) =
            pipeline.layout_initial();

        // Debug: Dump element tree
        if crate::debug::is_enabled() {
            pipeline.element_tree().dump();
        }

        let menu_json = menu_bar
            .as_ref()
            .map(|menu_bar| menu_bar.to_json())
            .unwrap_or_default();

        // 5. Create platform window (default: SWS backend)
        // Use create_with_type for special window types (TASKBAR, ALWAYS_ON_TOP)
        let mut platform_window = if window_type == crate::views::window_type::NORMAL {
            SWSPlatformWindow::new_with_menu(&app_id, &window_title, window_size, &menu_json)
                .map_err(|_| crate::error::Error::WindowCreationFailed)?
        } else {
            SWSPlatformWindow::create_with_type_and_menu(
                &app_id,
                &window_title,
                window_size,
                window_type,
                &menu_json,
            )
            .map_err(|_| crate::error::Error::WindowCreationFailed)?
        };

        if let Some(menu_bar) = menu_bar {
            if !menu_json.is_empty() {
                let _ = platform_window.set_menu_titles(&menu_json);
                menu_model::register_menu_callbacks(platform_window.surface_id(), &menu_bar);
            }
        }

        // Apply window size limits (resizable, etc.) from Window view
        if let Some(limits) = pipeline
            .element_tree()
            .root()
            .and_then(|r| find_window_size_limits(r))
        {
            if !limits.resizable {
                let _ = platform_window.set_resizable(false);
            }
        }

        // 6. Main event loop
        loop {
            let mut presented_this_cycle = false;
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
                        if platform_window.resize(width, height).is_ok() {
                            pipeline.resize(new_size);
                            if let Some(buffer) = pipeline.render() {
                                platform_window.present(buffer);
                                presented_this_cycle = true;
                            }
                        }
                    }
                    Event::MenuItemActivated {
                        window_id,
                        menu_item_id,
                    } => {
                        let _ = menu_model::invoke_menu_callback(window_id, &menu_item_id);
                    }
                    Event::Custom { event_type, data } if event_type == 0xF0C0F => {
                        // FocusChanged event from SWS
                        // Decode the data: window_id (u32) + app_id_len (u32) + app_id + app_name_len (u32) + app_name + title_len (u32) + title + menu_titles_len (u32) + menu_titles
                        let mut offset = 0;
                        if data.len() >= 4 {
                            let window_id = u32::from_le_bytes(data[0..4].try_into().unwrap());
                            offset = 4;

                            let read_str = |data: &[u8], offset: &mut usize| -> String {
                                if *offset + 4 > data.len() {
                                    return String::new();
                                }
                                let len = u32::from_le_bytes(data[*offset..*offset+4].try_into().unwrap()) as usize;
                                *offset += 4;
                                if *offset + len > data.len() {
                                    return String::new();
                                }
                                let s = core::str::from_utf8(&data[*offset..*offset+len]).unwrap_or("");
                                *offset += len;
                                String::from(s)
                            };

                            // Skip app_id
                            let _app_id = read_str(&data, &mut offset);
                            let app_name = read_str(&data, &mut offset);
                            let _title = read_str(&data, &mut offset);
                            let menu_titles = read_str(&data, &mut offset);

                            println!("[Application] FocusChanged: window_id={}, app_name={}, menu_titles={}", window_id, app_name, menu_titles);
                            self.on_focus_changed(window_id, &app_name, &menu_titles);
                        }
                    }
                    Event::Custom { event_type, data } if event_type == 0xF0C0A => {
                        // ActiveAppChanged event from SWS
                        // Decode the data (same format as FocusChanged)
                        let mut offset = 0;
                        if data.len() >= 4 {
                            let window_id = u32::from_le_bytes(data[0..4].try_into().unwrap());
                            offset = 4;

                            let read_str = |data: &[u8], offset: &mut usize| -> String {
                                if *offset + 4 > data.len() {
                                    return String::new();
                                }
                                let len = u32::from_le_bytes(data[*offset..*offset+4].try_into().unwrap()) as usize;
                                *offset += 4;
                                if *offset + len > data.len() {
                                    return String::new();
                                }
                                let s = core::str::from_utf8(&data[*offset..*offset+len]).unwrap_or("");
                                *offset += len;
                                String::from(s)
                            };

                            // Skip app_id
                            let _app_id = read_str(&data, &mut offset);
                            let app_name = read_str(&data, &mut offset);
                            let _title = read_str(&data, &mut offset);
                            let menu_titles = read_str(&data, &mut offset);

                            println!("[Application] ActiveAppChanged: window_id={}, app_name={}, menu_titles={}", window_id, app_name, menu_titles);
                            self.on_active_app_changed(window_id, &app_name, &menu_titles);

                            // Force redraw after active app changed (State updates trigger dirty flag)
                            if !presented_this_cycle && pipeline.has_dirty() {
                                println!("[Application] ActiveAppChanged triggered redraw, has_dirty=true");
                                if let Some(buffer) = pipeline.render() {
                                    platform_window.present(buffer);
                                    presented_this_cycle = true;
                                }
                            }
                        }
                    }
                    _ => {
                        // Other events are handled by the pipeline
                        let _ = pipeline.handle_event(&event);
                        if !presented_this_cycle && pipeline.has_dirty() {
                            if let Some(buffer) = pipeline.render() {
                                platform_window.present(buffer);
                                presented_this_cycle = true;
                            }
                        }
                    }
                }
            }

            // 6.2 Handle emitted Window events
            for emitted_event in pipeline.take_emitted_events() {
                match emitted_event {
                    Event::Window(crate::event::WindowEvent::CloseRequested) => {
                        // Close the window and exit the application
                        let _ = platform_window.close();
                        return Ok(());
                    }
                    Event::Window(crate::event::WindowEvent::MaximizeRequested) => {
                        let _ = platform_window.maximize();
                    }
                    Event::Window(crate::event::WindowEvent::RestoreRequested) => {
                        let _ = platform_window.restore();
                    }
                    Event::Window(crate::event::WindowEvent::MinimizeRequested) => {
                        let _ = platform_window.minimize();
                    }
                    Event::Window(crate::event::WindowEvent::MoveRequested) => {
                        let _ = platform_window.request_move();
                    }
                    _ => {}
                }
            }

            // 6.3 Render frame
            // if let Some(buffer) = pipeline.render() {
            //     platform_window.present(buffer);
            // }
            if !presented_this_cycle && pipeline.has_dirty() {
                println!("[Application] has_dirty=true, calling render()");
                if let Some(buffer) = pipeline.render() {
                    platform_window.present(buffer);
                }
            }

            // 6.4 Small sleep to prevent busy-waiting
            // In a real implementation, this would use proper frame timing
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}

fn find_window_size_limits(element: &dyn Element) -> Option<WindowSizeLimits> {
    if let Some(limits) = element.get_window_size_limits() {
        return Some(limits);
    }
    for child in element.children() {
        if let Some(limits) = find_window_size_limits(child.as_ref()) {
            return Some(limits);
        }
    }
    None
}

struct ApplicationRootElement<A: Application + Clone> {
    id: ElementId,
    app: A,
    child: Option<Box<dyn Element>>,
    size: crate::geometry::Size,
    position: Point,
    subscriptions: Vec<SubscriptionId>,
}

impl<A: Application + Clone> ApplicationRootElement<A> {
    fn new(app: A) -> Self {
        let id = ElementId::generate();
        let child = app.body().create_element();
        Self {
            id,
            app,
            child: Some(child),
            size: crate::geometry::Size::ZERO,
            position: Point::ZERO,
            subscriptions: Vec::new(),
        }
    }
}

impl<A: Application + Clone + 'static> Element for ApplicationRootElement<A> {
    fn id(&self) -> ElementId {
        self.id
    }

    fn type_name(&self) -> &str {
        "ApplicationRootElement"
    }

    fn type_name_debug(&self) -> alloc::string::String {
        alloc::format!("ApplicationRootElement<{}>", core::any::type_name::<A>())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn children(&self) -> &[Box<dyn Element>] {
        match &self.child {
            Some(child) => core::slice::from_ref(child),
            None => &[],
        }
    }

    fn children_mut(&mut self) -> &mut [Box<dyn Element>] {
        match &mut self.child {
            Some(child) => core::slice::from_mut(child),
            None => &mut [],
        }
    }

    fn update(&mut self, new_view: &dyn View) -> UpdateResult {
        if let Some(app) = new_view.as_any().downcast_ref::<A>() {
            self.app = app.clone();
            self.child = Some(self.app.body().create_element());
            UpdateResult::Updated
        } else {
            UpdateResult::Replaced
        }
    }

    fn rebuild(&mut self) -> UpdateResult {
        println!("[ApplicationRootElement] rebuild() called for id={}", self.id.get());
        if let Some(ref mut child) = self.child {
            child.unmount();
        }
        self.child = Some(self.app.body().create_element());
        if let Some(ref mut child) = self.child {
            child.mount();
        }
        UpdateResult::Updated
    }

    fn mount(&mut self) {
        let listenables = self.app.listenables();
        println!("[ApplicationRootElement] mount() called: {} listenables found", listenables.len());
        for listenable in listenables {
            let element_id = self.id;
            println!("[ApplicationRootElement] Subscribing to element_id={}", element_id.get());
            let callback = alloc::sync::Arc::new(move || {
                crate::pipeline::mark_element_dirty(element_id);
            });
            let subscription_id = listenable.subscribe_any(callback);
            self.subscriptions.push(subscription_id);
        }

        if let Some(ref mut child) = self.child {
            child.mount();
        }
    }

    fn unmount(&mut self) {
        if let Some(ref mut child) = self.child {
            child.unmount();
        }
        self.subscriptions.clear();
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> crate::geometry::Size {
        if let Some(ref mut child) = self.child {
            self.size = child.layout(constraints);
        } else {
            self.size = crate::geometry::Size::ZERO;
        }
        self.size
    }

    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, position: Point) {
        self.position = position;
        if let Some(ref mut child) = self.child {
            child.set_position(position);
        }
    }

    fn bounds(&self) -> Rect {
        Rect {
            origin: self.position,
            size: self.size,
        }
    }

    fn hit_test(&self, point: Point) -> bool {
        if let Some(ref child) = self.child {
            child.hit_test(point)
        } else {
            self.bounds().contains(point)
        }
    }

    fn handle_event(&mut self, event: &crate::event::Event, phase: crate::event::Phase) -> bool {
        if let Some(ref mut child) = self.child {
            child.handle_event(event, phase)
        } else {
            false
        }
    }
}
