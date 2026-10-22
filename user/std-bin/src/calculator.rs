//! Desktop calculator with an editable expression, reusable history and units.

mod calculator_model;

use calculator_model::{CATEGORIES, convert, evaluate, format_value, units};
use scarlet_ui::prelude::*;
use scarlet_ui::views::Either;
use scarlet_ui::{Event, KeyCode, KeyEvent, ListView, Select, hstack, vstack};
use scarlet_ui_macros::View;

#[derive(Clone, Debug)]
struct Calculation {
    expression: String,
    value: f64,
}

#[derive(View, Clone)]
struct CalculatorApp {
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
}

impl CalculatorApp {
    fn new() -> Self {
        let app = Self::default();
        app.amount.set("1".into());
        app.to.set(1);
        app
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
                self.selection.set(TextSelection {
                    anchor: TextPosition::new(0),
                    caret: TextPosition::new(expression.len()),
                });
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
            Event::Keyboard(KeyEvent::Char { c })
                if self.evaluated.get() && "+-*/^%×÷−".contains(*c) =>
            {
                self.insert(&c.to_string());
                true
            }
            _ => false,
        }
    }

    fn fallback_key(&self, event: &Event) -> bool {
        if self.handle_key(event) {
            return true;
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

    fn key_button(&self, label: &'static str) -> impl View + Clone {
        let app = self.clone();
        Button::new(label)
            .font_size(19.0)
            .on_click(move || match label {
                "AC" => app.clear(),
                "⌫" => app.backspace(),
                "=" => app.calculate(),
                "√" => app.insert("sqrt("),
                _ => app.insert(label),
            })
            .frame(f32::INFINITY, 38.0)
    }

    fn row(&self, keys: [&'static str; 4]) -> impl View + Clone {
        hstack! { self.key_button(keys[0]), self.key_button(keys[1]), self.key_button(keys[2]), self.key_button(keys[3]) }.spacing(6.0)
    }

    fn calculation_view(&self) -> impl View + Clone {
        let palette = ColorPalette::default();
        let edited = self.clone();
        let input_keys = self.clone();
        let reuse = self.clone();
        let history = self.clone();
        let clear_history = self.history.clone();
        let error = self.error.get();
        vstack! {
            Text::new("Expression").font_size(12.0).color(palette.text_secondary()),
            TextView::new(self.expression.clone(), self.selection.clone())
                .wrap_mode(WrapMode::None).font_size(23.0).padding(10.0)
                .placeholder("Type 2 + 3 × 4, then Enter").autofocus(true)
                .reveal_selection_on_change(true)
                .on_text_change(move |_| { edited.evaluated.set(false); edited.error.set(String::new()); })
                .frame(f32::INFINITY, 56.0),
            Text::new(self.answer.get().map(format_value).unwrap_or_else(|| "0".into()))
                .font_size(30.0).frame_width(f32::INFINITY),
            Text::new(if error.is_empty() { "Enter = calculate · Esc = clear · % divides by 100".into() } else { error.clone() })
                .font_size(12.0).color(if error.is_empty() { palette.text_secondary() } else { palette.error() })
                .frame_height(30.0),
            vstack! {
            self.row(["AC", "(", ")", "⌫"]),
            self.row(["7", "8", "9", "÷"]),
            self.row(["4", "5", "6", "×"]),
            self.row(["1", "2", "3", "−"]),
            self.row(["0", ".", "=", "+"]),
            self.row(["√", "^", "%", "π"]),
            }.spacing(6.0),
            hstack! {
                Button::new("Use result").on_click(move || { if let Some(value) = reuse.answer.get() { reuse.set_expression(value.to_string()); } }),
                Spacer::new(),
                Text::new("History · last 32").font_size(12.0),
                Button::new("Clear history").on_click(move || clear_history.set(Vec::new())),
            }.spacing(8.0),
            ListView::new(self.history.clone(), self.history_selection.clone(), 35.0, move |_, calculation, _| {
                let app = history.clone();
                let label = format!("{} = {}", abbreviated(&calculation.expression), format_value(calculation.value));
                Button::new(label).on_click(move || { app.set_expression(calculation.expression.clone()); app.answer.set(Some(calculation.value)); })
                    .frame(f32::INFINITY, 32.0)
            }).frame(f32::INFINITY, f32::INFINITY),
        }.spacing(6.0).alignment(Alignment::Leading)
            .on_event_capture(move |event| input_keys.handle_key(event))
    }

    fn conversion_view(&self) -> impl View + Clone {
        let palette = ColorPalette::default();
        let category = self.category.get();
        let unit_names: Vec<String> = units(category)
            .iter()
            .map(|unit| unit.name.into())
            .collect();
        let change = self.clone();
        let swap = self.clone();
        let converted = evaluate(&self.amount.get())
            .map_err(|e| e.to_string())
            .and_then(|value| {
                convert(value, category, self.from.get(), self.to.get()).map_err(String::from)
            });
        let output = match &converted {
            Ok(value) => format_value(*value),
            Err(error) => error.clone(),
        };
        vstack! {
            Text::new("Unit conversion").font_size(22.0),
            vstack! {
            Text::new("Quantity").font_size(13.0),
            Select::new(CATEGORIES.iter().map(|s| (*s).into()).collect(), self.category.clone())
                .on_change(move |_| { change.from.set(0); change.to.set(1); }).width(310.0),
            Text::new("Value (arithmetic expressions are accepted)").font_size(13.0),
            TextField::new(self.amount.clone()).font_size(22.0).frame(f32::INFINITY, 46.0),
            Text::new("From").font_size(13.0),
            Select::new(unit_names.clone(), self.from.clone()).width(310.0),
            }.spacing(14.0).alignment(Alignment::Leading),
            Text::new("To").font_size(13.0),
            Select::new(unit_names, self.to.clone()).width(310.0),
            Button::new("Swap units").on_click(move || { let from = swap.from.get(); swap.from.set(swap.to.get()); swap.to.set(from); }),
            Divider::new(),
            Text::new(output).font_size(if converted.is_ok() { 26.0 } else { 14.0 })
                .color(if converted.is_ok() { palette.text_primary() } else { palette.error() }),
            Text::new("Decimal kB/MB/GB and binary KiB/MiB/GiB are distinct.").font_size(12.0).color(palette.text_secondary()),
            Spacer::new(),
        }.spacing(14.0).alignment(Alignment::Leading)
    }
}

fn abbreviated(expression: &str) -> String {
    if expression.chars().count() <= 38 {
        expression.into()
    } else {
        format!("{}…", expression.chars().take(37).collect::<String>())
    }
}

impl Application for CalculatorApp {
    fn scenes(&self) -> impl Scene {
        let app = self.clone();
        let body = if self.mode.get() == 0 {
            Either::A(self.calculation_view())
        } else {
            Either::B(self.conversion_view())
        };
        WindowGroup::new("main", Window::new("Calculator", vstack! {
            Select::new(vec!["Calculator".into(), "Unit converter".into()], self.mode.clone()).width(240.0),
            body.frame(f32::INFINITY, f32::INFINITY),
        }.spacing(12.0).padding(16.0).frame(f32::INFINITY, f32::INFINITY)
            .on_event(move |event| app.fallback_key(event)))
            .app_id("org.scarlet-os.desktop.calculator")
            .size(Size::new(440.0, 740.0)).min_size(Size::new(400.0, 680.0)).resizable(true))
    }
    fn debug_logging(&self) -> bool {
        false
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
    fn unicode_keypad_backspace_is_safe() {
        let app = CalculatorApp::new();
        app.insert("π");
        app.backspace();
        assert!(app.expression.get().is_empty());
    }
}
