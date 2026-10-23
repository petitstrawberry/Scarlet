use std::process::ExitCode;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use scarlet_ui::graphics::Canvas;
use scarlet_ui::prelude::*;
use scarlet_ui::state::Listenable;
use scarlet_ui::views::{Either, Either3};
use scarlet_ui::{
    CanvasView, Event, KeyCode, MenuBarModel, MenuEntry, MenuItemModel, hstack, scenes, vstack,
};
use scarlet_ui_macros::View;

mod model;

#[cfg(target_os = "scarlet")]
use scarlet_os::time::{local_utc_offset_seconds, monotonic_time, system_time_ns};

#[cfg(not(target_os = "scarlet"))]
fn monotonic_time() -> std::time::Duration {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    ORIGIN.get_or_init(std::time::Instant::now).elapsed()
}

#[cfg(not(target_os = "scarlet"))]
fn system_time_ns() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_nanos()).ok())
}

#[cfg(not(target_os = "scarlet"))]
fn local_utc_offset_seconds() -> Option<i64> {
    Some(0)
}

fn zone_label() -> &'static str {
    #[cfg(target_os = "scarlet")]
    {
        if local_utc_offset_seconds().is_some() {
            "Local time"
        } else {
            "UTC"
        }
    }
    #[cfg(not(target_os = "scarlet"))]
    {
        "UTC (host preview)"
    }
}

use model::{Stopwatch, Timer, TimerPhase, ceil_seconds, format_seconds, parse_duration};

fn presets() -> [(Color, &'static str); 6] {
    [
        (Color::rgb(30, 30, 40), "Dark"),
        (Color::rgb(20, 30, 60), "Blue"),
        (Color::rgb(20, 40, 30), "Green"),
        (Color::rgb(60, 20, 40), "Red"),
        (Color::rgb(40, 20, 50), "Purple"),
        (Color::rgb(240, 240, 245), "Light"),
    ]
}

#[derive(View, Clone)]
struct ClockApp {
    tick: State<u64>,
    appearance: State<usize>,
    alpha: State<f32>,
    mode: State<usize>,
    timer: State<Timer>,
    stopwatch: State<Stopwatch>,
    duration_input: State<String>,
    error: State<String>,
    focused: State<bool>,
}

impl ClockApp {
    fn select_mode(&self, index: usize) {
        self.mode.set(index);
        self.focused.set(true);
    }

    fn menu_item(
        &self,
        id: &str,
        title: &str,
        shortcut: &str,
        enabled: bool,
        action: impl Fn(&Self) + Send + Sync + 'static,
    ) -> MenuEntry {
        // Like Notepad and Files, callbacks only capture thread-safe app state.
        // CanvasView's UI-thread Rc is constructed separately in clock_content.
        let app = self.clone();
        MenuEntry::Item(
            MenuItemModel::new(id, title)
                .shortcut(shortcut)
                .enabled(enabled)
                .on_activate(Arc::new(move || action(&app))),
        )
    }

    fn timer_finished(&self) -> bool {
        self.timer.get().phase(monotonic_time()) == TimerPhase::Finished
    }

    fn window_title(&self) -> &'static str {
        if self.timer_finished() {
            "Timer Finished — Clock"
        } else {
            "Clock"
        }
    }

    fn menu_bar(&self) -> MenuBarModel {
        let modes = [
            ("clock", "Clock", "Ctrl+1"),
            (
                "timer",
                if self.timer_finished() {
                    "Timer (Finished)"
                } else {
                    "Timer"
                },
                "Ctrl+2",
            ),
            ("stopwatch", "Stopwatch", "Ctrl+3"),
        ];
        MenuBarModel::new(vec![
            MenuItemModel::new("view", "View").children(
                modes
                    .into_iter()
                    .enumerate()
                    .map(|(index, (id, title, shortcut))| {
                        self.menu_item(id, title, shortcut, self.mode.get() != index, move |app| {
                            app.select_mode(index);
                        })
                    })
                    .collect(),
            ),
            MenuItemModel::new("display", "Display").children(vec![
                MenuEntry::Item(
                    MenuItemModel::new("background", "Background").children(
                        presets()
                            .into_iter()
                            .enumerate()
                            .map(|(index, (_, title))| {
                                self.menu_item(
                                    &format!("background-{index}"),
                                    title,
                                    "",
                                    self.appearance.get() != index,
                                    move |app| {
                                        app.appearance.set(index);
                                    },
                                )
                            })
                            .collect(),
                    ),
                ),
                MenuEntry::Item(
                    MenuItemModel::new("opacity", "Opacity").children(
                        [0, 25, 50, 75, 100]
                            .into_iter()
                            .map(|percent| {
                                let alpha = percent as f32 / 100.0;
                                self.menu_item(
                                    &format!("opacity-{percent}"),
                                    &format!("{percent}%"),
                                    "",
                                    self.alpha.get() != alpha,
                                    move |app| {
                                        app.alpha.set(alpha);
                                    },
                                )
                            })
                            .collect(),
                    ),
                ),
            ]),
        ])
    }

    fn bg_color(&self) -> Color {
        let c = presets()[self.appearance.get().min(5)].0;
        Color::rgba_f32(c.r, c.g, c.b, self.alpha.get())
    }

    fn timer_action(&self) {
        let now = monotonic_time();
        let phase = self.timer.get().phase(now);
        match phase {
            TimerPhase::Ready => match parse_duration(&self.duration_input.get()) {
                Ok(duration) => self.timer.update(|timer| {
                    *timer = Timer::new(duration);
                    timer.start(now);
                }),
                Err(message) => {
                    self.error.set(message.to_owned());
                    return;
                }
            },
            TimerPhase::Running => self.timer.update(|timer| timer.pause(now)),
            TimerPhase::Paused | TimerPhase::Finished => {
                self.timer.update(|timer| timer.start(now))
            }
        }
        self.error.set(String::new());
        if phase == TimerPhase::Ready {
            // Starting replaces the duration editor. Keep keyboard commands
            // owned by this window after that focused control disappears.
            self.focused.set(true);
        }
    }

    fn handle_key(&self, event: KeyEvent) -> bool {
        let KeyEvent::Pressed { keycode, modifiers } = event else {
            return false;
        };
        if !modifiers.control || modifiers.alt || modifiers.super_key {
            return false;
        }
        match keycode {
            KeyCode::Char('1'..='3') => {
                let KeyCode::Char(digit) = keycode else {
                    unreachable!()
                };
                self.select_mode((digit as u8 - b'1') as usize);
            }
            KeyCode::Space if self.mode.get() == 1 => self.timer_action(),
            KeyCode::Space if self.mode.get() == 2 => self.stopwatch.update(|watch| {
                let now = monotonic_time();
                if watch.running() {
                    watch.pause(now);
                } else {
                    watch.start(now);
                }
            }),
            KeyCode::Char('r' | 'R') if self.mode.get() == 1 => {
                self.timer.update(Timer::reset);
                self.error.set(String::new());
            }
            KeyCode::Char('r' | 'R') if self.mode.get() == 2 => {
                self.stopwatch.update(Stopwatch::reset)
            }
            _ => return false,
        }
        true
    }

    fn timer_content(&self) -> impl View + Clone + use<> {
        let now = monotonic_time();
        let timer = self.timer.get();
        let phase = timer.phase(now);
        let (status, action) = match phase {
            TimerPhase::Ready => ("Ready", "Start"),
            TimerPhase::Running => ("Running", "Pause"),
            TimerPhase::Paused => ("Paused", "Resume"),
            TimerPhase::Finished => ("Timer finished", "Restart"),
        };
        let palette = ColorPalette::default();
        let action_app = self.clone();
        let reset_app = self.clone();
        let submit_app = self.clone();
        let editor = if phase == TimerPhase::Ready {
            Either::A(
                vstack! {
                    Text::new("Duration · hh:mm:ss").font_size(13.0),
                    TextField::new(self.duration_input.clone())
                        .font_size(13.0)
                        .placeholder("00:05:00")
                        .on_submit(move || submit_app.timer_action())
                        .frame_width(200.0),
                }
                .spacing(6.0),
            )
        } else {
            Either::B(
                Text::new("Reset to change duration")
                    .font_size(13.0)
                    .color(palette.text_secondary())
                    .frame_height(58.0),
            )
        };
        let remaining = if phase == TimerPhase::Ready {
            parse_duration(&self.duration_input.get()).unwrap_or_else(|_| timer.remaining(now))
        } else {
            timer.remaining(now)
        };
        let error = if self.error.get().is_empty() {
            Either::A(Spacer::new().frame_height(0.0))
        } else {
            Either::B(
                Text::from_state(self.error.clone())
                    .font_size(13.0)
                    .color(palette.error()),
            )
        };
        vstack! {
            Spacer::new(),
            vstack! {
                Text::new(status).font_size(13.0).color(if phase == TimerPhase::Finished { palette.primary() } else { palette.text_secondary() }),
                Text::new(format_seconds(ceil_seconds(remaining))).font_size(44.0),
            }.spacing(8.0).padding(8.0).frame_width(f32::INFINITY),
            editor,
            error,
            hstack! {
                Button::new(action).font_size(13.0).background_color(palette.primary()).text_color(palette.text_inverse())
                    .on_click(move || action_app.timer_action()).frame(88.0, 30.0),
                Button::new("Reset").font_size(13.0).on_click(move || {
                    reset_app.timer.update(Timer::reset);
                    reset_app.error.set(String::new());
                }).frame(72.0, 30.0),
            }.spacing(8.0),
            Spacer::new(),
            Text::new("Keep Clock open to finish the timer.").font_size(13.0).color(palette.text_secondary()),
        }.spacing(6.0).padding(14.0).frame(f32::INFINITY, f32::INFINITY)
    }

    fn stopwatch_content(&self) -> impl View + Clone + use<> {
        let stopwatch = self.stopwatch.get();
        let elapsed = stopwatch.elapsed(monotonic_time());
        let running = stopwatch.running();
        let status = if running {
            "Running"
        } else if elapsed.is_zero() {
            "Ready"
        } else {
            "Paused"
        };
        let action = if running {
            "Pause"
        } else if elapsed.is_zero() {
            "Start"
        } else {
            "Resume"
        };
        let action_state = self.stopwatch.clone();
        let reset_state = self.stopwatch.clone();
        let palette = ColorPalette::default();
        vstack! {
            Spacer::new(),
            Text::new(status).font_size(13.0).color(palette.text_secondary()),
            Text::new(format!("{}.{:01}", format_seconds(elapsed.as_secs()), elapsed.subsec_millis() / 100)).font_size(38.0),
            Spacer::new().frame_height(20.0),
            hstack! {
                Button::new(action).font_size(13.0).background_color(palette.primary()).text_color(palette.text_inverse()).on_click(move || action_state.update(|watch| {
                    let now = monotonic_time();
                    if watch.running() { watch.pause(now); } else { watch.start(now); }
                })).frame(88.0, 30.0),
                Button::new("Reset").font_size(13.0).on_click(move || reset_state.update(Stopwatch::reset)).frame(72.0, 30.0),
            }.spacing(8.0),
            Spacer::new(),
        }.spacing(8.0).padding(18.0).frame(f32::INFINITY, f32::INFINITY)
    }

    fn clock_content(&self) -> impl View + Clone + use<> {
        let light = self.appearance.get() == 5;
        let content = if system_time_ns().is_some() {
            Either::A(
                CanvasView::new(
                    300.0,
                    300.0,
                    Rc::new(move |buf, w, h| {
                        draw_clock(&mut Canvas::new(buf, w, h), w, h, light);
                    }),
                )
                .frame(f32::INFINITY, f32::INFINITY),
            )
        } else {
            Either::B(
                Text::new("System time unavailable")
                    .font_size(13.0)
                    .color(if light { Color::BLACK } else { Color::WHITE })
                    .frame(f32::INFINITY, f32::INFINITY),
            )
        };
        vstack! {
            content,
            Text::new(zone_label()).font_size(13.0)
                .color(if light { Color::rgb(50, 50, 65) } else { Color::rgb(215, 215, 225) })
                .padding(8.0),
        }
        .frame(f32::INFINITY, f32::INFINITY)
    }
}

impl Application for ClockApp {
    fn scenes(&self) -> impl Scene {
        let _ = self.tick.get();
        let palette = ColorPalette::default();
        let mode = self.mode.get();
        let content = match mode {
            1 => Either3::B(self.timer_content()),
            2 => Either3::C(self.stopwatch_content()),
            _ => Either3::A(self.clock_content()),
        };
        let key_app = self.clone();
        scenes! {
            Window::new(self.window_title(), content.frame(f32::INFINITY, f32::INFINITY)
                .focusable(self.focused.clone())
                .on_event_capture(move |event| match event {
                    Event::Keyboard(key) => key_app.handle_key(*key),
                    _ => false,
                }))
                .background_color(if mode == 0 { self.bg_color() } else { palette.background() })
                .opaque(mode != 0)
                .app_id("org.scarlet-os.desktop.clock")
                .menu_bar(self.menu_bar())
                .size(Size::new(360.0, 380.0))
                .min_size(Size::new(340.0, 320.0)),
        }
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn draw_clock(canvas: &mut Canvas, w: u32, h: u32, light: bool) {
    // CanvasView retains its backing buffer across View updates. This clock
    // redraws a complete frame, so clear the previous hands and tick marks
    // before drawing the current time.
    canvas.fill_rect(0, 0, w, h, Color::TRANSPARENT);

    let utc_ns = system_time_ns().unwrap_or(0);
    let offset = local_utc_offset_seconds().unwrap_or(0);
    let secs = (utc_ns / 1_000_000_000) as i64 + offset;
    let sod = (((secs % 86400) + 86400) % 86400) as u64;
    let hh = (sod / 3600) % 12;
    let mm = (sod / 60) % 60;
    let ss = sod % 60;

    let cx = w as i32 / 2;
    let cy = h as i32 / 2;
    let radius = (w.min(h) as i32 / 2 - 20).max(40);

    for i in 0..60u32 {
        let a = (i as f32) * (core::f32::consts::TAU / 60.0) - core::f32::consts::FRAC_PI_2;
        let is_hour = i % 5 == 0;
        let (ri, ro, color) = if is_hour {
            (
                (radius - 16) as f32,
                radius as f32,
                if light {
                    Color::rgb(50, 50, 70)
                } else {
                    Color::rgb(180, 180, 200)
                },
            )
        } else {
            (
                (radius - 7) as f32,
                radius as f32,
                if light {
                    Color::rgb(90, 90, 110)
                } else {
                    Color::rgb(110, 110, 135)
                },
            )
        };
        canvas.draw_line(
            cx + (ri * a.cos()) as i32,
            cy + (ri * a.sin()) as i32,
            cx + (ro * a.cos()) as i32,
            cy + (ro * a.sin()) as i32,
            color,
        );
    }

    let hour_deg = (hh as f32 + mm as f32 / 60.0) * 30.0;
    let min_deg = (mm as f32 + ss as f32 / 60.0) * 6.0;
    let sec_deg = ss as f32 * 6.0;

    let mut draw_hand = |deg: f32, len: i32, color: Color| {
        let a = deg.to_radians() - core::f32::consts::FRAC_PI_2;
        canvas.draw_line(
            cx,
            cy,
            cx + (len as f32 * a.cos()) as i32,
            cy + (len as f32 * a.sin()) as i32,
            color,
        );
    };

    draw_hand(
        hour_deg,
        radius * 6 / 10,
        if light {
            Color::rgb(30, 30, 45)
        } else {
            Color::rgb(200, 200, 210)
        },
    );
    draw_hand(
        min_deg,
        radius * 8 / 10,
        if light {
            Color::rgb(40, 65, 90)
        } else {
            Color::rgb(160, 180, 200)
        },
    );
    draw_hand(sec_deg, radius * 9 / 10, Color::rgb(255, 149, 0));

    canvas.fill_rect(cx - 4, cy - 4, 8, 8, Color::rgb(255, 255, 255));
}

// Sleep until the next visible time change, or until an input changes the
// schedule. Paused utilities do not continuously rebuild the view tree.
fn start_updates(app: &ClockApp) {
    let (wake, changes) = mpsc::sync_channel(1);
    for state in [&app.mode as &dyn Listenable, &app.timer, &app.stopwatch] {
        let wake = wake.clone();
        state.subscribe_any(Arc::new(move || {
            let _ = wake.try_send(());
        }));
    }
    let tick = app.tick.clone();
    let mode = app.mode.clone();
    let timer = app.timer.clone();
    let stopwatch = app.stopwatch.clone();
    thread::spawn(move || {
        let mut previous = None;
        loop {
            let now = monotonic_time();
            let mode = mode.get();
            let timer = timer.get();
            let stopwatch = stopwatch.get();
            let phase = timer.phase(now);
            let remaining = timer.remaining(now);
            let elapsed = stopwatch.elapsed(now);
            let wall = system_time_ns();
            let signature = (
                mode,
                if mode == 0 {
                    wall.map(|ns| ns / 1_000_000_000)
                } else {
                    None
                },
                if mode == 1 {
                    ceil_seconds(remaining)
                } else {
                    0
                },
                if mode == 2 {
                    elapsed.as_millis() / 100
                } else {
                    0
                },
                phase == TimerPhase::Finished,
            );
            if previous != Some(signature) {
                tick.update(|value| *value = value.wrapping_add(1));
                previous = Some(signature);
            }
            let mut delay: Option<Duration> = None;
            let mut consider = |duration: Duration| {
                delay = Some(delay.map_or(duration, |current| current.min(duration)));
            };
            if mode == 0 {
                consider(Duration::from_nanos(
                    1_000_000_000 - wall.unwrap_or(0) % 1_000_000_000,
                ));
            }
            if phase == TimerPhase::Running {
                consider(if mode == 1 {
                    let nanos = remaining.subsec_nanos();
                    if nanos == 0 {
                        Duration::from_secs(1)
                    } else {
                        Duration::from_nanos(u64::from(nanos))
                    }
                } else {
                    remaining
                });
            }
            if mode == 2 && stopwatch.running() {
                consider(Duration::from_nanos(
                    100_000_000 - u64::from(elapsed.subsec_nanos()) % 100_000_000,
                ));
            }
            match delay {
                Some(delay) => {
                    let _ = changes.recv_timeout(delay.max(Duration::from_millis(1)));
                }
                None => {
                    if changes.recv().is_err() {
                        break;
                    }
                }
            }
        }
    });
}

fn main() -> ExitCode {
    let mut app = ClockApp {
        tick: State::new(StateId::new(0), 0),
        appearance: State::new(StateId::new(1), 0),
        alpha: State::new(StateId::new(2), 1.0),
        mode: State::new(StateId::new(3), 0),
        timer: State::new(StateId::new(4), Timer::default()),
        stopwatch: State::new(StateId::new(5), Stopwatch::default()),
        duration_input: State::new(StateId::new(6), "00:05:00".into()),
        error: State::new(StateId::new(7), String::new()),
        focused: State::new(StateId::new(8), true),
    };
    start_updates(&app);
    match app.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[clock] {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scarlet_ui::event::KeyModifiers;

    fn find_menu_item(items: &[MenuItemModel], id: &str) -> Option<MenuItemModel> {
        for item in items {
            if item.id() == id {
                return Some(item.clone());
            }
            for entry in item.children_value() {
                if let MenuEntry::Item(child) = entry {
                    if let Some(found) = find_menu_item(std::slice::from_ref(child), id) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }

    #[test]
    fn native_menus_change_modes_and_display_without_resetting_utilities() {
        use scarlet_ui::menu_model::{
            invoke_menu_callback, register_menu_callbacks, unregister_menu_callbacks,
        };
        const WINDOW: u32 = 0xc10c;
        struct Registration;
        impl Drop for Registration {
            fn drop(&mut self) {
                unregister_menu_callbacks(WINDOW);
            }
        }
        let _registration = Registration;
        let app = ClockApp::default();
        app.alpha.set(1.0);
        app.duration_input.set("00:05:00".into());
        app.timer_action();
        app.stopwatch.update(|watch| watch.start(monotonic_time()));

        let activate = |id: &str, changed: &dyn Fn() -> bool| {
            let menu = app.menu_bar();
            assert!(find_menu_item(menu.items(), id).unwrap().enabled_value());
            register_menu_callbacks(WINDOW, &menu);
            assert!(invoke_menu_callback(WINDOW, id));
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !changed() {
                assert!(std::time::Instant::now() < deadline, "menu callback: {id}");
                thread::sleep(Duration::from_millis(1));
            }
            assert!(
                !find_menu_item(app.menu_bar().items(), id)
                    .unwrap()
                    .enabled_value()
            );
        };
        app.focused.set(false);
        activate("timer", &|| app.mode.get() == 1 && app.focused.get());
        assert_eq!(
            find_menu_item(app.menu_bar().items(), "timer")
                .unwrap()
                .shortcut_value(),
            Some("Ctrl+2")
        );
        activate("background-5", &|| app.appearance.get() == 5);
        activate("opacity-50", &|| app.alpha.get() == 0.5);
        app.focused.set(false);
        activate("stopwatch", &|| app.mode.get() == 2 && app.focused.get());
        activate("clock", &|| app.mode.get() == 0);
        assert_eq!(
            app.bg_color(),
            Color::rgba_f32(presets()[5].0.r, presets()[5].0.g, presets()[5].0.b, 0.5)
        );
        assert_eq!(app.timer.get().phase(monotonic_time()), TimerPhase::Running);
        assert!(app.stopwatch.get().running());

        app.timer.update(|timer| {
            *timer = Timer::new(Duration::from_secs(1));
            timer.start(Duration::ZERO);
            timer.pause(Duration::from_secs(2));
        });
        assert_eq!(app.window_title(), "Timer Finished — Clock");
        assert_eq!(
            find_menu_item(app.menu_bar().items(), "timer")
                .unwrap()
                .title(),
            "Timer (Finished)"
        );
        app.timer.update(Timer::reset);
        assert_eq!(app.window_title(), "Clock");
        assert_eq!(
            find_menu_item(app.menu_bar().items(), "timer")
                .unwrap()
                .title(),
            "Timer"
        );
    }

    #[test]
    fn single_window_keeps_only_primary_controls_at_compact_size() {
        use scarlet_ui::SceneBuilder;
        use scarlet_ui::element::{Element, LayoutConstraints, RenderElement};
        use scarlet_ui::views::ButtonRenderObject;

        fn buttons(element: &dyn Element, labels: &mut Vec<String>) {
            if let Some(button) = element
                .as_any()
                .downcast_ref::<RenderElement<Button, ButtonRenderObject>>()
            {
                assert_eq!(button.view().get_font_size(), 13.0);
                assert!(button.bounds().size.height <= 30.0);
                labels.push(button.view().label().to_owned());
            }
            for child in element.children() {
                buttons(child.as_ref(), labels);
            }
        }

        let app = ClockApp::default();
        app.duration_input.set("00:05:00".into());
        for mode in 0..3 {
            app.select_mode(mode);
            let mut builder = SceneBuilder::new();
            app.scenes().build(&mut builder);
            let windows = builder.into_declarations();
            assert_eq!(windows.len(), 1, "no secondary settings window");
            assert!(windows[0].opens_at_launch);
            let mut element = windows[0].view.create_element();
            element.layout(LayoutConstraints::tight(340.0, 320.0));
            let mut labels = Vec::new();
            buttons(element.as_ref(), &mut labels);
            assert_eq!(
                labels,
                if mode == 0 {
                    Vec::<String>::new()
                } else {
                    vec!["Start".to_owned(), "Reset".to_owned()]
                }
            );
        }
    }

    fn shortcut(app: &ClockApp, keycode: KeyCode) -> bool {
        app.handle_key(KeyEvent::Pressed {
            keycode,
            modifiers: KeyModifiers {
                control: true,
                ..KeyModifiers::default()
            },
        })
    }

    #[test]
    fn timer_shortcuts_validate_then_start_pause_and_reset() {
        let app = ClockApp::default();
        assert!(shortcut(&app, KeyCode::Char('2')));
        app.duration_input.set("00:00:00".into());
        shortcut(&app, KeyCode::Space);
        assert_eq!(app.timer.get().phase(monotonic_time()), TimerPhase::Ready);
        assert!(!app.error.get().is_empty());
        app.duration_input.set("00:05:00".into());
        shortcut(&app, KeyCode::Space);
        assert_eq!(app.timer.get().phase(monotonic_time()), TimerPhase::Running);
        assert!(app.error.get().is_empty());
        shortcut(&app, KeyCode::Space);
        assert_eq!(app.timer.get().phase(monotonic_time()), TimerPhase::Paused);
        shortcut(&app, KeyCode::Char('r'));
        assert_eq!(app.timer.get().phase(monotonic_time()), TimerPhase::Ready);
    }

    #[test]
    fn stopwatch_shortcuts_leave_timer_state_independent() {
        let app = ClockApp::default();
        shortcut(&app, KeyCode::Char('3'));
        shortcut(&app, KeyCode::Space);
        assert!(app.stopwatch.get().running());
        shortcut(&app, KeyCode::Space);
        assert!(!app.stopwatch.get().running());
        shortcut(&app, KeyCode::Char('r'));
        assert!(app.stopwatch.get().elapsed(monotonic_time()).is_zero());
        assert_eq!(app.timer.get().phase(monotonic_time()), TimerPhase::Ready);
        assert!(!app.handle_key(KeyEvent::Pressed {
            keycode: KeyCode::Char('r'),
            modifiers: KeyModifiers::default(),
        }));
    }
}
