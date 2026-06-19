//! Application entry point and multi-window runner for ScarletUI applications.

use alloc::boxed::Box;
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::any::Any;
use core::marker::PhantomData;
use std::time::Duration;

use crate::command::{self, ApplicationCommand};
use crate::element::{Element, ElementId, LayoutConstraints, UpdateResult, WindowSizeLimits};
use crate::error::{Error, Result};
use crate::event::Event;
use crate::geometry::{Point, Rect, Size};
use crate::menu_model;
use crate::pipeline::{MountContext, PipelineId, RenderingPipeline};
use crate::platform::{PlatformBackend, PlatformWindow, WindowCreateRequest};
use crate::scene::{
    Scene, SceneBuilder, SceneWindowKey, WindowContext, WindowDeclaration, WindowId,
};
use crate::state::{InvalidationKind, StateId, SubscriptionId};
use crate::view::View;

/// Application trait - main entry point for ScarletUI apps.
///
/// Applications declare top-level windows via `scenes()`.
pub trait Application: Clone + 'static {
    /// Returns the scene graph of top-level application windows.
    fn scenes(&self) -> impl Scene;

    /// Handle focus change event from window server.
    fn on_focus_changed(&mut self, _window_id: u32, _app_name: &str, _menu_titles: &str) {}

    /// Handle active application change event from window server.
    fn on_active_app_changed(&mut self, _window_id: u32, _app_name: &str, _menu_titles: &str) {}

    /// Configure a created platform window before the main loop starts.
    fn on_window_created(&mut self, _ctx: &WindowContext, _window: &mut dyn PlatformWindow) {}

    /// Synchronize application-managed window state.
    fn on_window_sync(&mut self, _ctx: &WindowContext, _window: &mut dyn PlatformWindow) {}

    /// Handle committed text from an input method.
    fn on_text_input_commit(
        &mut self,
        _ctx: &WindowContext,
        _context_id: u32,
        _serial: u32,
        _text: &str,
    ) {
    }

    /// Handle preedit text from an input method.
    fn on_text_input_preedit(
        &mut self,
        _ctx: &WindowContext,
        _context_id: u32,
        _serial: u32,
        _cursor_byte: u32,
        _anchor_byte: u32,
        _text: &str,
        _spans: &[u8],
    ) {
    }

    /// Handle a request to delete text around the cursor from an input method.
    fn on_text_input_delete_surrounding_text(
        &mut self,
        _ctx: &WindowContext,
        _context_id: u32,
        _serial: u32,
        _before_bytes: u32,
        _after_bytes: u32,
    ) {
    }

    /// Handle window resize event.
    fn on_window_resize(&mut self, _ctx: &WindowContext, _width: u32, _height: u32) {}

    /// Handle display size change event.
    fn on_screen_size_changed(&mut self, _width: u32, _height: u32) -> Option<Size> {
        None
    }

    /// Register all State instances used by this Application.
    fn register_states(&self) -> Vec<StateId> {
        Vec::new()
    }

    /// Initialize the application.
    fn init(&mut self) {}

    /// Handle idle ticks on the application main thread.
    fn on_idle(&mut self) {}

    /// Exit when all windows are closed.
    fn exit_when_all_windows_closed(&self) -> bool {
        true
    }

    /// Enable or disable debug logging for this application.
    fn debug_logging(&self) -> bool {
        false
    }
}

/// Backend-independent application runner.
pub struct ApplicationRunner {
    backend: Box<dyn PlatformBackend>,
}

impl ApplicationRunner {
    /// Create a runner backed by the supplied platform backend.
    ///
    /// # Arguments
    ///
    /// * `backend` - Platform backend used to create and drive windows
    ///
    /// # Returns
    ///
    /// A backend-independent application runner.
    pub fn new(backend: Box<dyn PlatformBackend>) -> Self {
        Self { backend }
    }

    /// Run an application using this runner's platform backend.
    ///
    /// # Arguments
    ///
    /// * `app` - Application instance to run
    ///
    /// # Returns
    ///
    /// `Ok(())` when the application exits normally.
    pub fn run<A: Application + View>(&mut self, app: &mut A) -> Result<()> {
        crate::debug::set_enabled(app.debug_logging());
        app.init();

        let declarations = collect_scene_declarations(app)?;
        let declarations = declarations
            .into_iter()
            .find(|declaration| declaration.opens_at_launch)
            .into_iter()
            .collect();
        let mut slots = self.create_slots(app, declarations)?;

        if slots.is_empty() && app.exit_when_all_windows_closed() {
            return Ok(());
        }

        self.run_loop(app, &mut slots)
    }

    fn create_slots<A: Application + View>(
        &mut self,
        app: &mut A,
        declarations: Vec<WindowDeclaration>,
    ) -> Result<Vec<WindowSlot<A>>> {
        let scale_milli = self.backend.output_scale_milli();
        let mut slots = Vec::new();

        for (index, declaration) in declarations.into_iter().enumerate() {
            slots.push(self.create_slot(app, declaration, scale_milli, index == 0)?);
        }

        Ok(slots)
    }

    fn create_slot<A: Application + View>(
        &mut self,
        app: &mut A,
        declaration: WindowDeclaration,
        scale_milli: u32,
        is_primary: bool,
    ) -> Result<WindowSlot<A>> {
        let window_id = WindowId::generate();
        let pipeline_id = PipelineId::new(window_id.get());
        let mut pipeline = RenderingPipeline::with_pipeline_id(pipeline_id);
        pipeline.set_scale_milli(scale_milli);

        let root = Box::new(SceneWindowRootElement::new(
            app.clone(),
            declaration.key.clone(),
            pipeline_id,
        ));
        pipeline.set_root(root);

        let window_info = pipeline.layout_initial();
        let limits = pipeline
            .element_tree()
            .root()
            .and_then(find_window_size_limits)
            .unwrap_or_default();
        let menu_json = window_info
            .menu_bar
            .as_ref()
            .map(|menu_bar| menu_bar.to_json())
            .unwrap_or_default();

        let request = WindowCreateRequest {
            app_id: window_info.app_id.clone(),
            title: window_info.title.clone(),
            size: window_info.size,
            window_type: window_info.window_type,
            menu_titles: menu_json.clone(),
            focus_on_create: window_info.focus_on_create,
            active_on_focus: window_info.active_on_focus,
            opaque: window_info.opaque,
        };

        let mut window = self.backend.create_window(request)?;
        sync_output_scale(&mut pipeline, window.as_ref());

        if let Some(menu_bar) = window_info.menu_bar {
            if !menu_json.is_empty() {
                let _ = window.set_menu_titles(&menu_json);
                menu_model::register_menu_callbacks(window.surface_id(), &menu_bar);
            }
        }

        if !limits.resizable {
            let _ = window.set_resizable(false);
        }

        let context = WindowContext {
            window_id,
            scene_key: declaration.key,
            pipeline_id,
            platform_window_id: window.platform_window_id(),
            is_primary,
        };

        app.on_window_created(&context, window.as_mut());
        sync_text_input(window.as_mut(), &pipeline);

        Ok(WindowSlot {
            context,
            pipeline,
            window,
            presented_this_cycle: false,
            _app: PhantomData,
        })
    }

    fn run_loop<A: Application + View>(
        &mut self,
        app: &mut A,
        slots: &mut Vec<WindowSlot<A>>,
    ) -> Result<()> {
        loop {
            let mut any_event = false;
            let mut any_presented = false;
            let mut close_ids = Vec::new();

            for slot in slots.iter_mut() {
                slot.presented_this_cycle = false;
                while let Some(event) = slot.window.poll_event() {
                    any_event = true;
                    handle_window_event(app, slot, event, &mut close_ids)?;
                }
            }

            remove_closed_slots(slots, &close_ids);
            if slots.is_empty() && app.exit_when_all_windows_closed() {
                return Ok(());
            }

            app.on_idle();
            self.handle_application_commands(app, slots)?;
            if slots.is_empty() && app.exit_when_all_windows_closed() {
                return Ok(());
            }

            for slot in slots.iter_mut() {
                app.on_window_sync(&slot.context, slot.window.as_mut());
                sync_text_input(slot.window.as_mut(), &slot.pipeline);
                if slot.pipeline.has_dirty()
                    && !slot.presented_this_cycle
                    && present_pipeline(&mut slot.pipeline, slot.window.as_mut())
                {
                    slot.presented_this_cycle = true;
                    any_presented = true;
                }
            }

            if !any_event && !any_presented {
                wait_for_next_event(slots, Duration::from_millis(16));
            }
        }
    }

    fn handle_application_commands<A: Application + View>(
        &mut self,
        app: &mut A,
        slots: &mut Vec<WindowSlot<A>>,
    ) -> Result<()> {
        for command in command::take_application_commands() {
            match command {
                ApplicationCommand::OpenWindow(key) => {
                    if slots.iter().any(|slot| slot.context.scene_key == key) {
                        continue;
                    }
                    if let Some(declaration) = collect_scene_declarations(app)?
                        .into_iter()
                        .find(|declaration| declaration.key == key)
                    {
                        let scale_milli = self.backend.output_scale_milli();
                        slots.push(self.create_slot(
                            app,
                            declaration,
                            scale_milli,
                            slots.is_empty(),
                        )?);
                    }
                }
                ApplicationCommand::DismissWindow(key) => {
                    let close_ids = slots
                        .iter()
                        .filter(|slot| slot.context.scene_key == key)
                        .map(|slot| slot.context.window_id)
                        .collect::<Vec<_>>();
                    for slot in slots.iter_mut() {
                        if close_ids.contains(&slot.context.window_id) {
                            let _ = slot.window.close();
                        }
                    }
                    remove_closed_slots(slots, &close_ids);
                }
            }
        }

        Ok(())
    }
}

struct WindowSlot<A: Application> {
    context: WindowContext,
    pipeline: RenderingPipeline,
    window: Box<dyn PlatformWindow>,
    presented_this_cycle: bool,
    _app: PhantomData<A>,
}

fn collect_scene_declarations<A: Application>(app: &A) -> Result<Vec<WindowDeclaration>> {
    let mut builder = SceneBuilder::new();
    app.scenes().build(&mut builder);
    let declarations = builder.into_declarations();
    let mut keys = BTreeSet::new();
    for declaration in &declarations {
        if !keys.insert(declaration.key.clone()) {
            return Err(Error::DuplicateSceneWindowKey);
        }
    }
    Ok(declarations)
}

fn resolve_scene_view<A: Application>(
    app: &A,
    target_key: &SceneWindowKey,
) -> Option<Box<dyn View>> {
    collect_scene_declarations(app)
        .ok()?
        .into_iter()
        .find(|declaration| &declaration.key == target_key)
        .map(|declaration| declaration.view)
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

fn sync_text_input(window: &mut dyn PlatformWindow, pipeline: &RenderingPipeline) {
    let state = pipeline.focused_text_input_state();
    window.sync_text_input(state.as_ref());
}

fn present_pipeline(pipeline: &mut RenderingPipeline, window: &mut dyn PlatformWindow) -> bool {
    if let Some((buffer, damage)) = pipeline.render_with_damage() {
        window.present_with_damage(buffer, damage);
        true
    } else {
        false
    }
}

fn wait_for_next_event<A: Application>(slots: &mut [WindowSlot<A>], timeout: Duration) {
    if let Some(slot) = slots.first_mut() {
        slot.window.wait_for_event(timeout);
    } else {
        std::thread::sleep(timeout);
    }
}

fn sync_output_scale(pipeline: &mut RenderingPipeline, window: &dyn PlatformWindow) {
    let scale_milli = window.output_scale_milli();
    if pipeline.scale_milli() != scale_milli {
        pipeline.set_scale_milli(scale_milli);
    }
}

fn handle_window_event<A: Application>(
    app: &mut A,
    slot: &mut WindowSlot<A>,
    event: Event,
    close_ids: &mut Vec<WindowId>,
) -> Result<()> {
    match event {
        Event::Quit => {
            let _ = slot.window.close();
            close_ids.push(slot.context.window_id);
        }
        Event::Resize { width, height } => {
            let new_size = Size::new(width as f32, height as f32);
            if slot.window.resize(width, height).is_ok() {
                sync_output_scale(&mut slot.pipeline, slot.window.as_ref());
                slot.pipeline.resize(new_size);
                app.on_window_resize(&slot.context, width, height);
                sync_text_input(slot.window.as_mut(), &slot.pipeline);
                if present_pipeline(&mut slot.pipeline, slot.window.as_mut()) {
                    slot.presented_this_cycle = true;
                }
            }
        }
        Event::ScreenSizeChanged { width, height } => {
            let resize_to = app.on_screen_size_changed(width, height);
            if let Some(new_size) = resize_to {
                let new_width = new_size.width.max(1.0) as u32;
                let new_height = new_size.height.max(1.0) as u32;
                if slot.window.resize(new_width, new_height).is_ok() {
                    sync_output_scale(&mut slot.pipeline, slot.window.as_ref());
                    slot.pipeline.resize(new_size);
                    sync_text_input(slot.window.as_mut(), &slot.pipeline);
                }
            }
            if resize_to.is_some() || slot.pipeline.has_dirty() {
                if present_pipeline(&mut slot.pipeline, slot.window.as_mut()) {
                    slot.presented_this_cycle = true;
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
            if !slot.pipeline.handle_event(&event)
                && let Event::TextInputCommit {
                    context_id,
                    serial,
                    text,
                } = &event
            {
                app.on_text_input_commit(&slot.context, *context_id, *serial, text);
            }
            sync_after_event(slot);
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
            if !slot.pipeline.handle_event(&event)
                && let Event::TextInputPreedit {
                    context_id,
                    serial,
                    cursor_byte,
                    anchor_byte,
                    text,
                    spans,
                } = &event
            {
                app.on_text_input_preedit(
                    &slot.context,
                    *context_id,
                    *serial,
                    *cursor_byte,
                    *anchor_byte,
                    text,
                    spans,
                );
            }
            sync_after_event(slot);
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
            if !slot.pipeline.handle_event(&event)
                && let Event::TextInputDeleteSurroundingText {
                    context_id,
                    serial,
                    before_bytes,
                    after_bytes,
                } = event
            {
                app.on_text_input_delete_surrounding_text(
                    &slot.context,
                    context_id,
                    serial,
                    before_bytes,
                    after_bytes,
                );
            }
            sync_after_event(slot);
        }
        Event::TextInputDone { context_id, serial } => {
            let event = Event::TextInputDone { context_id, serial };
            let _ = slot.pipeline.handle_event(&event);
        }
        Event::Custom { event_type, data } if event_type == 0xF0C0F => {
            if let Some((window_id, app_name, menu_titles)) = decode_app_change_payload(&data) {
                app.on_focus_changed(window_id, &app_name, &menu_titles);
            }
        }
        Event::Custom { event_type, data } if event_type == 0xF0C0A => {
            if let Some((window_id, app_name, menu_titles)) = decode_app_change_payload(&data) {
                app.on_active_app_changed(window_id, &app_name, &menu_titles);
                sync_after_event(slot);
            }
        }
        _ => {
            let _ = slot.pipeline.handle_event(&event);
            handle_emitted_window_events(slot, close_ids)?;
            sync_after_event(slot);
        }
    }
    Ok(())
}

fn sync_after_event<A: Application>(slot: &mut WindowSlot<A>) {
    sync_text_input(slot.window.as_mut(), &slot.pipeline);
    if slot.pipeline.has_dirty() && present_pipeline(&mut slot.pipeline, slot.window.as_mut()) {
        slot.presented_this_cycle = true;
    }
}

fn handle_emitted_window_events<A: Application>(
    slot: &mut WindowSlot<A>,
    close_ids: &mut Vec<WindowId>,
) -> Result<()> {
    for emitted_event in slot.pipeline.take_emitted_events() {
        match emitted_event {
            Event::Window(crate::event::WindowEvent::CloseRequested) => {
                let _ = slot.window.close();
                close_ids.push(slot.context.window_id);
            }
            Event::Window(crate::event::WindowEvent::MaximizeRequested) => {
                let _ = slot.window.maximize();
            }
            Event::Window(crate::event::WindowEvent::RestoreRequested) => {
                let _ = slot.window.restore();
            }
            Event::Window(crate::event::WindowEvent::MinimizeRequested) => {
                let _ = slot.window.minimize();
            }
            Event::Window(crate::event::WindowEvent::MoveRequested) => {
                let _ = slot.window.request_move();
            }
            _ => {}
        }
    }
    Ok(())
}

fn remove_closed_slots<A: Application>(slots: &mut Vec<WindowSlot<A>>, close_ids: &[WindowId]) {
    if close_ids.is_empty() {
        return;
    }
    let mut retained = Vec::new();
    for mut slot in core::mem::take(slots) {
        if close_ids.contains(&slot.context.window_id) {
            menu_model::unregister_menu_callbacks(slot.window.surface_id());
            slot.pipeline.teardown();
        } else {
            retained.push(slot);
        }
    }
    *slots = retained;
}

fn decode_app_change_payload(data: &[u8]) -> Option<(u32, String, String)> {
    if data.len() < 4 {
        return None;
    }
    let window_id = u32::from_le_bytes(data[0..4].try_into().ok()?);
    let mut offset = 4;

    let read_str = |data: &[u8], offset: &mut usize| -> Option<String> {
        if *offset + 4 > data.len() {
            return None;
        }
        let len = u32::from_le_bytes(data[*offset..*offset + 4].try_into().ok()?) as usize;
        *offset += 4;
        if *offset + len > data.len() {
            return None;
        }
        let s = core::str::from_utf8(&data[*offset..*offset + len]).ok()?;
        *offset += len;
        Some(String::from(s))
    };

    let _app_id = read_str(data, &mut offset)?;
    let app_name = read_str(data, &mut offset)?;
    let _title = read_str(data, &mut offset)?;
    let menu_titles = read_str(data, &mut offset)?;
    Some((window_id, app_name, menu_titles))
}

struct SceneWindowRootElement<A: Application + View> {
    id: ElementId,
    app: A,
    scene_key: SceneWindowKey,
    pipeline_id: PipelineId,
    child: Option<Box<dyn Element>>,
    size: Size,
    position: Point,
    subscriptions: Vec<SubscriptionId>,
}

impl<A: Application + View> SceneWindowRootElement<A> {
    fn new(app: A, scene_key: SceneWindowKey, pipeline_id: PipelineId) -> Self {
        let id = ElementId::generate();
        let child = resolve_scene_view(&app, &scene_key).map(|view| view.create_element());
        Self {
            id,
            app,
            scene_key,
            pipeline_id,
            child,
            size: Size::ZERO,
            position: Point::ZERO,
            subscriptions: Vec::new(),
        }
    }
}

impl<A: Application + View> Element for SceneWindowRootElement<A> {
    fn id(&self) -> ElementId {
        self.id
    }

    fn type_name(&self) -> &str {
        "SceneWindowRootElement"
    }

    fn type_name_debug(&self) -> String {
        alloc::format!("SceneWindowRootElement<{}>", core::any::type_name::<A>())
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

    fn update(&mut self, _new_view: &dyn View) -> UpdateResult {
        UpdateResult::NoChange
    }

    fn rebuild(&mut self) -> UpdateResult {
        let focused_path = self
            .child
            .as_ref()
            .and_then(|child| crate::element::focused_descendant_path(child.as_ref()));
        let Some(new_view) = resolve_scene_view(&self.app, &self.scene_key) else {
            return UpdateResult::NoChange;
        };
        if let Some(ref mut child) = self.child {
            match child.update(new_view.as_ref()) {
                UpdateResult::NoChange => return UpdateResult::NoChange,
                UpdateResult::Updated => {
                    crate::pipeline::mark_element_needs_layout(self.pipeline_id, child.id());
                    return UpdateResult::NoChange;
                }
                UpdateResult::Replaced => child.unmount(),
            }
        }
        self.child = Some(new_view.create_element());
        if let Some(ref mut child) = self.child {
            let ctx = MountContext::new(self.pipeline_id);
            child.mount(&ctx);
            if let Some(path) = focused_path.as_deref() {
                crate::element::restore_focus_at_path(child.as_mut(), path);
            }
            crate::pipeline::mark_element_needs_layout(self.pipeline_id, child.id());
        }
        UpdateResult::NoChange
    }

    fn mount(&mut self, ctx: &MountContext) {
        self.pipeline_id = ctx.pipeline_id();
        for listenable in View::listenables(&self.app) {
            let element_id = self.id;
            let pipeline_id = self.pipeline_id;
            let invalidation_kind = listenable.invalidation_kind();
            let callback = alloc::sync::Arc::new(move || match invalidation_kind {
                InvalidationKind::Build => {
                    crate::pipeline::mark_element_dirty(pipeline_id, element_id)
                }
                InvalidationKind::Paint => {
                    crate::pipeline::mark_element_needs_paint(pipeline_id, element_id)
                }
            });
            self.subscriptions.push(listenable.subscribe_any(callback));
        }
        if let Some(ref mut child) = self.child {
            child.mount(ctx);
        }
    }

    fn unmount(&mut self) {
        if let Some(ref mut child) = self.child {
            child.unmount();
        }
        let listenables = View::listenables(&self.app);
        for (listenable, subscription_id) in listenables.iter().zip(self.subscriptions.iter()) {
            listenable.unsubscribe(*subscription_id);
        }
        self.subscriptions.clear();
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        if let Some(ref mut child) = self.child {
            self.size = child.layout(constraints);
        } else {
            self.size = Size::ZERO;
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
        self.child
            .as_ref()
            .is_some_and(|child| child.hit_test(point))
            || self.bounds().contains(point)
    }

    fn handle_event(&mut self, event: &Event, phase: crate::event::Phase) -> bool {
        self.child
            .as_mut()
            .is_some_and(|child| child.handle_event(event, phase))
    }

    fn take_window_action(&mut self) -> Option<crate::event::WindowEvent> {
        self.child
            .as_mut()
            .and_then(|child| child.take_window_action())
    }
}
