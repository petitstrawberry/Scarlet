//! System input panel. Text editing and conversion remain in SWS/TextInput/IME.
#[cfg(any(target_os = "scarlet", test))]
#[path = "soft_keyboard/input.rs"]
mod keyboard_input;

#[cfg(any(target_os = "scarlet", test))]
#[path = "soft_keyboard/layout.rs"]
mod keyboard_layout;

#[cfg(target_os = "scarlet")]
#[path = "soft_keyboard/view.rs"]
mod keyboard_view;

#[cfg(target_os = "scarlet")]
mod panel {
    use super::keyboard_input::{Contact, HeldKeys, Modifiers, Stroke};
    use super::keyboard_layout::{Action, Layer, panel_height};
    use scarlet_ui::element::UpdateResult;
    use scarlet_ui::error::{Error as UiError, RenderFailureKind};
    use scarlet_ui::prelude::*;
    use scarlet_ui::{
        PlatformWindow, SWSPlatformWindow, event::Event, pipeline::RenderingPipeline,
        renderer::PresentedFrame,
    };
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        time::{Duration, Instant},
    };
    use sws_protocol::{
        input_panel::{self, Context},
        text_input_content_purpose as purpose,
    };

    #[derive(Clone, Copy)]
    struct PendingAction {
        context: Context,
        action: Action,
        time_ns: u64,
        contact: Option<Contact>,
    }
    fn keyboard_for_context(
        size: Size,
        context: Context,
        modifiers: u32,
        layer: Layer,
        pending: Rc<RefCell<Vec<PendingAction>>>,
        dispatch_contact: Rc<Cell<Option<Contact>>>,
        clock: Instant,
    ) -> Box<dyn View> {
        let numeric = matches!(
            context.content_purpose,
            purpose::DIGITS | purpose::NUMBER | purpose::PHONE | purpose::PIN
        );
        crate::keyboard_view::keyboard(
            size,
            modifiers,
            layer,
            numeric,
            Rc::new(move |action| {
                let time_ns = clock.elapsed().as_nanos().min(u64::MAX as u128) as u64;
                pending.borrow_mut().push(PendingAction {
                    context,
                    action,
                    time_ns: time_ns.max(1),
                    contact: dispatch_contact.get(),
                });
            }),
            |_| crate::keyboard_view::KeyTarget::Desktop,
        )
    }
    pub enum RunError {
        DuplicateProvider,
        Failed(String),
    }
    impl From<String> for RunError {
        fn from(message: String) -> Self {
            Self::Failed(message)
        }
    }
    impl std::fmt::Display for RunError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::DuplicateProvider => f.write_str("another input panel is already registered"),
                Self::Failed(message) => f.write_str(message),
            }
        }
    }
    pub fn run() -> std::result::Result<(), RunError> {
        let probe =
            sws_client::Connection::connect_default().map_err(|error| format!("{error:?}"))?;
        let (width, height) = probe
            .get_screen_size()
            .map_err(|error| format!("{error:?}"))?;
        let scale = probe
            .get_output_scale()
            .map_err(|error| format!("{error:?}"))?
            .max(1000) as f32
            / 1000.0;
        let logical_width = width as f32 / scale;
        let logical_screen_height = height as f32 / scale;
        let size = Size::new(
            logical_width,
            panel_height(logical_width, logical_screen_height),
        );
        let mut window = SWSPlatformWindow::create_with_type(
            "org.scarlet-os.input-panel",
            "Keyboard",
            size,
            sws_protocol::window_types::INPUT_PANEL,
        )
        .map_err(|error| format!("{error:?}"))?;
        let receiver = window
            .connection()
            .subscribe_window_events(window.surface_id());
        if !window
            .connection()
            .register_input_panel(window.surface_id())
            .map_err(|error| format!("{error:?}"))?
        {
            return Err(RunError::DuplicateProvider);
        }
        let mut environment = window
            .connection()
            .get_input_environment()
            .map_err(|error| format!("{error:?}"))?;
        let mut context = Context::default();
        let mut visible = false;
        let mut retry_render = false;
        let mut retried_rejection = false;
        let mut dismissed = None;
        let mut modifiers = Modifiers::default();
        let mut layer = Layer::Letters;
        let pending_actions = Rc::new(RefCell::new(Vec::<PendingAction>::new()));
        let action_clock = Instant::now();
        let dispatch_contact = Rc::new(Cell::new(None));
        let mut held_keys = HeldKeys::default();
        let mut pipeline = RenderingPipeline::new();
        let root = keyboard_for_context(
            window.size(),
            context,
            modifiers.flags,
            layer,
            Rc::clone(&pending_actions),
            Rc::clone(&dispatch_contact),
            action_clock,
        );
        pipeline.set_root(root.create_element());
        pipeline.layout_initial();
        pipeline.resize(window.size());
        pipeline.set_scale_milli(window.output_scale_milli());
        if let Some(paint_backend) = window
            .take_paint_backend()
            .map_err(|error| format!("{error:?}"))?
        {
            pipeline.set_paint_backend(paint_backend);
        }
        loop {
            window
                .connection()
                .dispatch()
                .map_err(|error| format!("{error:?}"))?;
            let mut rebuild = false;
            let mut replace_tree = false;
            for event in receiver.drain_events() {
                match event {
                    sws_client::Event::InputPanelContext(next) => {
                        if next != context {
                            if (next.context_id, next.window_id, next.generation)
                                != (context.context_id, context.window_id, context.generation)
                            {
                                replace_tree = true;
                                visible = false;
                                modifiers = Modifiers::default();
                                layer = layer.for_activation();
                                dismissed = None;
                                pending_actions.borrow_mut().clear();
                                held_keys.clear();
                            }
                            if next.content_purpose != context.content_purpose {
                                replace_tree = true;
                                held_keys.clear();
                            }
                            context = next;
                            rebuild = true;
                        }
                    }
                    sws_client::Event::InputEnvironmentChanged(next) => environment = next,
                    _ => {}
                }
            }
            // Posture and keyboard hotplug updates also hide an already-open panel.
            let should_show = super::keyboard_input::should_show(
                context,
                environment.should_auto_show_input_panel(),
                dismissed,
            );
            if should_show != visible {
                replace_tree = true;
                if context.active() {
                    window
                        .connection()
                        .set_input_panel_visible(context, should_show)
                        .map_err(|error| format!("{error:?}"))?;
                }
                visible = should_show;
                if !visible {
                    held_keys.clear();
                }
                rebuild = true;
            }
            while let Some(event) = window.poll_event() {
                if let Event::Resize { width, height } = &event {
                    replace_tree = true;
                    held_keys.clear();
                    // A configure changes layout geometry first; acknowledge it
                    // by replacing the backing surface before rendering at the
                    // new size, just as the standard Application runner does.
                    window
                        .resize(*width, *height)
                        .map_err(|error| format!("{error:?}"))?;
                    let scale_milli = window.output_scale_milli();
                    if pipeline.scale_milli() != scale_milli {
                        pipeline.set_scale_milli(scale_milli);
                    }
                    pipeline.resize(Size::new(*width as f32, *height as f32));
                    rebuild = true;
                }
                if matches!(event, Event::Quit) {
                    return Err("input-panel surface closed".to_string().into());
                }
                if matches!(event, Event::WindowSuspendedChanged { suspended: false }) {
                    pipeline.request_redraw();
                    retry_render = true;
                }
                if matches!(
                    event,
                    Event::WindowSuspendedChanged { suspended: true }
                        | Event::Focus(scarlet_ui::event::FocusEvent::Lost)
                ) {
                    held_keys.clear();
                }
                if let Event::TouchFrame(frame) = &event {
                    use scarlet_ui::event::TouchPhase;
                    // Dispatch one contact at a time so callbacks capture the
                    // exact DOWN, including multiple keys in a single report.
                    for change in &frame.changes {
                        let contact = Contact {
                            seat: change.seat_id,
                            id: change.id,
                            serial: change.serial,
                        };
                        match change.phase {
                            TouchPhase::Down => held_keys.begin(contact, change.x, change.y),
                            TouchPhase::Move => {
                                held_keys.motion(contact.seat, contact.id, change.x, change.y)
                            }
                            TouchPhase::Up | TouchPhase::Cancel => {
                                held_keys.end(contact.seat, contact.id)
                            }
                        }
                        dispatch_contact.set((change.phase == TouchPhase::Down).then_some(contact));
                        let mut single = frame.clone();
                        single.changes = vec![*change];
                        pipeline.handle_event(&Event::TouchFrame(single));
                        dispatch_contact.set(None);
                    }
                } else {
                    pipeline.handle_event(&event);
                }
            }
            let pending: Vec<_> = pending_actions.borrow_mut().drain(..).collect();
            for pending_action in pending {
                let activation = pending_action.context;
                let action = pending_action.action;
                if !visible
                    || activation.context_id != context.context_id
                    || activation.window_id != context.window_id
                    || activation.generation != context.generation
                {
                    continue;
                }
                match action {
                    Action::Key(code, required_modifiers) => {
                        held_keys.arm(
                            pending_action.contact,
                            Stroke {
                                context: activation,
                                code,
                                modifiers: modifiers.flags | required_modifiers,
                            },
                            pending_action.time_ns,
                        );
                        window
                            .connection()
                            .input_panel_key(context, code, modifiers.flags | required_modifiers)
                            .map_err(|error| format!("{error:?}"))?;
                        // A single Shift is one-shot. A fast double tap locks it.
                        if modifiers.consume_stroke() {
                            rebuild = true;
                        }
                    }
                    Action::Modifier(flag) => {
                        modifiers.toggle(flag, pending_action.time_ns);
                        rebuild = true;
                    }
                    Action::SetLayer(next) => {
                        replace_tree = true;
                        held_keys.clear();
                        layer = next;
                        modifiers.change_layer();
                        rebuild = true;
                    }
                    Action::ToggleFunction => {
                        held_keys.clear();
                        layer = layer.toggle_function();
                        rebuild = true;
                    }
                    Action::Hide => {
                        replace_tree = true;
                        held_keys.clear();
                        window
                            .connection()
                            .dismiss_input_panel(context)
                            .map_err(|error| format!("{error:?}"))?;
                        dismissed = Some(context.generation);
                        visible = false;
                        modifiers = Modifiers::default();
                        layer = layer.for_activation();
                        pending_actions.borrow_mut().clear();
                        rebuild = true;
                    }
                }
            }
            // Process queued UP/CANCEL and context changes before repeat, so a
            // late frame cannot type one extra key after a finger has lifted.
            if visible {
                let now_ns = action_clock.elapsed().as_nanos().min(u64::MAX as u128) as u64;
                for stroke in held_keys.due(now_ns) {
                    if (
                        stroke.context.context_id,
                        stroke.context.window_id,
                        stroke.context.generation,
                    ) == (context.context_id, context.window_id, context.generation)
                    {
                        window
                            .connection()
                            .input_panel_key(context, stroke.code, stroke.modifiers)
                            .map_err(|error| format!("{error:?}"))?;
                    }
                }
            }
            if rebuild {
                let root = keyboard_for_context(
                    window.size(),
                    context,
                    modifiers.flags,
                    layer,
                    Rc::clone(&pending_actions),
                    Rc::clone(&dispatch_contact),
                    action_clock,
                );
                // Shift changes labels after DOWN. Reconcile the same keys so
                // their pressed state and touch routes survive that update.
                let retained = !replace_tree
                    && pipeline
                        .element_tree_mut()
                        .root_mut()
                        .is_some_and(|element| {
                            !matches!(element.update(root.as_ref()), UpdateResult::Replaced)
                        });
                if !retained {
                    pipeline.set_root(root.create_element());
                    pipeline.layout_initial();
                    pipeline.resize(window.size());
                } else if let Some(id) = pipeline.element_tree().root().map(|element| element.id())
                {
                    pipeline.pipeline_owner_mut().mark_needs_layout(id);
                    pipeline.pipeline_owner_mut().mark_needs_paint(id);
                    pipeline.request_redraw();
                }
            }
            if visible && (pipeline.has_dirty() || retry_render) {
                retry_render = false;
                match pipeline.render_for_present() {
                    Ok(PresentedFrame::Cpu { buffer, damage }) => {
                        retried_rejection = false;
                        window.present_with_damage(buffer, damage);
                    }
                    Ok(PresentedFrame::External | PresentedFrame::Idle) => {
                        retried_rejection = false;
                    }
                    Err(UiError::RenderFailure(failure)) => match failure.kind {
                        // Match ScarletUI's standard Application runner: transient
                        // GPU backpressure must not terminate the input provider.
                        RenderFailureKind::Busy => {
                            // Keep touch routes and pressed render objects.
                            // Replacing the tree loses DOWN feedback even while
                            // the independent key tracker continues typing.
                            pipeline.request_redraw();
                            retry_render = true;
                        }
                        RenderFailureKind::Rejected => {
                            pipeline.request_redraw();
                            // Repaint once from the retained tree. Repeated rejection
                            // waits for input instead of spinning on invalid work.
                            retry_render = !retried_rejection;
                            retried_rejection = true;
                            eprintln!("soft-keyboard: discarded frame: {failure}");
                        }
                        // RecoveryRequired and future failure kinds need a new backend.
                        _ => {
                            return Err(format!("render: {failure}").into());
                        }
                    },
                    Err(error) => return Err(format!("render: {error:?}").into()),
                }
            }
            window
                .connection()
                .wait_for_window_events(if visible && (retry_render || held_keys.active()) {
                    Duration::from_millis(16)
                } else {
                    Duration::from_secs(30)
                })
                .map_err(|error| format!("{error:?}"))?;
        }
    }
}
fn main() {
    #[cfg(target_os = "scarlet")]
    {
        // Keep the stemd-launched process alive across a lost SWS connection or
        // a renderer that needs recreation. Bound repeated startup failures.
        let mut failures = 0u32;
        loop {
            let started = std::time::Instant::now();
            let error = match panel::run() {
                Ok(()) => return,
                Err(error) => error,
            };
            if started.elapsed() >= std::time::Duration::from_secs(30) {
                failures = 0;
            }
            failures += 1;
            eprintln!("soft-keyboard: {error} (attempt {failures}/5)");
            if matches!(error, panel::RunError::DuplicateProvider) || failures >= 5 {
                std::process::exit(1);
            }
            let delay_ms = 250u64 << (failures - 1);
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }
    }
}
