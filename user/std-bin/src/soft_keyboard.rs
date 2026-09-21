//! System input panel. Text editing and conversion remain in SWS/TextInput/IME.
#[cfg(target_os = "scarlet")]
mod panel {
    use scarlet_ui::element::{ComponentElement, Element};
    use scarlet_ui::prelude::*;
    use scarlet_ui::views::containers::ViewTuple;
    use scarlet_ui::{
        PlatformWindow, SWSPlatformWindow, event::Event, pipeline::RenderingPipeline,
        renderer::PresentedFrame,
    };
    use std::{
        cell::RefCell,
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
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Layer {
        Letters,
        Symbols,
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Action {
        Key(u16, u32),
        Modifier(u32),
        SetLayer(Layer),
        Hide,
    }
    #[derive(Clone, Copy)]
    enum Tone {
        Character,
        Utility,
        Toolbar,
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum RowGeometry {
        Ipad,
        Fill,
    }
    #[derive(Clone)]
    struct Key {
        plain: &'static str,
        shifted: &'static str,
        action: Action,
        weight: f32,
        tone: Tone,
    }
    #[derive(Clone)]
    struct KeyRow {
        keys: Vec<Key>,
        height: f32,
        geometry: RowGeometry,
    }
    #[derive(Clone, Copy)]
    struct PendingAction {
        context: Context,
        action: Action,
        time_ns: u64,
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
        clock: Instant,
    }
    impl KeyboardKeyView {
        fn body(&self) -> Box<dyn View> {
            let pending = Rc::clone(&self.pending);
            let context = self.context;
            let action = self.action;
            let clock = self.clock;
            let mut button = Button::new(self.label)
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
                    });
                });
            if matches!(self.action, Action::Key(..)) {
                button = button
                    .repeat_while_pressed(Duration::from_millis(420), Duration::from_millis(55));
            }
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
    fn key(label: &'static str, shifted: &'static str, code: u16) -> Key {
        Key {
            plain: label,
            shifted,
            action: Action::Key(code, 0),
            weight: 1.0,
            tone: Tone::Character,
        }
    }
    fn stroke(label: &'static str, code: u16, modifiers: u32) -> Key {
        Key {
            plain: label,
            shifted: label,
            action: Action::Key(code, modifiers),
            weight: 1.0,
            tone: Tone::Character,
        }
    }
    fn control(label: &'static str, action: Action, weight: f32) -> Key {
        Key {
            plain: label,
            shifted: label,
            action,
            weight,
            tone: Tone::Utility,
        }
    }
    fn toolbar(label: &'static str, action: Action) -> Key {
        Key {
            plain: label,
            shifted: label,
            action,
            weight: 1.0,
            tone: Tone::Toolbar,
        }
    }
    fn row(keys: Vec<Key>) -> KeyRow {
        KeyRow {
            keys,
            height: 1.0,
            geometry: RowGeometry::Ipad,
        }
    }
    fn fill_row(keys: Vec<Key>) -> KeyRow {
        KeyRow {
            keys,
            height: 1.0,
            geometry: RowGeometry::Fill,
        }
    }
    fn terminal_row() -> KeyRow {
        use Action::Key;
        KeyRow {
            keys: vec![
                toolbar("esc", Key(1, 0)),
                toolbar("tab", Key(15, 0)),
                toolbar("home", Key(102, 0)),
                toolbar("end", Key(107, 0)),
                toolbar("pg up", Key(104, 0)),
                toolbar("pg dn", Key(109, 0)),
                toolbar("←", Key(105, 0)),
                toolbar("↑", Key(103, 0)),
                toolbar("↓", Key(108, 0)),
                toolbar("→", Key(106, 0)),
            ],
            height: 0.73,
            geometry: RowGeometry::Fill,
        }
    }
    fn rows(numeric: bool, layer: Layer) -> Vec<KeyRow> {
        use Action::*;
        if numeric {
            return vec![
                terminal_row(),
                fill_row(vec![
                    key("7", "7", 8),
                    key("8", "8", 9),
                    key("9", "9", 10),
                    control("delete", Key(14, 0), 1.25),
                ]),
                fill_row(vec![
                    key("4", "4", 5),
                    key("5", "5", 6),
                    key("6", "6", 7),
                    key("-", "-", 12),
                ]),
                fill_row(vec![
                    key("1", "1", 2),
                    key("2", "2", 3),
                    key("3", "3", 4),
                    key(".", ".", 52),
                ]),
                fill_row(vec![
                    control("hide", Hide, 1.0),
                    control("0", Key(11, 0), 2.0),
                    control("return", Key(28, 0), 1.25),
                ]),
            ];
        }
        if layer == Layer::Symbols {
            return vec![
                terminal_row(),
                row(vec![
                    control("tab", Key(15, 0), 1.285),
                    key("1", "1", 2),
                    key("2", "2", 3),
                    key("3", "3", 4),
                    key("4", "4", 5),
                    key("5", "5", 6),
                    key("6", "6", 7),
                    key("7", "7", 8),
                    key("8", "8", 9),
                    key("9", "9", 10),
                    key("0", "0", 11),
                    control("delete", Key(14, 0), 1.285),
                ]),
                row(vec![
                    control("ctrl", Modifier(input_panel::CTRL), 1.70),
                    stroke("!", 2, input_panel::SHIFT),
                    stroke("@", 3, input_panel::SHIFT),
                    stroke("#", 4, input_panel::SHIFT),
                    stroke("$", 5, input_panel::SHIFT),
                    stroke("%", 6, input_panel::SHIFT),
                    stroke("^", 7, input_panel::SHIFT),
                    stroke("&", 8, input_panel::SHIFT),
                    stroke("*", 9, input_panel::SHIFT),
                    stroke("(", 10, input_panel::SHIFT),
                    control("return", Key(28, 0), 2.04),
                ]),
                row(vec![
                    control("ABC", SetLayer(Layer::Letters), 2.22),
                    stroke(")", 11, input_panel::SHIFT),
                    key("-", "-", 12),
                    stroke("_", 12, input_panel::SHIFT),
                    key("=", "=", 13),
                    stroke("+", 13, input_panel::SHIFT),
                    key("[", "[", 26),
                    key("]", "]", 27),
                    key("/", "/", 53),
                    stroke("?", 53, input_panel::SHIFT),
                    control("ABC", SetLayer(Layer::Letters), 1.52),
                ]),
                row(vec![
                    control("あ/A", Key(85, 0), 1.03),
                    control("ABC", SetLayer(Layer::Letters), 1.04),
                    control("alt", Modifier(input_panel::ALT), 1.04),
                    control("space", Key(57, 0), 7.48),
                    control("ABC", SetLayer(Layer::Letters), 1.51),
                    control("hide", Hide, 1.49),
                ]),
            ];
        }
        vec![
            terminal_row(),
            row(vec![
                control("tab", Key(15, 0), 1.285),
                key("q", "Q", 16),
                key("w", "W", 17),
                key("e", "E", 18),
                key("r", "R", 19),
                key("t", "T", 20),
                key("y", "Y", 21),
                key("u", "U", 22),
                key("i", "I", 23),
                key("o", "O", 24),
                key("p", "P", 25),
                control("delete", Key(14, 0), 1.285),
            ]),
            row(vec![
                control("ctrl", Modifier(input_panel::CTRL), 1.70),
                key("a", "A", 30),
                key("s", "S", 31),
                key("d", "D", 32),
                key("f", "F", 33),
                key("g", "G", 34),
                key("h", "H", 35),
                key("j", "J", 36),
                key("k", "K", 37),
                key("l", "L", 38),
                control("return", Key(28, 0), 2.04),
            ]),
            row(vec![
                control("shift", Modifier(input_panel::SHIFT), 2.22),
                key("z", "Z", 44),
                key("x", "X", 45),
                key("c", "C", 46),
                key("v", "V", 47),
                key("b", "B", 48),
                key("n", "N", 49),
                key("m", "M", 50),
                key(",", "<", 51),
                key(".", ">", 52),
                control("shift", Modifier(input_panel::SHIFT), 1.52),
            ]),
            row(vec![
                control("あ/A", Key(85, 0), 1.03),
                control("123", SetLayer(Layer::Symbols), 1.04),
                control("alt", Modifier(input_panel::ALT), 1.04),
                control("space", Key(57, 0), 7.48),
                control("123", SetLayer(Layer::Symbols), 1.51),
                control("hide", Hide, 1.49),
            ]),
        ]
    }
    fn keyboard_for_context(
        size: Size,
        context: Context,
        modifiers: u32,
        layer: Layer,
        pending: Rc<RefCell<Vec<PendingAction>>>,
        clock: Instant,
    ) -> Box<dyn Element> {
        let numeric = matches!(
            context.content_purpose,
            purpose::DIGITS | purpose::NUMBER | purpose::PHONE | purpose::PIN
        );
        keyboard_for_purpose(size, context, modifiers, layer, numeric, pending, clock)
    }
    fn keyboard_for_purpose(
        size: Size,
        context: Context,
        modifiers: u32,
        layer: Layer,
        numeric: bool,
        pending: Rc<RefCell<Vec<PendingAction>>>,
        clock: Instant,
    ) -> Box<dyn Element> {
        let layout = rows(numeric, layer);
        // Prompt's landscape iPad keyboard uses one shared horizontal grid:
        // 0.17-unit outer insets and gaps, one-unit character keys, and wider
        // edge controls. Keeping the unit global prevents Q/A/Z from changing
        // width or drifting horizontally from one row to the next.
        const IPAD_GRID_UNITS: f32 = 14.78;
        const IPAD_EDGE_UNITS: f32 = 0.17;
        const IPAD_GAP_UNITS: f32 = 0.17;
        let ipad_layout = layout.iter().any(|row| row.geometry == RowGeometry::Ipad);
        let ipad_unit = size.width / IPAD_GRID_UNITS;
        let horizontal_padding = if ipad_layout {
            ipad_unit * IPAD_EDGE_UNITS
        } else {
            10.0
        };
        let ipad_gap = ipad_unit * IPAD_GAP_UNITS;
        let fill_gap = if ipad_layout { ipad_gap } else { 7.0 };
        let vertical_gap = if ipad_layout { ipad_gap } else { 7.0 };
        let vertical_padding = if ipad_layout {
            ipad_unit * IPAD_EDGE_UNITS
        } else {
            10.0
        };
        let content_width = (size.width - horizontal_padding * 2.0).max(1.0);
        let available_height =
            (size.height - vertical_padding * 2.0 - vertical_gap * (layout.len() - 1) as f32)
                .max(1.0);
        let total_height: f32 = layout.iter().map(|row| row.height).sum();
        let mut views: Vec<Box<dyn View>> = Vec::new();
        for row in layout {
            let row_height = available_height * row.height / total_height;
            let weight: f32 = row.keys.iter().map(|key| key.weight).sum();
            let gap = if row.geometry == RowGeometry::Ipad {
                ipad_gap
            } else {
                fill_gap
            };
            let unit = if row.geometry == RowGeometry::Ipad {
                ipad_unit
            } else {
                ((content_width - gap * (row.keys.len() - 1) as f32) / weight).max(1.0)
            };
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
                        clock,
                    }) as Box<dyn View>
                })
                .collect();
            views.push(Box::new(
                HStack::new(Views(keys))
                    .spacing(gap)
                    .alignment(Alignment::TopLeading),
            ));
        }
        VStack::new(Views(views))
            .spacing(vertical_gap)
            .alignment(Alignment::TopLeading)
            .padding_insets(EdgeInsets::new(
                horizontal_padding,
                vertical_padding,
                horizontal_padding,
                vertical_padding,
            ))
            .background(Color::rgba_f32(0.10, 0.10, 0.13, 0.96))
            .frame(size.width, size.height)
            .create_element()
    }
    pub fn run() -> std::result::Result<(), String> {
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
            (logical_width * 0.35).min(logical_screen_height * 0.62),
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
            return Err("another input panel is already registered".into());
        }
        let mut environment = window
            .connection()
            .get_input_environment()
            .map_err(|error| format!("{error:?}"))?;
        let mut context = Context::default();
        let mut visible = false;
        let mut dismissed = None;
        let mut modifiers = 0;
        let mut shift_locked = false;
        let mut last_shift_tap_ns = 0;
        let mut layer = Layer::Letters;
        let pending_actions = Rc::new(RefCell::new(Vec::<PendingAction>::new()));
        let action_clock = Instant::now();
        let mut animation_tick = Instant::now();
        let mut pipeline = RenderingPipeline::new();
        let root = keyboard_for_context(
            window.size(),
            context,
            modifiers,
            layer,
            Rc::clone(&pending_actions),
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
            let now = Instant::now();
            let elapsed = now.saturating_duration_since(animation_tick);
            animation_tick = now;
            if pipeline.has_active_animation() {
                pipeline.advance_animations(elapsed);
            }
            window
                .connection()
                .dispatch()
                .map_err(|error| format!("{error:?}"))?;
            let mut rebuild = false;
            for event in receiver.drain_events() {
                match event {
                    sws_client::Event::InputPanelContext(next) => {
                        if next != context {
                            if next.generation != context.generation {
                                visible = false;
                                modifiers = 0;
                                shift_locked = false;
                                last_shift_tap_ns = 0;
                                layer = Layer::Letters;
                                dismissed = None;
                                pending_actions.borrow_mut().clear();
                            }
                            context = next;
                            rebuild = true;
                        }
                    }
                    sws_client::Event::InputEnvironmentChanged(next) => environment = next,
                    _ => {}
                }
            }
            // On touch devices, activating an editor opens the panel. A physical
            // desktop without touch keeps the provider dormant.
            let should_show = context.active()
                && environment.has_direct_touch()
                && dismissed != Some(context.generation);
            if should_show != visible {
                if context.active() {
                    window
                        .connection()
                        .set_input_panel_visible(context, should_show)
                        .map_err(|error| format!("{error:?}"))?;
                }
                visible = should_show;
                rebuild = true;
            }
            while let Some(event) = window.poll_event() {
                if let Event::Resize { width, height } = &event {
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
                    return Ok(());
                }
                pipeline.handle_event(&event);
            }
            let pending: Vec<_> = pending_actions.borrow_mut().drain(..).collect();
            for pending_action in pending {
                let activation = pending_action.context;
                let action = pending_action.action;
                if !visible
                    || activation.context_id != context.context_id
                    || activation.generation != context.generation
                {
                    continue;
                }
                match action {
                    Action::Key(code, required_modifiers) => {
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
                        layer = next;
                        modifiers = 0;
                        shift_locked = false;
                        last_shift_tap_ns = 0;
                        rebuild = true;
                    }
                    Action::Hide => {
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
            if rebuild {
                let root = keyboard_for_context(
                    window.size(),
                    context,
                    modifiers,
                    layer,
                    Rc::clone(&pending_actions),
                    action_clock,
                );
                pipeline.set_root(root);
                pipeline.layout_initial();
                pipeline.resize(window.size());
            }
            if visible && pipeline.has_dirty() {
                match pipeline
                    .render_for_present()
                    .map_err(|error| format!("{error:?}"))?
                {
                    PresentedFrame::Cpu { buffer, damage } => {
                        window.present_with_damage(buffer, damage);
                    }
                    PresentedFrame::External | PresentedFrame::Idle => {}
                }
            }
            window
                .connection()
                .wait_for_window_events(if pipeline.has_active_animation() {
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
    if let Err(error) = panel::run() {
        eprintln!("soft-keyboard: {error}");
    }
}
