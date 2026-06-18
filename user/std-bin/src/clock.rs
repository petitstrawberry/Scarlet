use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use scarlet_os::time;
use scarlet_ui::graphics::Canvas;
use scarlet_ui::prelude::*;
use scarlet_ui::{CanvasView, MenuBarModel, MenuEntry, MenuItemModel, hstack, match_view, vstack};
use scarlet_ui_macros::View;

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
    base_color: State<Color>,
    alpha: State<f32>,
    show_settings: State<bool>,
}

impl ClockApp {
    fn bg_color(&self) -> Color {
        let c = self.base_color.get();
        let a = self.alpha.get();
        Color::rgba_f32(c.r, c.g, c.b, a)
    }
}

impl Application for ClockApp {
    fn body(&self) -> impl View {
        let _ = self.tick.get();
        let bg = self.bg_color();
        let show = self.show_settings.get();

        Window::new(
            if show { "Clock Settings" } else { "Clock" },
            match_view!(show, {
                true => settings_content(self.clone()),
                false => CanvasView::new(300.0, 300.0, Rc::new(move |buf, w, h| {
                    let mut canvas = Canvas::new(buf, w, h);
                    draw_clock(&mut canvas, w, h);
                })),
            })
            .frame(f32::INFINITY, f32::INFINITY),
        )
        .background_color(bg)
        .opaque(false)
        .app_id("org.scarlet-os.desktop.clock")
        .menu_bar(MenuBarModel::new(vec![MenuItemModel::app().children(
            vec![MenuEntry::Item(
                MenuItemModel::new("settings", "Settings...").on_activate(Arc::new({
                    let s = self.show_settings.clone();
                    move || {
                        s.set(true);
                    }
                })),
            )],
        )]))
        .size(Size::new(320.0, 320.0))
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn settings_content(app: ClockApp) -> impl View + Clone {
    let presets = presets();
    let current_base = app.base_color.get();
    let current_alpha = app.alpha.get();
    let base_state = app.base_color.clone();
    let alpha_state = app.alpha.clone();
    let show_state = app.show_settings.clone();

    vstack! {
        vstack! {
            Text::new("Background").font_size(16.0).color(Color::rgb(40, 40, 50)),
            Spacer::new().frame_height(12.0),

            hstack! {
                swatch(presets[0].0, presets[0].1, current_base, base_state.clone()),
                Spacer::new().frame_width(12.0),
                swatch(presets[1].0, presets[1].1, current_base, base_state.clone()),
                Spacer::new().frame_width(12.0),
                swatch(presets[2].0, presets[2].1, current_base, base_state.clone()),
            },

            Spacer::new().frame_height(12.0),

            hstack! {
                swatch(presets[3].0, presets[3].1, current_base, base_state.clone()),
                Spacer::new().frame_width(12.0),
                swatch(presets[4].0, presets[4].1, current_base, base_state.clone()),
                Spacer::new().frame_width(12.0),
                swatch(presets[5].0, presets[5].1, current_base, base_state.clone()),
            },

            Spacer::new().frame_height(16.0),

            Text::new(format!("Opacity: {}%", (current_alpha * 100.0) as u32))
                .font_size(13.0)
                .color(Color::rgb(60, 60, 70)),
            Slider::new(alpha_state).min(0.0).max(1.0),

            Spacer::new().frame_height(16.0),
            Button::new("Done").on_click(move || { show_state.set(false); }),
        }
        .padding(20.0)
    }
}

fn swatch(
    color: Color,
    label: &str,
    current: Color,
    base_state: State<Color>,
) -> impl View + Clone {
    let is_current = current == color;
    vstack! {
        Rectangle::new()
            .fill(color)
            .border(
                if is_current { 3.0 } else { 1.0 },
                if is_current { Color::rgb(255, 149, 0) } else { Color::rgb(80, 80, 90) },
            )
            .frame(56.0, 56.0)
            .on_click(move || { base_state.set(color); }),
        Text::new(label).font_size(11.0).color(Color::rgb(60, 60, 70)),
    }
    .alignment(Alignment::Center)
}

fn draw_clock(canvas: &mut Canvas, w: u32, h: u32) {
    let utc_ns = time::system_time_ns().unwrap_or(0);
    let offset = time::local_utc_offset_seconds().unwrap_or(0);
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
                Color::rgb(180, 180, 200),
            )
        } else {
            (
                (radius - 7) as f32,
                radius as f32,
                Color::rgb(110, 110, 135),
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

    draw_hand(hour_deg, radius * 6 / 10, Color::rgb(200, 200, 210));
    draw_hand(min_deg, radius * 8 / 10, Color::rgb(160, 180, 200));
    draw_hand(sec_deg, radius * 9 / 10, Color::rgb(255, 149, 0));

    canvas.fill_rect(cx - 4, cy - 4, 8, 8, Color::rgb(255, 255, 255));
}

fn main() -> ExitCode {
    println!("[clock] starting");
    let mut app = ClockApp {
        tick: State::new(StateId::new(0), 0),
        base_color: State::new(StateId::new(1), Color::rgb(30, 30, 40)),
        alpha: State::new(StateId::new(3), 1.0),
        show_settings: State::new(StateId::new(2), false),
    };

    let tick = app.tick.clone();
    thread::spawn(move || {
        loop {
            let ns = time::system_time_ns().unwrap_or(0);
            let ms_into_sec = (ns / 1_000_000) % 1000;
            let ms_until_next = (1000 - ms_into_sec).max(50);
            thread::sleep(Duration::from_millis(ms_until_next));
            tick.set(tick.get() + 1);
        }
    });

    match app.run() {
        Ok(()) => {
            println!("[clock] exited");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[clock] error: {}", e);
            ExitCode::FAILURE
        }
    }
}
