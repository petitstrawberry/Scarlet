use std::process::ExitCode;
use std::rc::Rc;
use std::thread;
use std::time::Duration;

use scarlet_os::time;
use scarlet_ui::CanvasView;
use scarlet_ui::graphics::Canvas;
use scarlet_ui::prelude::*;
use scarlet_ui_macros::View;

#[derive(View, Clone)]
struct ClockApp {
    tick: State<u64>,
}

impl Application for ClockApp {
    fn body(&self) -> impl View {
        let _ = self.tick.get();

        Window::new(
            "Clock",
            CanvasView::new(
                300.0,
                300.0,
                Rc::new(|buf, w, h| {
                    let utc_ns = time::system_time_ns().unwrap_or(0);
                    let offset = time::local_utc_offset_seconds().unwrap_or(0);
                    let secs = (utc_ns / 1_000_000_000) as i64 + offset;
                    let sod = (((secs % 86400) + 86400) % 86400) as u64;
                    let hh = (sod / 3600) % 12;
                    let mm = (sod / 60) % 60;
                    let ss = sod % 60;

                    let mut canvas = Canvas::new(buf, w, h);
                    canvas.fill_rect(0, 0, w, h, Color::rgb(30, 30, 40));

                    let cx = w as i32 / 2;
                    let cy = h as i32 / 2;
                    let radius = (w.min(h) as i32 / 2 - 20).max(40);

                    for i in 0..60 {
                        let a = (i as f32) * (core::f32::consts::TAU / 60.0)
                            - core::f32::consts::FRAC_PI_2;
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
                }),
            )
            .frame(f32::INFINITY, f32::INFINITY),
        )
        .app_id("org.scarlet-os.desktop.clock")
        .decorated(true)
        .size(Size::new(320.0, 320.0))
    }

    fn debug_logging(&self) -> bool {
        false
    }
}

fn main() -> ExitCode {
    println!("[clock] starting");
    let mut app = ClockApp {
        tick: State::new(StateId::new(0), 0),
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
