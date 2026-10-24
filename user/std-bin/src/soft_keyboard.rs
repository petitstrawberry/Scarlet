//! System input panel. Text editing and conversion remain in SWS/TextInput/IME.
#[cfg(any(target_os = "scarlet", test))]
#[path = "soft_keyboard/input.rs"]
mod keyboard_input;

#[cfg(any(target_os = "scarlet", test))]
#[path = "soft_keyboard/layout.rs"]
mod keyboard_layout;

#[cfg(target_os = "scarlet")]
mod panel {
    use super::keyboard_input::{Contact, HeldKeys, Stroke};
    use super::keyboard_layout::{
        Action, EDGE_UNITS, GAP_UNITS, GRID_UNITS, Layer, RowGeometry, Tone, VerticalGeometry,
        panel_height, rows,
    };
    use scarlet_ui::element::{ComponentElement, Element};
    use scarlet_ui::error::{Error as UiError, RenderFailureKind};
    use scarlet_ui::prelude::*;
    use scarlet_ui::views::containers::ViewTuple;
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

    #[derive(Clone)]
    struct Views(Vec<Box<dyn View>>);
    impl ViewTuple for Views {
        fn create_elements(&self) -> Vec<Box<dyn Element>> {
            self.0.iter().map(|v| v.create_element()).collect()
        }
        fn clone_views(&self) -> Vec<Box<dyn View>> {
            self.0.clone()
        }
        fn collect_listenables<'a>(&'a self, out: &mut Vec<&'a dyn scarlet_ui::state::Listenable>) {
            for view in &self.0 {
                out.extend(view.listenables());
            }
        }
    }
    #[derive(Clone, Copy)]
    struct PendingAction {
        context: Context,
        action: Action,
        time_ns: u64,
        contact: Option<Contact>,
    }
    #[derive(Clone)]
    struct KeyboardKeyView {
        label: &'static str,
        action: Action,
        context: Context,
        background: Color,
        text_color: Color,
        width: f32,
        height: f32,
        font_size: f32,
        pending: Rc<RefCell<Vec<PendingAction>>>,
        dispatch_contact: Rc<Cell<Option<Contact>>>,
        clock: Instant,
    }
    impl KeyboardKeyView {
        fn body(&self) -> Box<dyn View> {
            let pending = Rc::clone(&self.pending);
            let dispatch_contact = Rc::clone(&self.dispatch_contact);
            let context = self.context;
            let action = self.action;
            let clock = self.clock;
            let button = if action == Action::Hide {
                Button::icon_only(Icon::KeyboardHide)
                    .icon_size(IconSize::Medium)
                    .icon_color(self.text_color)
            } else {
                Button::new(self.label)
            };
            let button = button
                .header_style()
                .font_size(self.font_size)
                .padding(2.0)
                .background_color(self.background)
                .text_color(self.text_color)
                .on_click(move || {
                    let time_ns = clock.elapsed().as_nanos().min(u64::MAX as u128) as u64;
                    pending.borrow_mut().push(PendingAction {
                        context,
                        action,
                        time_ns: time_ns.max(1),
                        contact: dispatch_contact.get(),
                    });
                })
                // All touch keys activate on DOWN. Utility actions must not
                // depend on an UP routed through a tree replaced after painting.
                // The pure keyboard tracker owns actual repeat timing; this
                // Button option supplies press activation and suppresses the
                // release tap without ever firing a UI-owned repeat.
                .repeat_while_pressed(Duration::MAX, Duration::MAX);
            Box::new(button.frame(self.width, self.height))
        }
    }
    fn keyboard_key_body(view: &KeyboardKeyView) -> Box<dyn View> {
        view.body()
    }
    impl View for KeyboardKeyView {
        fn create_element(&self) -> Box<dyn Element> {
            Box::new(ComponentElement::new_with_builder(
                self.clone(),
                keyboard_key_body,
            ))
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }
    fn keyboard_for_context(
        size: Size,
        context: Context,
        modifiers: u32,
        layer: Layer,
        pending: Rc<RefCell<Vec<PendingAction>>>,
        dispatch_contact: Rc<Cell<Option<Contact>>>,
        clock: Instant,
    ) -> Box<dyn Element> {
        let numeric = matches!(
            context.content_purpose,
            purpose::DIGITS | purpose::NUMBER | purpose::PHONE | purpose::PIN
        );
        keyboard_for_purpose(
            size,
            context,
            modifiers,
            layer,
            numeric,
            pending,
            dispatch_contact,
            clock,
        )
    }
    fn keyboard_for_purpose(
        size: Size,
        context: Context,
        modifiers: u32,
        layer: Layer,
        numeric: bool,
        pending: Rc<RefCell<Vec<PendingAction>>>,
        dispatch_contact: Rc<Cell<Option<Contact>>>,
        clock: Instant,
    ) -> Box<dyn Element> {
        let layout = rows(numeric, layer);
        // One shared unit keeps Q/A/Z widths consistent across staggered rows.
        let tablet_layout = layout.iter().any(|row| row.geometry == RowGeometry::Tablet);
        let tablet_unit = size.width / GRID_UNITS;
        let horizontal_padding = if tablet_layout {
            tablet_unit * EDGE_UNITS
        } else {
            10.0
        };
        let tablet_gap = tablet_unit * GAP_UNITS;
        let fill_gap = if tablet_layout { tablet_gap } else { 7.0 };
        let vertical_gap = if tablet_layout { tablet_gap } else { 7.0 };
        let vertical_padding = if tablet_layout {
            tablet_unit * EDGE_UNITS
        } else {
            10.0
        };
        let content_width = (size.width - horizontal_padding * 2.0).max(1.0);
        let vertical = VerticalGeometry::new(size.height, vertical_padding, vertical_gap, &layout);
        let mut views: Vec<Box<dyn View>> = Vec::new();
        for row in layout {
            let row_height = vertical.row_unit * row.height;
            let weight: f32 = row.keys.iter().map(|key| key.weight).sum();
            let gap = if row.geometry == RowGeometry::Tablet {
                tablet_gap
            } else {
                fill_gap
            };
            let unit = if row.geometry == RowGeometry::Tablet {
                tablet_unit
            } else {
                ((content_width - gap * (row.keys.len() - 1) as f32) / weight).max(1.0)
            };
            let inset = row.inset * unit;
            let keys = row
                .keys
                .into_iter()
                .map(|key| {
                    let label = if modifiers & input_panel::SHIFT != 0 {
                        key.shifted
                    } else {
                        key.plain
                    };
                    let active = matches!(key.action,Action::Modifier(flag) if modifiers & flag!=0);
                    let (background, text_color) = if active {
                        (Color::rgb(224, 225, 229), Color::rgb(28, 29, 33))
                    } else {
                        (
                            match key.tone {
                                Tone::Character => Color::rgba_f32(0.46, 0.46, 0.49, 0.96),
                                Tone::Utility => Color::rgba_f32(0.29, 0.29, 0.33, 0.96),
                                Tone::Toolbar => Color::rgba_f32(1.0, 1.0, 1.0, 0.045),
                            },
                            Color::WHITE,
                        )
                    };
                    let width = unit * key.weight;
                    let font_size = if row.height < 0.8 {
                        (row_height * 0.32).clamp(12.0, 16.0)
                    } else if label.chars().count() > 2 {
                        (row_height * 0.25).clamp(14.0, 19.0)
                    } else {
                        (row_height * 0.41).clamp(20.0, 30.0)
                    };
                    Box::new(KeyboardKeyView {
                        label,
                        action: key.action,
                        context,
                        background,
                        text_color,
                        width,
                        height: row_height,
                        font_size,
                        pending: Rc::clone(&pending),
                        dispatch_contact: Rc::clone(&dispatch_contact),
                        clock,
                    }) as Box<dyn View>
                })
                .collect();
            views.push(Box::new(
                HStack::new(Views(keys))
                    .spacing(gap)
                    .alignment(Alignment::TopLeading)
                    .padding_insets(EdgeInsets::new(inset, 0.0, 0.0, 0.0)),
            ));
        }
        VStack::new(Views(views))
            .spacing(vertical_gap)
            .alignment(Alignment::TopLeading)
            .padding_insets(EdgeInsets::new(
                horizontal_padding,
                vertical_padding,
                horizontal_padding,
                vertical.bottom_padding,
            ))
            .background(Color::rgba_f32(0.10, 0.10, 0.13, 0.96))
            .frame(size.width, size.height)
            .create_element()
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
        let mut rebuild_after_failure = false;
        let mut retried_rejection = false;
        let mut dismissed = None;
        let mut modifiers = 0;
        let mut shift_locked = false;
        let mut last_shift_tap_ns = 0;
        let mut layer = Layer::Letters;
        let pending_actions = Rc::new(RefCell::new(Vec::<PendingAction>::new()));
        let action_clock = Instant::now();
        let dispatch_contact = Rc::new(Cell::new(None));
        let mut held_keys = HeldKeys::default();
        let mut pipeline = RenderingPipeline::new();
        let root = keyboard_for_context(
            window.size(),
            context,
            modifiers,
            layer,
            Rc::clone(&pending_actions),
            Rc::clone(&dispatch_contact),
            action_clock,
        );
        pipeline.set_root(root);
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
            let mut rebuild = core::mem::take(&mut rebuild_after_failure);
            for event in receiver.drain_events() {
                match event {
                    sws_client::Event::InputPanelContext(next) => {
                        if next != context {
                            if (next.context_id, next.window_id, next.generation)
                                != (context.context_id, context.window_id, context.generation)
                            {
                                visible = false;
                                modifiers = 0;
                                shift_locked = false;
                                last_shift_tap_ns = 0;
                                layer = Layer::Letters;
                                dismissed = None;
                                pending_actions.borrow_mut().clear();
                                held_keys.clear();
                            }
                            if next.content_purpose != context.content_purpose {
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
                                modifiers: modifiers | required_modifiers,
                            },
                            pending_action.time_ns,
                        );
                        window
                            .connection()
                            .input_panel_key(context, code, modifiers | required_modifiers)
                            .map_err(|error| format!("{error:?}"))?;
                        // A single Shift is one-shot. A fast double tap locks it.
                        if modifiers & input_panel::SHIFT != 0 && !shift_locked {
                            modifiers &= !input_panel::SHIFT;
                            last_shift_tap_ns = 0;
                            rebuild = true;
                        }
                    }
                    Action::Modifier(flag) => {
                        if flag == input_panel::SHIFT {
                            const DOUBLE_TAP_NS: u64 = 350_000_000;
                            let double_tap = last_shift_tap_ns != 0
                                && pending_action.time_ns.saturating_sub(last_shift_tap_ns)
                                    <= DOUBLE_TAP_NS;
                            if shift_locked {
                                shift_locked = false;
                                modifiers &= !input_panel::SHIFT;
                                last_shift_tap_ns = 0;
                            } else if double_tap {
                                shift_locked = true;
                                modifiers |= input_panel::SHIFT;
                                last_shift_tap_ns = 0;
                            } else {
                                modifiers ^= input_panel::SHIFT;
                                last_shift_tap_ns = pending_action.time_ns;
                            }
                        } else {
                            modifiers ^= flag;
                        }
                        rebuild = true;
                    }
                    Action::SetLayer(next) => {
                        held_keys.clear();
                        layer = next;
                        modifiers = 0;
                        shift_locked = false;
                        last_shift_tap_ns = 0;
                        rebuild = true;
                    }
                    Action::Hide => {
                        held_keys.clear();
                        window
                            .connection()
                            .set_input_panel_visible(context, false)
                            .map_err(|error| format!("{error:?}"))?;
                        dismissed = Some(context.generation);
                        visible = false;
                        modifiers = 0;
                        shift_locked = false;
                        last_shift_tap_ns = 0;
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
                    modifiers,
                    layer,
                    Rc::clone(&pending_actions),
                    Rc::clone(&dispatch_contact),
                    action_clock,
                );
                pipeline.set_root(root);
                pipeline.layout_initial();
                pipeline.resize(window.size());
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
                            // Recreate the view tree as well as paint caches:
                            // a failed frame can cache pixels never presented.
                            rebuild_after_failure = true;
                            retry_render = true;
                        }
                        RenderFailureKind::Rejected => {
                            rebuild_after_failure = true;
                            // Repaint once from a fresh tree. Repeated rejection
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
