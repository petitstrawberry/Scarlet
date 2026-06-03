//! Application Trait - Main entry point for ScarletUI applications
//!
//! The Application trait provides the main loop and lifecycle management
//! for ScarletUI applications.

use crate::element::{Element, ElementId, LayoutConstraints, UpdateResult, WindowSizeLimits};
use crate::error::Result;
use crate::event::Event;
use crate::geometry::Size;
use crate::geometry::{Point, Rect};
use crate::menu_model;
use crate::pipeline::RenderingPipeline;
use crate::platform::{PlatformWindow, SWSPlatformWindow};
use crate::state::{InvalidationKind, StateId, SubscriptionId};
use crate::view::View;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
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

    /// Configure the created platform window before the main loop starts.
    ///
    /// Applications can override this to register optional window-scoped
    /// protocols such as text-input contexts.
    ///
    /// # Arguments
    ///
    /// * `window` - The SWS platform window created for this application.
    fn on_window_created(&mut self, _window: &mut SWSPlatformWindow) {
        // Default: do nothing
    }

    /// Synchronize application-managed window state.
    ///
    /// Applications that manage window-scoped protocols directly can override
    /// this hook to update those protocol states during the main loop.
    ///
    /// # Arguments
    ///
    /// * `window` - The SWS platform window for this application.
    fn on_window_sync(&mut self, _window: &mut SWSPlatformWindow) {
        // Default: do nothing
    }

    /// Handle committed text from an input method.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Text-input context that received the commit.
    /// * `serial` - Server serial for the text-input update.
    /// * `text` - UTF-8 text committed by the input method.
    fn on_text_input_commit(&mut self, _context_id: u32, _serial: u32, _text: &str) {
        // Default: do nothing
    }

    /// Handle preedit text from an input method.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Text-input context that received the preedit update.
    /// * `serial` - Server serial for the text-input update.
    /// * `cursor_byte` - UTF-8 byte offset of the preedit cursor.
    /// * `anchor_byte` - UTF-8 byte offset of the active preedit segment.
    /// * `text` - UTF-8 preedit text.
    /// * `spans` - Encoded preedit style spans.
    fn on_text_input_preedit(
        &mut self,
        _context_id: u32,
        _serial: u32,
        _cursor_byte: u32,
        _anchor_byte: u32,
        _text: &str,
        _spans: &[u8],
    ) {
        // Default: do nothing
    }

    /// Handle a request to delete text around the cursor from an input method.
    ///
    /// # Arguments
    ///
    /// * `context_id` - Text-input context that received the request.
    /// * `serial` - Server serial for the text-input update.
    /// * `before_bytes` - Bytes before the cursor to delete.
    /// * `after_bytes` - Bytes after the cursor to delete.
    fn on_text_input_delete_surrounding_text(
        &mut self,
        _context_id: u32,
        _serial: u32,
        _before_bytes: u32,
        _after_bytes: u32,
    ) {
        // Default: do nothing
    }

    /// Handle window resize event
    ///
    /// Called when the window is resized. Applications like TaskBar
    /// can override this to update their internal state (e.g., screen_width).
    /// Default implementation does nothing.
    fn on_resize(&mut self, _width: u32, _height: u32) {
        // Default: do nothing
    }

    /// Handle display size change event
    ///
    /// Called when the compositor reports a new physical screen size.
    /// Default implementation does nothing.
    fn on_screen_size_changed(&mut self, _width: u32, _height: u32) -> Option<Size> {
        // Default: do nothing
        None
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

    /// Handle idle ticks on the application main thread.
    ///
    /// This is called once per main-loop iteration after pending platform and
    /// window events are processed and before rendering. Applications can use
    /// this to drain messages from worker threads and update UI state without
    /// mutating `State` from background threads.
    fn on_idle(&mut self) {
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

        let output_scale_milli = SWSPlatformWindow::query_output_scale();
        pipeline.set_scale_milli(output_scale_milli);

        // 4. Perform initial layout to determine window size and extract window properties
        let (
            app_id,
            window_title,
            window_size,
            window_type,
            menu_bar,
            focus_on_create,
            active_on_focus,
        ) = pipeline.layout_initial();

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
            SWSPlatformWindow::new_with_menu_and_policies(
                &app_id,
                &window_title,
                window_size,
                &menu_json,
                focus_on_create,
                active_on_focus,
            )
            .map_err(|_| crate::error::Error::WindowCreationFailed)?
        } else {
            SWSPlatformWindow::create_with_type_and_menu_and_policies(
                &app_id,
                &window_title,
                window_size,
                window_type,
                &menu_json,
                focus_on_create,
                active_on_focus,
            )
            .map_err(|_| crate::error::Error::WindowCreationFailed)?
        };

        if let Some(menu_bar) = menu_bar {
            if !menu_json.is_empty() {
                let _ = platform_window.set_menu_titles(&menu_json);
                menu_model::register_menu_callbacks(platform_window.surface_id(), &menu_bar);
            }
        }

        self.on_window_created(&mut platform_window);

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
        sync_text_input(&mut platform_window, &pipeline);

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
                            self.on_resize(width, height);
                            sync_text_input(&mut platform_window, &pipeline);
                            if let Some(buffer) = pipeline.render() {
                                platform_window.present(buffer);
                                presented_this_cycle = true;
                            }
                        }
                    }
                    Event::ScreenSizeChanged { width, height } => {
                        let resize_to = self.on_screen_size_changed(width, height);
                        if let Some(new_size) = resize_to {
                            let new_width = new_size.width.max(1.0) as u32;
                            let new_height = new_size.height.max(1.0) as u32;
                            if platform_window.resize(new_width, new_height).is_ok() {
                                pipeline.resize(new_size);
                                sync_text_input(&mut platform_window, &pipeline);
                            }
                        }
                        if !presented_this_cycle && (resize_to.is_some() || pipeline.has_dirty()) {
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
                    Event::TextInputCommit {
                        context_id,
                        serial,
                        text,
                    } => {
                        let event = Event::TextInputCommit {
                            context_id,
                            serial,
                            text,
                        };
                        if !pipeline.handle_event(&event)
                            && let Event::TextInputCommit {
                                context_id,
                                serial,
                                text,
                            } = &event
                        {
                            self.on_text_input_commit(*context_id, *serial, text);
                        }
                        sync_text_input(&mut platform_window, &pipeline);
                        if !presented_this_cycle
                            && pipeline.has_dirty()
                            && let Some(buffer) = pipeline.render()
                        {
                            platform_window.present(buffer);
                            presented_this_cycle = true;
                        }
                    }
                    Event::TextInputPreedit {
                        context_id,
                        serial,
                        cursor_byte,
                        anchor_byte,
                        text,
                        spans,
                    } => {
                        let event = Event::TextInputPreedit {
                            context_id,
                            serial,
                            cursor_byte,
                            anchor_byte,
                            text,
                            spans,
                        };
                        if !pipeline.handle_event(&event)
                            && let Event::TextInputPreedit {
                                context_id,
                                serial,
                                cursor_byte,
                                anchor_byte,
                                text,
                                spans,
                                ..
                            } = &event
                        {
                            self.on_text_input_preedit(
                                *context_id,
                                *serial,
                                *cursor_byte,
                                *anchor_byte,
                                text,
                                spans,
                            );
                        }
                        sync_text_input(&mut platform_window, &pipeline);
                        if !presented_this_cycle
                            && pipeline.has_dirty()
                            && let Some(buffer) = pipeline.render()
                        {
                            platform_window.present(buffer);
                            presented_this_cycle = true;
                        }
                    }
                    Event::TextInputDeleteSurroundingText {
                        context_id,
                        serial,
                        before_bytes,
                        after_bytes,
                    } => {
                        let event = Event::TextInputDeleteSurroundingText {
                            context_id,
                            serial,
                            before_bytes,
                            after_bytes,
                        };
                        if !pipeline.handle_event(&event)
                            && let Event::TextInputDeleteSurroundingText {
                                context_id,
                                serial,
                                before_bytes,
                                after_bytes,
                            } = event
                        {
                            self.on_text_input_delete_surrounding_text(
                                context_id,
                                serial,
                                before_bytes,
                                after_bytes,
                            );
                        }
                        sync_text_input(&mut platform_window, &pipeline);
                        if !presented_this_cycle
                            && pipeline.has_dirty()
                            && let Some(buffer) = pipeline.render()
                        {
                            platform_window.present(buffer);
                            presented_this_cycle = true;
                        }
                    }
                    Event::TextInputDone { context_id, serial } => {
                        let event = Event::TextInputDone { context_id, serial };
                        let _ = pipeline.handle_event(&event);
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
                                let len = u32::from_le_bytes(
                                    data[*offset..*offset + 4].try_into().unwrap(),
                                ) as usize;
                                *offset += 4;
                                if *offset + len > data.len() {
                                    return String::new();
                                }
                                let s = core::str::from_utf8(&data[*offset..*offset + len])
                                    .unwrap_or("");
                                *offset += len;
                                String::from(s)
                            };

                            // Skip app_id
                            let _app_id = read_str(&data, &mut offset);
                            let app_name = read_str(&data, &mut offset);
                            let _title = read_str(&data, &mut offset);
                            let menu_titles = read_str(&data, &mut offset);

                            // Box large strings to move them to heap and reduce stack pressure
                            let app_name: Box<str> = app_name.into_boxed_str();
                            let menu_titles: Box<str> = menu_titles.into_boxed_str();

                            if crate::debug::is_enabled() {
                                println!(
                                    "[Application] FocusChanged: window_id={}, app_name={}, menu_titles={}",
                                    window_id, app_name, menu_titles
                                );
                            }
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
                                let len = u32::from_le_bytes(
                                    data[*offset..*offset + 4].try_into().unwrap(),
                                ) as usize;
                                *offset += 4;
                                if *offset + len > data.len() {
                                    return String::new();
                                }
                                let s = core::str::from_utf8(&data[*offset..*offset + len])
                                    .unwrap_or("");
                                *offset += len;
                                String::from(s)
                            };

                            // Skip app_id
                            let _app_id = read_str(&data, &mut offset);
                            let app_name = read_str(&data, &mut offset);
                            let _title = read_str(&data, &mut offset);
                            let menu_titles = read_str(&data, &mut offset);

                            // Box large strings to move them to heap and reduce stack pressure
                            let app_name: Box<str> = app_name.into_boxed_str();
                            let menu_titles: Box<str> = menu_titles.into_boxed_str();

                            if crate::debug::is_enabled() {
                                println!(
                                    "[Application] ActiveAppChanged: window_id={}, app_name={}, menu_titles={}",
                                    window_id, app_name, menu_titles
                                );
                            }
                            self.on_active_app_changed(window_id, &app_name, &menu_titles);

                            // Force redraw after active app changed (State updates trigger dirty flag)
                            if !presented_this_cycle && pipeline.has_dirty() {
                                if crate::debug::is_enabled() {
                                    println!(
                                        "[Application] ActiveAppChanged triggered redraw, has_dirty=true"
                                    );
                                }
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
                        sync_text_input(&mut platform_window, &pipeline);
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
            self.on_idle();
            self.on_window_sync(&mut platform_window);
            sync_text_input(&mut platform_window, &pipeline);
            if !presented_this_cycle && pipeline.has_dirty() {
                if crate::debug::is_enabled() {
                    println!("[Application] has_dirty=true, calling render()");
                }
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

fn sync_text_input(window: &mut SWSPlatformWindow, pipeline: &RenderingPipeline) {
    let state = pipeline.focused_text_input_state();
    window.sync_text_input(state.as_ref());
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
            let focused_path = self
                .child
                .as_ref()
                .and_then(|child| crate::element::focused_descendant_path(child.as_ref()));
            self.app = app.clone();
            self.child = Some(self.app.body().create_element());
            if let (Some(path), Some(child)) = (focused_path.as_deref(), self.child.as_mut()) {
                crate::element::restore_focus_at_path(child.as_mut(), path);
            }
            UpdateResult::Updated
        } else {
            UpdateResult::Replaced
        }
    }

    fn rebuild(&mut self) -> UpdateResult {
        if crate::debug::is_enabled() {
            println!(
                "[ApplicationRootElement] rebuild() called for id={}",
                self.id.get()
            );
        }
        let focused_path = self
            .child
            .as_ref()
            .and_then(|child| crate::element::focused_descendant_path(child.as_ref()));
        if let Some(ref mut child) = self.child {
            child.unmount();
        }
        self.child = Some(self.app.body().create_element());
        if let Some(ref mut child) = self.child {
            child.mount();
            if let Some(path) = focused_path.as_deref() {
                crate::element::restore_focus_at_path(child.as_mut(), path);
            }
        }
        UpdateResult::Updated
    }

    fn mount(&mut self) {
        let listenables = self.app.listenables();
        if crate::debug::is_enabled() {
            println!(
                "[ApplicationRootElement] mount() called: {} listenables found",
                listenables.len()
            );
        }
        for listenable in listenables {
            let element_id = self.id;
            let invalidation_kind = listenable.invalidation_kind();
            if crate::debug::is_enabled() {
                println!(
                    "[ApplicationRootElement] Subscribing to element_id={}",
                    element_id.get()
                );
            }
            let callback = alloc::sync::Arc::new(move || match invalidation_kind {
                InvalidationKind::Build => crate::pipeline::mark_element_dirty(element_id),
                InvalidationKind::Paint => crate::pipeline::mark_element_needs_paint(element_id),
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
