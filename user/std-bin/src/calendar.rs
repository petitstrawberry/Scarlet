use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use scarlet_ui::prelude::*;
use scarlet_ui::views::Either;
use scarlet_ui::{KeyCode, hstack, vstack};
use scarlet_ui_macros::View;

#[path = "calendar/model.rs"]
mod model;
#[path = "desktop_utilities/time.rs"]
#[allow(dead_code)]
mod time;
use model::{Date, days_in_month};

fn today() -> Option<Date> {
    let seconds = i64::try_from(time::system_time_ns()? / 1_000_000_000).ok()?;
    Date::from_unix_seconds(seconds, time::local_utc_offset_seconds().unwrap_or(0))
}

#[derive(View, Clone)]
struct CalendarApp {
    selected: State<Date>,
    today: State<Option<Date>>,
    focused: State<bool>,
}

impl CalendarApp {
    fn move_month(&self, delta: i32) {
        self.selected
            .update(|date| *date = date.shift_months(delta));
    }

    fn go_today(&self) {
        let current = today();
        self.today.set(current);
        if let Some(date) = current {
            self.selected.set(date);
        }
    }

    fn handle_key(&self, event: KeyEvent) -> bool {
        let KeyEvent::Pressed { keycode, modifiers } = event else {
            return false;
        };
        if modifiers.control || modifiers.alt || modifiers.super_key {
            return false;
        }
        let selected = self.selected.get();
        let next = match keycode {
            KeyCode::Left => selected.shift_days(-1),
            KeyCode::Right => selected.shift_days(1),
            KeyCode::Up => selected.shift_days(-7),
            KeyCode::Down => selected.shift_days(7),
            KeyCode::PageUp => selected.shift_months(if modifiers.shift { -12 } else { -1 }),
            KeyCode::PageDown => selected.shift_months(if modifiers.shift { 12 } else { 1 }),
            KeyCode::Home => Date { day: 1, ..selected },
            KeyCode::End => Date {
                day: days_in_month(selected.year, selected.month),
                ..selected
            },
            KeyCode::Char('t' | 'T') => {
                self.go_today();
                return true;
            }
            _ => return false,
        };
        self.selected.set(next);
        true
    }

    fn cell(&self, date: Option<Date>) -> impl View + Clone + use<> {
        match date {
            Some(date) => {
                let palette = ColorPalette::default();
                let selected = self.selected.get() == date;
                let is_today = self.today.get() == Some(date);
                let state = self.selected.clone();
                Either::A(
                    Button::new(date.day.to_string())
                        .font_size(16.0)
                        .background_color(if selected {
                            palette.primary()
                        } else if is_today {
                            palette.surface_variant()
                        } else {
                            palette.background()
                        })
                        .text_color(if selected {
                            palette.text_inverse()
                        } else {
                            palette.text()
                        })
                        .border_color(if is_today {
                            palette.primary()
                        } else {
                            Color::CLEAR
                        })
                        .on_click(move || state.set(date))
                        .frame(f32::INFINITY, f32::INFINITY),
                )
            }
            None => Either::B(Spacer::new().frame(f32::INFINITY, f32::INFINITY)),
        }
    }

    fn week(&self, dates: &[Option<Date>]) -> impl View + Clone + use<> {
        hstack! {
            self.cell(dates[0]), self.cell(dates[1]), self.cell(dates[2]),
            self.cell(dates[3]), self.cell(dates[4]), self.cell(dates[5]), self.cell(dates[6]),
        }
        .spacing(4.0)
        .frame(f32::INFINITY, f32::INFINITY)
    }

    fn content(&self) -> impl View + Clone + use<> {
        let selected = self.selected.get();
        let cells = selected.month_cells();
        let previous = self.clone();
        let next = self.clone();
        let current = self.clone();
        let keys = self.clone();
        let palette = ColorPalette::default();
        let is_today = self.today.get() == Some(selected);
        vstack! {
            hstack! {
                Text::new(selected.month_title()).font_size(24.0),
                Spacer::new(),
                Button::new("Today").font_size(12.0).enabled(self.today.get().is_some())
                    .on_click(move || current.go_today()).frame(58.0, 34.0),
                Button::new("‹").font_size(22.0).on_click(move || previous.move_month(-1)).frame(32.0, 34.0),
                Button::new("›").font_size(22.0).on_click(move || next.move_month(1)).frame(32.0, 34.0),
            }.spacing(6.0).frame_width(f32::INFINITY),
            Divider::new(),
            hstack! {
                weekday("Mon"), weekday("Tue"), weekday("Wed"), weekday("Thu"),
                weekday("Fri"), weekday("Sat"), weekday("Sun"),
            }.spacing(4.0).frame_width(f32::INFINITY),
            vstack! {
                self.week(&cells[0..7]), self.week(&cells[7..14]),
                self.week(&cells[14..21]), self.week(&cells[21..28]),
                self.week(&cells[28..35]), self.week(&cells[35..42]),
            }.spacing(4.0).frame(f32::INFINITY, f32::INFINITY),
            vstack! {
                Text::new(if is_today { "TODAY" } else { "SELECTED DATE" }).font_size(10.0).color(palette.text_secondary()),
                Text::new(selected.label()).font_size(15.0),
            }.spacing(6.0).alignment(Alignment::Leading).padding(14.0).frame_width(f32::INFINITY).background(palette.surface_variant()),
            Text::new("Arrows  day / week    PgUp / PgDn  month    T  today").font_size(10.0).color(palette.text_secondary()),
            Text::new(if self.today.get().is_none() { "System date unavailable".into() } else { time::zone_label() }).font_size(10.0).color(palette.text_secondary()),
        }.spacing(12.0).padding(18.0)
            .frame(f32::INFINITY, f32::INFINITY)
            .focusable(self.focused.clone())
            .on_key(move |event| keys.handle_key(event))
    }
}

fn weekday(label: &str) -> impl View + Clone {
    Text::new(label)
        .font_size(12.0)
        .color(ColorPalette::default().text_secondary())
        .alignment(Alignment::Center)
        .frame(f32::INFINITY, 20.0)
}

impl Application for CalendarApp {
    fn scenes(&self) -> impl Scene {
        Window::new("Calendar", self.content())
            .app_id("org.scarlet-os.desktop.calendar")
            .background_color(ColorPalette::default().background())
            .size(Size::new(480.0, 550.0))
            .min_size(Size::new(440.0, 530.0))
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn main() -> ExitCode {
    let current = today();
    let mut app = CalendarApp {
        selected: State::new(StateId::new(0), current.unwrap_or_default()),
        today: State::new(StateId::new(1), current),
        focused: State::new(StateId::new(2), true),
    };
    let current_day = app.today.clone();
    thread::spawn(move || {
        loop {
            // Also pick up wall-clock/timezone changes without redrawing every second.
            let seconds = time::system_time_ns().map(|ns| (ns / 1_000_000_000) as i64);
            let delay = seconds.map_or(60, |seconds| {
                let local = seconds + time::local_utc_offset_seconds().unwrap_or(0);
                (86400 - local.rem_euclid(86400)).min(60) as u64
            });
            thread::sleep(Duration::from_secs(delay));
            let now = today();
            if current_day.get() != now {
                current_day.set(now);
            }
        }
    });
    match app.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[calendar] {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scarlet_ui::event::KeyModifiers;

    #[test]
    fn keyboard_navigation_crosses_leap_years_and_respects_shortcut_modifiers() {
        let app = CalendarApp::default();
        app.selected.set(Date::new(2024, 2, 29).unwrap());
        assert!(app.handle_key(KeyEvent::Pressed {
            keycode: KeyCode::PageDown,
            modifiers: KeyModifiers {
                shift: true,
                ..KeyModifiers::default()
            },
        }));
        assert_eq!(app.selected.get(), Date::new(2025, 2, 28).unwrap());
        assert!(app.handle_key(KeyEvent::Pressed {
            keycode: KeyCode::Right,
            modifiers: KeyModifiers::default(),
        }));
        assert_eq!(app.selected.get(), Date::new(2025, 3, 1).unwrap());
        assert!(!app.handle_key(KeyEvent::Pressed {
            keycode: KeyCode::PageDown,
            modifiers: KeyModifiers {
                control: true,
                ..KeyModifiers::default()
            },
        }));
        assert_eq!(app.selected.get(), Date::new(2025, 3, 1).unwrap());
    }
}
