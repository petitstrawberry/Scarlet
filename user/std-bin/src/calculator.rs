//! Desktop calculator with an editable expression, reusable history and units.

mod calculator_model;

use calculator_model::{CATEGORIES, convert, evaluate, format_value, units};
use scarlet_ui::clipboard::{ClipboardHandle, TextClipboard};
use scarlet_ui::prelude::*;
use scarlet_ui::views::Either;
use scarlet_ui::{
    Event, KeyCode, KeyEvent, ListView, Select, TabBarPlacement, TabItem, TabView, hstack, vstack,
};
use scarlet_ui::{PlatformWindow, WindowContext};
use scarlet_ui_macros::View;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
struct Calculation {
    expression: String,
    value: f64,
}

#[derive(Default)]
struct ClipboardRuntime {
    service: Option<TextClipboard>,
    pending: Option<ClipboardHandle<()>>,
    unavailable: Option<String>,
}

#[derive(View, Clone)]
struct CalculatorApp {
    clipboard: State<Arc<Mutex<ClipboardRuntime>>>,
    notice: State<String>,
    expression: State<String>,
    selection: State<TextSelection>,
    answer: State<Option<f64>>,
    error: State<String>,
    evaluated: State<bool>,
    history: State<Vec<Calculation>>,
    history_selection: State<Option<usize>>,
    mode: State<usize>,
    category: State<usize>,
    from: State<usize>,
    to: State<usize>,
    amount: State<String>,
    amount_selection: State<TextSelection>,
    advanced: State<bool>,
    viewport: State<Size>,
}

impl CalculatorApp {
    fn new() -> Self {
        let app = Self::default();
        app.amount.set("1".into());
        app.viewport.set(Size::new(380.0, 584.0));
        app.to.set(1);
        app.amount_selection.set(TextSelection {
            anchor: TextPosition::new(0),
            caret: TextPosition::new(1),
        });
        app
    }

    fn copy_result(&self) {
        let Some(value) = self.current_value() else {
            return;
        };
        let shared = self.clipboard.get();
        let mut runtime = shared.lock().unwrap();
        if runtime.pending.is_some() {
            self.notice.set("Copy is already pending".into());
            return;
        }
        match runtime.service.as_ref() {
            Some(service) => {
                runtime.pending = Some(service.write_text(value.to_string()));
                self.notice.set("Copying result…".into());
            }
            None => self.notice.set(
                runtime
                    .unavailable
                    .clone()
                    .unwrap_or_else(|| "Desktop clipboard unavailable".into()),
            ),
        }
    }
    fn set_expression(&self, text: String) {
        self.selection.set(TextSelection::collapsed(text.len()));
        self.expression.set(text);
        self.evaluated.set(false);
        self.error.set(String::new());
    }

    fn clear(&self) {
        self.set_expression(String::new());
        self.answer.set(None);
    }

    fn insert(&self, text: &str) {
        let mut expression = self.expression.get();
        let mut selection = self.selection.get();
        if self.evaluated.get() {
            expression = if matches!(text, "+" | "−" | "-" | "×" | "*" | "÷" | "/" | "^" | "%")
            {
                self.answer
                    .get()
                    .map(|value| value.to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            selection = TextSelection::collapsed(expression.len());
        }
        let start = selection.start().byte.min(expression.len());
        let end = selection.end().byte.min(expression.len());
        if !expression.is_char_boundary(start) || !expression.is_char_boundary(end) {
            return;
        }
        if expression.len() - (end - start) + text.len() > calculator_model::MAX_EXPRESSION_BYTES {
            self.error
                .set("Expression is too long (maximum 512 bytes)".into());
            return;
        }
        expression.replace_range(start..end, text);
        self.expression.set(expression);
        self.selection
            .set(TextSelection::collapsed(start + text.len()));
        self.evaluated.set(false);
        self.error.set(String::new());
    }

    fn backspace(&self) {
        let mut expression = self.expression.get();
        let selection = self.selection.get();
        let end = selection.end().byte.min(expression.len());
        let mut start = selection.start().byte.min(end);
        if !expression.is_char_boundary(start) || !expression.is_char_boundary(end) {
            return;
        }
        if start == end {
            start = expression[..end]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
        if expression.is_char_boundary(start) && expression.is_char_boundary(end) {
            expression.replace_range(start..end, "");
            self.expression.set(expression);
            self.selection.set(TextSelection::collapsed(start));
            self.evaluated.set(false);
            self.error.set(String::new());
        }
    }

    fn calculate(&self) {
        let expression = self.expression.get();
        match evaluate(&expression) {
            Ok(value) => {
                self.answer.set(Some(value));
                self.error.set(String::new());
                self.evaluated.set(true);
                self.selection
                    .set(TextSelection::collapsed(expression.len()));
                self.history.update(|history| {
                    if history
                        .first()
                        .is_none_or(|old| old.expression != expression)
                    {
                        history.insert(0, Calculation { expression, value });
                        history.truncate(32);
                    }
                });
            }
            Err(error) => {
                self.error.set(error.to_string());
                self.evaluated.set(false);
            }
        }
    }

    fn handle_key(&self, event: &Event) -> bool {
        if self.mode.get() != 0 {
            return false;
        }
        match event {
            Event::Keyboard(KeyEvent::Pressed {
                keycode: KeyCode::Enter,
                ..
            }) => {
                self.calculate();
                true
            }
            Event::Keyboard(KeyEvent::Pressed {
                keycode: KeyCode::Escape,
                ..
            }) => {
                self.clear();
                true
            }
            Event::Keyboard(KeyEvent::Char { c: '=' }) => {
                self.calculate();
                true
            }
            Event::Keyboard(KeyEvent::Char { c }) if self.evaluated.get() && !c.is_control() => {
                self.insert(&c.to_string());
                true
            }
            _ => false,
        }
    }

    fn navigate_mode(&self, event: &Event) -> bool {
        if let Event::Keyboard(KeyEvent::Pressed {
            keycode: KeyCode::Tab,
            modifiers,
        }) = event
        {
            if modifiers.control && !modifiers.alt && !modifiers.super_key {
                self.mode
                    .set((self.mode.get() + if modifiers.shift { 2 } else { 1 }) % 3);
                return true;
            }
        }
        false
    }

    fn edit_expression(&self, event: &Event) -> bool {
        let editing = match event {
            Event::Mouse(scarlet_ui::MouseEvent::ButtonPressed { .. }) => true,
            Event::Keyboard(KeyEvent::Pressed { keycode, modifiers }) => {
                matches!(
                    keycode,
                    KeyCode::Left
                        | KeyCode::Right
                        | KeyCode::Home
                        | KeyCode::End
                        | KeyCode::Up
                        | KeyCode::Down
                        | KeyCode::PageUp
                        | KeyCode::PageDown
                ) || (modifiers.primary() && matches!(keycode, KeyCode::Char('a' | 'A')))
            }
            _ => false,
        };
        if editing {
            self.evaluated.set(false);
        }
        false
    }

    fn fallback_key(&self, event: &Event) -> bool {
        if self.handle_key(event) {
            return true;
        }
        if self.mode.get() == 1 {
            return match event {
                Event::Keyboard(KeyEvent::Char { c }) if !c.is_control() => {
                    self.insert_amount(&c.to_string());
                    true
                }
                Event::Keyboard(KeyEvent::Pressed {
                    keycode: KeyCode::Backspace,
                    ..
                }) => {
                    self.conversion_action("⌫");
                    true
                }
                _ => false,
            };
        }
        if self.mode.get() != 0 {
            return false;
        }
        match event {
            Event::Keyboard(KeyEvent::Char { c }) if !c.is_control() => {
                self.insert(&c.to_string());
                true
            }
            Event::Keyboard(KeyEvent::Pressed {
                keycode: KeyCode::Backspace,
                ..
            }) => {
                self.backspace();
                true
            }
            _ => false,
        }
    }

    fn toggle_sign(&self) {
        let text = self.expression.get();
        if text.is_empty() {
            self.insert("−");
            return;
        }
        let unsigned = text.strip_prefix("−(").and_then(|s| s.strip_suffix(')'));
        if unsigned.is_none() && text.len() + "−()".len() > calculator_model::MAX_EXPRESSION_BYTES
        {
            self.error
                .set("Expression is too long to change sign".into());
            return;
        }
        self.set_expression(
            unsigned
                .map(str::to_owned)
                .unwrap_or_else(|| format!("−({text})")),
        );
    }

    fn content_width(&self) -> f32 {
        (self.viewport.get().width - 32.0).max(280.0)
    }

    fn key_button(&self, label: &'static str) -> impl View + Clone + use<> {
        let app = self.clone();
        let palette = ColorPalette::default();
        let digit = label.chars().all(|c| c.is_ascii_digit()) || label == ".";
        let operator = matches!(label, "÷" | "×" | "−" | "+");
        let background = if label == "=" {
            palette.primary()
        } else if digit {
            palette.surface()
        } else if operator {
            palette.button_background()
        } else {
            palette.surface_variant()
        };
        let height =
            ((self.viewport.get().height - 288.0 - if self.advanced.get() { 32.0 } else { 0.0 })
                / 5.0)
                .clamp(40.0, 84.0);
        Button::new(label)
            .font_size(if digit { 26.0 } else { 22.0 })
            .background_color(background)
            .text_color(if label == "=" {
                palette.text_inverse()
            } else {
                palette.text_primary()
            })
            .on_click(move || match label {
                "AC" => app.clear(),
                "⌫" => app.backspace(),
                "=" => app.calculate(),
                "±" => app.toggle_sign(),
                "√" => app.insert("sqrt("),
                _ => app.insert(label),
            })
            .frame(f32::INFINITY, height)
    }

    fn row(&self, keys: [&'static str; 4]) -> impl View + Clone + use<> {
        hstack! { self.key_button(keys[0]), self.key_button(keys[1]), self.key_button(keys[2]), self.key_button(keys[3]) }.spacing(6.0)
    }

    fn current_value(&self) -> Option<f64> {
        if !self.error.get().is_empty() {
            return None;
        }
        if self.evaluated.get() {
            self.answer.get()
        } else {
            evaluate(&self.expression.get()).ok()
        }
    }

    fn result_text(&self) -> String {
        if !self.error.get().is_empty() {
            return "—".into();
        }
        self.current_value()
            .or(self.answer.get())
            .map(format_value)
            .unwrap_or_else(|| "0".into())
    }

    fn calculation_view(&self) -> impl View + Clone + use<> {
        let palette = ColorPalette::default();
        let edited = self.clone();
        let input_keys = self.clone();
        let copy = self.clone();
        let more = self.advanced.clone();
        let expression = self.expression.get();
        let editing = self.clone();
        let field_width = (scarlet_ui::graphics::measure_text_sized(
            if expression.is_empty() {
                "0"
            } else {
                &expression
            },
            16.0,
        )
        .0 as f32
            + 18.0)
            .clamp(44.0, self.content_width() - 16.0);
        let result = self.result_text();
        let font = fitted_font(&result, 54.0, self.content_width() - 16.0);
        let display_width = self.content_width() - 16.0;
        let status = if self.error.get().is_empty() {
            self.notice.get()
        } else {
            self.error.get()
        };
        let functions = if self.advanced.get() {
            Either::A(hstack! {
                self.function_key("("), self.function_key(")"), self.function_key("√"), self.function_key("^"), self.function_key("π"),
            }.spacing(6.0).frame_height(32.0))
        } else {
            Either::B(Spacer::new().frame_height(0.0))
        };
        vstack! {
            vstack! {
                hstack! {
                    Text::new("").frame_width((display_width - field_width).max(0.0)),
                    TextView::new(self.expression.clone(), self.selection.clone())
                        .wrap_mode(WrapMode::None).font_size(16.0).padding(6.0)
                        .background(Color::CLEAR).border_color(Color::CLEAR)
                        .focused_border_color(Color::CLEAR).text_color(palette.text_secondary())
                        .placeholder("0").autofocus(true).reveal_selection_on_change(true)
                        .on_text_change(move |_| { edited.evaluated.set(false); edited.error.set(String::new()); edited.notice.set(String::new()); })
                        .frame(field_width, 32.0)
                        .on_event_capture(move |event| editing.edit_expression(event)),
                }.frame(display_width, 32.0),
                right_aligned_text(&result, font, display_width, palette.text_primary()).frame_height(66.0),
                right_aligned_text(&status, fitted_font(&status, 12.0, display_width), display_width,
                    if self.error.get().is_empty() { palette.text_secondary() } else { palette.error() }).frame_height(22.0),
            }.spacing(2.0).padding(8.0),
            hstack! {
                Button::new(if self.advanced.get() { "Functions ▴" } else { "Functions ▾" }).header_style().font_size(12.0)
                    .on_click(move || more.set(!more.get())),
                Spacer::new(),
                Button::new("Copy").header_style().font_size(12.0).enabled(self.current_value().is_some())
                    .on_click(move || copy.copy_result()),
            }.frame_height(28.0),
            functions,
            vstack! {
                self.row(["AC", "±", "%", "÷"]),
                self.row(["7", "8", "9", "×"]),
                self.row(["4", "5", "6", "−"]),
                self.row(["1", "2", "3", "+"]),
                self.row(["⌫", "0", ".", "="]),
            }.spacing(6.0),
            Spacer::new(),
        }.spacing(6.0).padding(16.0).frame(f32::INFINITY, f32::INFINITY)
            .background(palette.background_secondary())
            .on_event_capture(move |event| input_keys.handle_key(event))
    }

    fn function_key(&self, label: &'static str) -> impl View + Clone + use<> {
        let app = self.clone();
        Button::new(label)
            .font_size(15.0)
            .on_click(move || app.insert(if label == "√" { "sqrt(" } else { label }))
            .frame(f32::INFINITY, 30.0)
    }

    fn insert_amount(&self, value: &str) {
        let mut text = self.amount.get();
        let selection = self.amount_selection.get();
        let start = selection.start().byte.min(text.len());
        let end = selection.end().byte.min(text.len());
        if !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
            || text.len() - (end - start) + value.len() > calculator_model::MAX_EXPRESSION_BYTES
        {
            return;
        }
        text.replace_range(start..end, value);
        self.amount.set(text);
        self.amount_selection
            .set(TextSelection::collapsed(start + value.len()));
    }

    fn conversion_action(&self, label: &str) {
        match label {
            "AC" => {
                self.amount.set("0".into());
                self.amount_selection.set(TextSelection {
                    anchor: TextPosition::new(0),
                    caret: TextPosition::new(1),
                });
            }
            "⌫" => {
                let text = self.amount.get();
                let selection = self.amount_selection.get();
                if selection.is_collapsed() {
                    let end = selection.caret.byte.min(text.len());
                    if !text.is_char_boundary(end) {
                        return;
                    }
                    let start = text[..end].char_indices().last().map_or(0, |(i, _)| i);
                    self.amount_selection.set(TextSelection {
                        anchor: TextPosition::new(start),
                        caret: TextPosition::new(end),
                    });
                }
                self.insert_amount("");
            }
            "±" => {
                let text = self.amount.get();
                let next = if let Some(value) =
                    text.strip_prefix("−(").and_then(|s| s.strip_suffix(')'))
                {
                    value.to_owned()
                } else if text.len() + "−()".len() <= calculator_model::MAX_EXPRESSION_BYTES {
                    format!("−({text})")
                } else {
                    return;
                };
                self.amount_selection
                    .set(TextSelection::collapsed(next.len()));
                self.amount.set(next);
            }
            "Swap" => {
                let from = self.from.get();
                self.from.set(self.to.get());
                self.to.set(from);
            }
            _ => self.insert_amount(label),
        }
    }

    fn conversion_key(&self, label: &'static str, columns: f32) -> impl View + Clone + use<> {
        let app = self.clone();
        let palette = ColorPalette::default();
        let numeric = label.chars().all(|c| c.is_ascii_digit()) || label == ".";
        let width = (self.content_width() - 18.0) / 4.0;
        let height = ((self.viewport.get().height - 344.0) / 4.0).clamp(44.0, 80.0);
        Button::new(label)
            .font_size(if numeric {
                24.0
            } else if label == "Swap" {
                13.0
            } else {
                20.0
            })
            .background_color(if numeric {
                palette.surface()
            } else {
                palette.button_background()
            })
            .on_click(move || app.conversion_action(label))
            .frame(width * columns + 6.0 * (columns - 1.0), height)
    }

    fn conversion_row(&self, labels: [&'static str; 4]) -> impl View + Clone + use<> {
        hstack! { self.conversion_key(labels[0], 1.0), self.conversion_key(labels[1], 1.0), self.conversion_key(labels[2], 1.0), self.conversion_key(labels[3], 1.0) }.spacing(6.0)
    }

    fn conversion_view(&self) -> impl View + Clone + use<> {
        let palette = ColorPalette::default();
        let category = self.category.get();
        let unit_names: Vec<String> = units(category).iter().map(|u| u.name.into()).collect();
        let change = self.clone();
        let converted = evaluate(&self.amount.get())
            .map_err(|e| e.to_string())
            .and_then(|value| {
                convert(value, category, self.from.get(), self.to.get()).map_err(String::from)
            });
        let output = converted
            .as_ref()
            .map(|v| format_value(*v))
            .unwrap_or_else(|_| "—".into());
        let note = if self.amount.get().trim().is_empty() {
            String::new()
        } else {
            converted.as_ref().err().cloned().unwrap_or_default()
        };
        let unit_width = (self.content_width() * 0.52).min(240.0);
        let value_width = (self.content_width() - unit_width - 36.0).max(80.0);
        vstack! {
            Select::new(CATEGORIES.iter().map(|s| (*s).into()).collect(), self.category.clone())
                .on_change(move |_| { change.from.set(0); change.to.set(1); }).width(self.content_width()),
            vstack! {
                hstack! { Text::new("FROM").font_size(11.0).color(palette.text_secondary()), Spacer::new() },
                hstack! {
                    TextView::new(self.amount.clone(), self.amount_selection.clone())
                        .wrap_mode(WrapMode::None).font_size(30.0).padding(2.0)
                        .background(Color::CLEAR).border_color(Color::CLEAR).focused_border_color(Color::CLEAR)
                        .placeholder("0").autofocus(true).reveal_selection_on_change(true)
                        .frame(value_width, 42.0),
                    Spacer::new(), Select::new(unit_names.clone(), self.from.clone()).width(unit_width),
                }.spacing(6.0),
                Divider::new(),
                hstack! { Text::new("TO").font_size(11.0).color(palette.text_secondary()), Spacer::new() },
                hstack! {
                    Text::new(output.clone()).font_size(fitted_font(&output, 32.0, value_width)).frame_width(value_width),
                    Spacer::new(), Select::new(unit_names, self.to.clone()).width(unit_width),
                }.spacing(6.0).frame_height(42.0),
            }.spacing(6.0).padding(12.0).frame_width(f32::INFINITY).background(palette.surface()).clip_radius(10.0),
            Text::new(note.clone()).font_size(fitted_font(&note, 12.0, self.content_width())).color(palette.error()).frame_height(16.0),
            vstack! {
                self.conversion_row(["7", "8", "9", "⌫"]),
                self.conversion_row(["4", "5", "6", "AC"]),
                self.conversion_row(["1", "2", "3", "±"]),
                hstack! { self.conversion_key("0", 2.0), self.conversion_key(".", 1.0), self.conversion_key("Swap", 1.0) }.spacing(6.0),
            }.spacing(6.0),
            Spacer::new(),
        }.spacing(12.0).padding(16.0).frame(f32::INFINITY, f32::INFINITY).background(palette.background_secondary())
    }

    fn history_view(&self) -> impl View + Clone + use<> {
        let palette = ColorPalette::default();
        let history = self.clone();
        let clear = self.history.clone();
        let row_width = self.content_width() - 24.0;
        let content = if self.history.get().is_empty() {
            Either::A(vstack! {
                Spacer::new(), Text::new("No calculations yet").font_size(20.0),
                Text::new("Your results will appear here.").font_size(13.0).color(palette.text_secondary()), Spacer::new(),
            }.spacing(10.0).frame(f32::INFINITY, f32::INFINITY))
        } else {
            Either::B(
                ListView::new(
                    self.history.clone(),
                    self.history_selection.clone(),
                    64.0,
                    move |_, calculation, _| {
                        let app = history.clone();
                        Button::new(history_label(&calculation, row_width))
                            .header_style()
                            .font_size(15.0)
                            .on_click(move || {
                                app.set_expression(calculation.expression.clone());
                                app.answer.set(Some(calculation.value));
                                app.evaluated.set(true);
                                app.mode.set(0);
                            })
                            .frame(f32::INFINITY, 58.0)
                    },
                )
                .frame(f32::INFINITY, f32::INFINITY),
            )
        };
        vstack! {
            hstack! {
                Text::new("Recent calculations").font_size(16.0), Spacer::new(),
                Button::new("Clear").header_style().font_size(12.0).enabled(!self.history.get().is_empty()).on_click(move || clear.set(Vec::new())),
            }.frame_height(32.0),
            content,
        }.spacing(12.0).padding(16.0).frame(f32::INFINITY, f32::INFINITY)
    }
}

fn fitted_font(text: &str, preferred: f32, width: f32) -> f32 {
    let measured = scarlet_ui::graphics::measure_text_sized(text, preferred).0 as f32;
    if measured <= width || measured <= 0.0 {
        preferred
    } else {
        (preferred * width / measured).max(9.0)
    }
}

fn right_aligned_text(
    text: &str,
    font: f32,
    width: f32,
    color: Color,
) -> impl View + Clone + use<> {
    let text_width =
        (scarlet_ui::graphics::measure_text_sized(text, font).0 as f32 + 1.0).min(width);
    // Finite leading space keeps full-width text from competing with a flex
    // spacer for the same available width in the native horizontal layout.
    hstack! {
        Text::new("").frame_width((width - text_width).max(0.0)),
        Text::new(text.to_owned()).font_size(font).color(color).frame_width(text_width),
    }
    .frame_width(width)
}

fn history_label(calculation: &Calculation, width: f32) -> String {
    let suffix = format!("  =  {}", format_value(calculation.value));
    let mut expression = calculation.expression.clone();
    let mut shortened = false;
    loop {
        let label = format!("{expression}{}{suffix}", if shortened { "…" } else { "" });
        if expression.is_empty()
            || scarlet_ui::graphics::measure_text_sized(&label, 15.0).0 as f32 <= width
        {
            return label;
        }
        expression.pop();
        shortened = true;
    }
}

impl Application for CalculatorApp {
    fn scenes(&self) -> impl Scene {
        let app = self.clone();
        let calc = self.clone();
        let convert = self.clone();
        let history = self.clone();
        let viewport = self.viewport.clone();
        let navigation = self.clone();
        let content = TabView::with_selected_index(
            vec![
                TabItem::new("Calculator", move || calc.calculation_view()),
                TabItem::new("Convert", move || convert.conversion_view()),
                TabItem::new("History", move || history.history_view()),
            ],
            self.mode.clone(),
        )
        .tab_bar_placement(TabBarPlacement::Top)
        .tab_bar_height(40.0)
        .font_size(13.0)
        .frame(f32::INFINITY, f32::INFINITY)
        .on_geometry_change(|geometry| geometry.size(), move |size| viewport.set(size))
        .on_event_capture(move |event| navigation.navigate_mode(event))
        .on_event(move |event| app.fallback_key(event));
        WindowGroup::new(
            "main",
            Window::new("Calculator", content)
                .app_id("org.scarlet-os.desktop.calculator")
                .size(Size::new(380.0, 584.0))
                .min_size(Size::new(340.0, 536.0))
                .resizable(true),
        )
    }

    fn debug_logging(&self) -> bool {
        false
    }
    fn on_window_created(&mut self, _: &WindowContext, window: &mut dyn PlatformWindow) {
        let shared = self.clipboard.get();
        let mut runtime = shared.lock().unwrap();
        match window.text_clipboard() {
            Ok(service) => runtime.service = Some(service),
            Err(error) => runtime.unavailable = Some(error.to_string()),
        }
    }
    fn on_idle(&mut self) {
        let result = {
            let shared = self.clipboard.get();
            let mut runtime = shared.lock().unwrap();
            let result = runtime.pending.as_ref().and_then(|job| job.try_take());
            if result.is_some() {
                runtime.pending = None;
            }
            result
        };
        if let Some(result) = result {
            self.notice.set(match result {
                Ok(()) => "Result copied to desktop clipboard".into(),
                Err(error) => error.to_string(),
            });
        }
    }
}

fn main() {
    if let Err(error) = CalculatorApp::new().run() {
        eprintln!("Calculator: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_shortens_expression_without_losing_the_result() {
        let calculation = Calculation {
            expression: "π+".repeat(100),
            value: 42.0,
        };
        let label = history_label(&calculation, 280.0);
        assert!(label.ends_with("…  =  42"));
        assert!(scarlet_ui::graphics::measure_text_sized(&label, 15.0).0 <= 280);
    }
    #[test]
    fn explicit_editing_after_evaluation_preserves_the_expression() {
        use scarlet_ui::event::KeyModifiers;
        let app = CalculatorApp::new();
        app.set_expression("12+3".into());
        app.calculate();
        assert!(!app.edit_expression(&Event::Keyboard(KeyEvent::Pressed {
            keycode: KeyCode::Home,
            modifiers: KeyModifiers::default(),
        })));
        app.selection.set(TextSelection {
            anchor: TextPosition::new(0),
            caret: TextPosition::new(2),
        });
        app.insert("7");
        assert_eq!(app.expression.get(), "7+3");
        assert_eq!(app.current_value(), Some(10.0));
        app.calculate();
        app.edit_expression(&Event::Mouse(scarlet_ui::MouseEvent::ButtonPressed {
            button: scarlet_ui::MouseButton::Left,
            x: 10,
            y: 10,
            click_count: 1,
        }));
        app.insert("0");
        assert_eq!(app.expression.get(), "7+30");
    }
    #[test]
    fn mode_shortcuts_wrap_in_both_directions_without_changing_values() {
        use scarlet_ui::event::KeyModifiers;
        let app = CalculatorApp::new();
        app.set_expression("2+3".into());
        let key = |shift| {
            Event::Keyboard(KeyEvent::Pressed {
                keycode: KeyCode::Tab,
                modifiers: KeyModifiers {
                    control: true,
                    shift,
                    ..KeyModifiers::default()
                },
            })
        };
        assert!(app.navigate_mode(&key(false)));
        assert_eq!(app.mode.get(), 1);
        assert!(app.navigate_mode(&key(true)));
        assert_eq!(app.mode.get(), 0);
        assert!(app.navigate_mode(&key(true)));
        assert_eq!(app.mode.get(), 2);
        assert_eq!(app.expression.get(), "2+3");
        assert_eq!(app.amount.get(), "1");
    }
    #[test]
    fn converter_keypad_replaces_selection_and_preserves_unit_choices() {
        let app = CalculatorApp::new();
        app.mode.set(1);
        app.conversion_action("7");
        app.conversion_action("5");
        assert_eq!(app.amount.get(), "75");
        app.amount_selection.set(TextSelection {
            anchor: TextPosition::new(1),
            caret: TextPosition::new(2),
        });
        app.conversion_action("2");
        assert_eq!(app.amount.get(), "72");
        app.conversion_action("±");
        assert_eq!(evaluate(&app.amount.get()), Ok(-72.0));
        app.conversion_action("±");
        assert_eq!(app.amount.get(), "72");
        app.conversion_action("Swap");
        assert_eq!((app.from.get(), app.to.get()), (1, 0));
        assert_eq!(app.amount.get(), "72");
        app.conversion_action("AC");
        assert!(app.fallback_key(&Event::Keyboard(KeyEvent::Char { c: '8' })));
        assert_eq!(app.amount.get(), "8");
    }

    #[test]
    fn converter_editing_rejects_invalid_utf8_boundaries_and_bounds_input() {
        let app = CalculatorApp::new();
        app.amount.set("π×2".into());
        app.amount_selection.set(TextSelection::collapsed(2));
        app.conversion_action("⌫");
        assert_eq!(app.amount.get(), "×2");
        app.amount_selection.set(TextSelection::collapsed(1));
        app.conversion_action("⌫");
        app.conversion_action("7");
        assert_eq!(app.amount.get(), "×2");
        let full = "1".repeat(calculator_model::MAX_EXPRESSION_BYTES);
        app.amount.set(full.clone());
        app.amount_selection
            .set(TextSelection::collapsed(full.len()));
        app.conversion_action("1");
        app.conversion_action("±");
        assert_eq!(app.amount.get(), full);
    }

    #[test]
    fn sign_wrapper_accounts_for_utf8_bytes_at_expression_limit() {
        let app = CalculatorApp::new();
        let text = "1".repeat(calculator_model::MAX_EXPRESSION_BYTES - 4);
        app.set_expression(text.clone());
        app.toggle_sign();
        assert_eq!(app.expression.get(), text);
        assert!(!app.error.get().is_empty());
        app.set_expression("1".repeat(calculator_model::MAX_EXPRESSION_BYTES - 5));
        app.toggle_sign();
        assert_eq!(
            app.expression.get().len(),
            calculator_model::MAX_EXPRESSION_BYTES
        );
        app.toggle_sign();
        assert_eq!(
            app.expression.get().len(),
            calculator_model::MAX_EXPRESSION_BYTES - 5
        );
    }
    #[test]
    fn enter_evaluates_even_when_keypad_button_has_focus() {
        use scarlet_ui::event::KeyModifiers;
        use scarlet_ui::pipeline::RenderingPipeline;
        let app = CalculatorApp::new();
        let capture = app.clone();
        let fallback = app.clone();
        let view = app
            .key_button("7")
            .on_event_capture(move |e| capture.handle_key(e))
            .on_event(move |e| fallback.fallback_key(e));
        let mut pipeline = RenderingPipeline::new();
        pipeline.set_root(
            Window::new("test", view)
                .decorated(false)
                .size(Size::new(200.0, 80.0))
                .create_element(),
        );
        pipeline.layout_initial();
        let key = |keycode| {
            Event::Keyboard(KeyEvent::Pressed {
                keycode,
                modifiers: KeyModifiers::empty(),
            })
        };
        pipeline.handle_event(&key(KeyCode::Tab));
        app.insert("7");
        pipeline.handle_event(&Event::Keyboard(KeyEvent::Char { c: '+' }));
        pipeline.handle_event(&Event::Keyboard(KeyEvent::Char { c: '3' }));
        pipeline.handle_event(&key(KeyCode::Enter));
        pipeline.handle_event(&Event::Keyboard(KeyEvent::Released {
            keycode: KeyCode::Enter,
            modifiers: KeyModifiers::empty(),
        }));
        assert_eq!(app.expression.get(), "7+3");
        assert_eq!(app.answer.get(), Some(10.0));
    }
    #[test]
    fn keypad_uses_selection_and_chains_answer() {
        let app = CalculatorApp::new();
        app.set_expression("12+3".into());
        app.selection.set(TextSelection {
            anchor: TextPosition::new(0),
            caret: TextPosition::new(2),
        });
        app.insert("7");
        assert_eq!(app.expression.get(), "7+3");
        app.calculate();
        assert_eq!(app.answer.get(), Some(10.0));
        app.insert("×");
        app.insert("2");
        app.calculate();
        assert_eq!(app.answer.get(), Some(20.0));
        app.insert("4");
        assert_eq!(app.expression.get(), "4");
    }
    #[test]
    fn errors_preserve_expression_and_history_is_bounded() {
        let app = CalculatorApp::new();
        app.set_expression("1/0".into());
        app.calculate();
        assert!(!app.error.get().is_empty());
        assert_eq!(app.expression.get(), "1/0");
        assert!(app.history.get().is_empty());
        for number in 0..40 {
            app.set_expression(number.to_string());
            app.calculate();
        }
        assert_eq!(app.history.get().len(), 32);
    }
    #[test]
    fn unfocused_keyboard_input_and_invalid_boundary_are_safe() {
        let app = CalculatorApp::new();
        assert!(app.fallback_key(&Event::Keyboard(KeyEvent::Char { c: '2' })));
        assert_eq!(app.expression.get(), "2");
        app.set_expression("π".into());
        app.selection.set(TextSelection::collapsed(1));
        app.backspace();
        assert_eq!(app.expression.get(), "π");
    }
    #[test]
    fn evaluated_typing_starts_new_number_but_operators_chain() {
        let app = CalculatorApp::new();
        app.set_expression("128*3.5".into());
        app.calculate();
        assert_eq!(app.result_text(), "448");
        assert!(app.handle_key(&Event::Keyboard(KeyEvent::Char { c: '7' })));
        assert_eq!(app.expression.get(), "7");
        assert_eq!(app.current_value(), Some(7.0));
        app.calculate();
        assert!(app.handle_key(&Event::Keyboard(KeyEvent::Char { c: '+' })));
        assert_eq!(app.expression.get(), "7+");
        assert_eq!(app.current_value(), None);
    }

    #[test]
    fn sign_change_is_reversible_and_preview_matches_copy_value() {
        let app = CalculatorApp::new();
        app.set_expression("2+3".into());
        app.toggle_sign();
        assert_eq!(app.current_value(), Some(-5.0));
        app.toggle_sign();
        assert_eq!(app.expression.get(), "2+3");
        app.calculate();
        app.set_expression("12".into());
        assert_eq!(app.result_text(), "12");
        assert_eq!(app.current_value(), Some(12.0));
        app.set_expression("1/0".into());
        app.calculate();
        assert_eq!(app.current_value(), None);
        assert_eq!(app.result_text(), "—");
    }

    #[test]
    fn unicode_keypad_backspace_is_safe() {
        let app = CalculatorApp::new();
        app.insert("π");
        app.backspace();
        assert!(app.expression.get().is_empty());
    }
}
