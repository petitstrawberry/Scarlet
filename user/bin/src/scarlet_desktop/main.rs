//! Scarlet Desktop - A composited desktop surface rendered with ScarletUI on SWS.
//!
//! This app builds a desktop-grade surface using ScarletUI widgets and marks the
//! window as a `DESKTOP` layer so it sits underneath regular application windows.

#![no_std]
#![no_main]

extern crate scarlet_std as std;

use core::time::Duration;
use scarlet_std::boxed::Box;
use scarlet_std::vec::Vec;
use scarlet_ui::event::Event;
use scarlet_ui::graphics::{Canvas, Rect};
use scarlet_ui::{
    Application, Button, Color, HStack, Label, Padding, RectView, Size, Spacer, StackAlignment,
    State, Text, Timer, VStack, View, ViewBox, Window, WindowKind,
};
use std::{format, println};

/// Simple card container with rounded background and padding.
struct Card {
    body: ViewBox,
    background: Color,
    border_color: Option<Color>,
    border_width: u32,
    corner_radius: u32,
    padding: u32,
    cached_size: Size,
}

impl Card {
    fn new<V: View + 'static>(body: V) -> Self {
        Self {
            body: Box::new(body),
            background: Color::rgb(30, 34, 44),
            border_color: None,
            border_width: 0,
            corner_radius: 12,
            padding: 16,
            cached_size: Size::ZERO,
        }
    }

    fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    fn border(mut self, width: u32, color: Color) -> Self {
        self.border_width = width;
        self.border_color = Some(color);
        self
    }

    fn padding(mut self, pad: u32) -> Self {
        self.padding = pad;
        self
    }

    fn corner_radius(mut self, radius: u32) -> Self {
        self.corner_radius = radius;
        self
    }
}

impl View for Card {
    fn layout(&mut self, available: Size) -> Size {
        let inner = Size::new(
            available.width.saturating_sub(self.padding * 2),
            available.height.saturating_sub(self.padding * 2),
        );
        let body_size = self.body.layout(inner);
        self.cached_size = body_size;
        Size::new(
            body_size.width.saturating_add(self.padding * 2),
            body_size.height.saturating_add(self.padding * 2),
        )
    }

    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        canvas.fill_rounded_rect(
            frame.x,
            frame.y,
            frame.width,
            frame.height,
            self.corner_radius,
            self.background,
        );

        if let Some(border_color) = self.border_color {
            for i in 0..self.border_width {
                canvas.draw_rounded_rect(
                    frame.x + i as i32,
                    frame.y + i as i32,
                    frame.width.saturating_sub(i * 2),
                    frame.height.saturating_sub(i * 2),
                    self.corner_radius.saturating_sub(i),
                    border_color,
                );
            }
        }

        let child_frame = Rect::new(
            frame.x + self.padding as i32,
            frame.y + self.padding as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        self.body.draw(canvas, child_frame);
    }

    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        let child_frame = Rect::new(
            frame.x + self.padding as i32,
            frame.y + self.padding as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        if child_frame.contains(event.x(), event.y()) {
            self.body.on_event(event, child_frame)
        } else {
            false
        }
    }

    fn children(&self) -> Vec<(&dyn View, Rect)> {
        let child_frame = Rect::new(
            self.padding as i32,
            self.padding as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        let mut out = Vec::new();
        out.push((self.body.as_ref() as &dyn View, child_frame));
        out
    }

    fn children_mut(&mut self) -> Vec<(&mut dyn View, Rect)> {
        let child_frame = Rect::new(
            self.padding as i32,
            self.padding as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        let mut out = Vec::new();
        out.push((self.body.as_mut() as &mut dyn View, child_frame));
        out
    }

    fn visit_children(&self, visitor: &mut dyn FnMut(&dyn View, Rect) -> bool) {
        let child_frame = Rect::new(
            self.padding as i32,
            self.padding as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        let _ = visitor(self.body.as_ref() as &dyn View, child_frame);
    }

    fn visit_children_mut(&mut self, visitor: &mut dyn FnMut(&mut dyn View, Rect) -> bool) {
        let child_frame = Rect::new(
            self.padding as i32,
            self.padding as i32,
            self.cached_size.width,
            self.cached_size.height,
        );
        let _ = visitor(self.body.as_mut() as &mut dyn View, child_frame);
    }

    fn needs_draw(&self) -> bool {
        self.body.needs_draw()
    }

    fn set_needs_draw(&mut self) {
        self.body.set_needs_draw();
    }

    fn clear_needs_draw(&mut self) {
        self.body.clear_needs_draw();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[scarlet_desktop] Launching Scarlet Desktop");

    let mut app = match Application::new() {
        Ok(app) => app,
        Err(e) => {
            println!("[scarlet_desktop] Failed to connect to SWS: {}", e);
            return 1;
        }
    };

    // Lightweight telemetry states to keep the desktop feeling alive.
    let uptime_seconds = State::new(0u32);
    let load = State::new(32u8);
    let memory = State::new(58u8);

    Timer::periodic(Duration::from_secs(1), {
        let uptime = uptime_seconds.clone();
        move || {
            uptime.update(|t| *t = t.saturating_add(1));
        }
    });

    // Small pulse so the cards animate without real metrics.
    Timer::periodic(Duration::from_millis(900), {
        let load = load.clone();
        let memory = memory.clone();
        move || {
            load.update(|v| {
                let next = v.wrapping_add(11);
                *v = (next % 85) + 10;
            });
            memory.update(|v| {
                let next = v.wrapping_add(7);
                *v = (next % 70) + 25;
            });
        }
    });

    let accent = Color::rgb(240, 96, 72);
    let cyan = Color::rgb(54, 176, 168);
    let lilac = Color::rgb(120, 126, 196);
    let card_border = Color::rgb(60, 68, 88);
    let page_bg = Color::rgb(18, 22, 30);
    let text_dim = Color::rgb(175, 186, 208);

    let hero = Card::new(
        VStack::new()
            .spacing(12)
            .alignment(StackAlignment::Start)
            .child(
                Label::new("Scarlet Desktop")
                    .color(Color::rgb(238, 242, 249))
                    .font_size(36),
            )
            .child(
                Label::new("Composited with SWS + ScarletUI. Stay grounded, stay responsive.")
                    .color(text_dim)
                    .font_size(18),
            )
            .child(
                HStack::new()
                    .spacing(12)
                    .alignment(StackAlignment::Start)
                    .child(
                        Button::new("Launch Terminal", || {
                            println!("[scarlet_desktop] Action: launch terminal (stub)");
                        })
                        .background(accent)
                        .text_color(Color::WHITE)
                        .corner_radius(10),
                    )
                    .child(
                        Button::new("Workspace Overview", || {
                            println!("[scarlet_desktop] Action: show overview (stub)");
                        })
                        .background(cyan)
                        .text_color(Color::rgb(14, 36, 42))
                        .corner_radius(10),
                    )
                    .child(
                        Button::new("Calm Mode", || {
                            println!("[scarlet_desktop] Action: toggle calm mode (stub)");
                        })
                        .background(lilac)
                        .text_color(Color::WHITE)
                        .corner_radius(10),
                    ),
            )
            .child(
                HStack::new()
                    .spacing(10)
                    .alignment(StackAlignment::Start)
                    .child(RectView::new(accent).width(8).height(32).corner_radius(4))
                    .child(
                        VStack::new()
                            .alignment(StackAlignment::Start)
                            .spacing(2)
                            .child(
                                Text::new({
                                    let uptime = uptime_seconds.clone();
                                    move || {
                                        let total = uptime.get();
                                        let hrs = total / 3600;
                                        let mins = (total / 60) % 60;
                                        let secs = total % 60;
                                        format!("Session uptime {:02}:{:02}:{:02}", hrs, mins, secs)
                                    }
                                })
                                .watch(uptime_seconds.clone())
                                .color(Color::WHITE)
                                .font_size(14),
                            )
                            .child(
                                Label::new("SWS linked • desktop layer pinned")
                                    .color(text_dim)
                                    .font_size(14),
                            ),
                    ),
            ),
    )
    .background(Color::rgb(30, 36, 48))
    .border(1, card_border)
    .corner_radius(14)
    .padding(20);

    let system_card = Card::new(
        VStack::new()
            .spacing(8)
            .alignment(StackAlignment::Start)
            .child(
                HStack::new()
                    .spacing(8)
                    .alignment(StackAlignment::Center)
                    .child(RectView::new(cyan).width(10).height(22).corner_radius(4))
                    .child(Label::new("System pulse").color(Color::WHITE).font_size(18)),
            )
            .child(
                Label::new("Ambient health and faux metrics")
                    .color(text_dim)
                    .font_size(14),
            )
            .child(
                VStack::new()
                    .spacing(6)
                    .alignment(StackAlignment::Start)
                    .child(
                        Text::new({
                            let load = load.clone();
                            move || format!("Load envelope {:>2}%", load.get())
                        })
                        .watch(load.clone())
                        .color(Color::rgb(222, 232, 240))
                        .font_size(16),
                    )
                    .child(
                        Text::new({
                            let memory = memory.clone();
                            move || {
                                format!(
                                    "Memory headroom {:>2}%",
                                    100u32.saturating_sub(memory.get() as u32)
                                )
                            }
                        })
                        .watch(memory.clone())
                        .color(Color::rgb(222, 232, 240))
                        .font_size(16),
                    )
                    .child(
                        Text::new({
                            let uptime = uptime_seconds.clone();
                            move || format!("Sched tick {}", uptime.get() % 2048)
                        })
                        .watch(uptime_seconds.clone())
                        .color(text_dim)
                        .font_size(14),
                    ),
            ),
    )
    .border(1, card_border);

    let workspace_card = Card::new(
        VStack::new()
            .spacing(8)
            .alignment(StackAlignment::Start)
            .child(
                HStack::new()
                    .spacing(8)
                    .alignment(StackAlignment::Center)
                    .child(RectView::new(lilac).width(10).height(22).corner_radius(4))
                    .child(Label::new("Workspaces").color(Color::WHITE).font_size(18)),
            )
            .child(
                Label::new("Arrange focus without leaving the home screen")
                    .color(text_dim)
                    .font_size(14),
            )
            .child(
                VStack::new()
                    .spacing(6)
                    .alignment(StackAlignment::Start)
                    .child(
                        Label::new("• Build & logs  — anchored left")
                            .color(Color::WHITE)
                            .font_size(15),
                    )
                    .child(
                        Label::new("• Docs & notes — center column")
                            .color(Color::WHITE)
                            .font_size(15),
                    )
                    .child(
                        Label::new("• Experiments  — floating stack")
                            .color(Color::WHITE)
                            .font_size(15),
                    ),
            )
            .child(
                HStack::new()
                    .spacing(8)
                    .alignment(StackAlignment::Start)
                    .child(
                        Button::new("Pin", || {
                            println!("[scarlet_desktop] Action: pin workspace (stub)");
                        })
                        .background(Color::rgb(60, 80, 110))
                        .text_color(Color::WHITE)
                        .corner_radius(8),
                    )
                    .child(
                        Button::new("Detach", || {
                            println!("[scarlet_desktop] Action: detach workspace (stub)");
                        })
                        .background(Color::rgb(70, 96, 82))
                        .text_color(Color::WHITE)
                        .corner_radius(8),
                    ),
            ),
    )
    .border(1, card_border);

    let network_card = Card::new(
        VStack::new()
            .spacing(8)
            .alignment(StackAlignment::Start)
            .child(
                HStack::new()
                    .spacing(8)
                    .alignment(StackAlignment::Center)
                    .child(RectView::new(accent).width(10).height(22).corner_radius(4))
                    .child(
                        Label::new("Sessions & links")
                            .color(Color::WHITE)
                            .font_size(18),
                    ),
            )
            .child(
                Label::new("Status for shell, apps, and network reachability")
                    .color(text_dim)
                    .font_size(14),
            )
            .child(
                VStack::new()
                    .spacing(6)
                    .alignment(StackAlignment::Start)
                    .child(
                        Label::new("Shell: attached • interactive")
                            .color(Color::WHITE)
                            .font_size(15),
                    )
                    .child(
                        Label::new("Desktop: ready • window server live")
                            .color(Color::WHITE)
                            .font_size(15),
                    )
                    .child(
                        Label::new("Network: loopback + vsock bridge")
                            .color(Color::WHITE)
                            .font_size(15),
                    ),
            )
            .child(
                HStack::new()
                    .spacing(8)
                    .alignment(StackAlignment::Start)
                    .child(
                        Button::new("Snapshot", || {
                            println!("[scarlet_desktop] Action: snapshot state (stub)");
                        })
                        .background(Color::rgb(66, 68, 88))
                        .text_color(Color::WHITE)
                        .corner_radius(8),
                    )
                    .child(
                        Button::new("Re-link", || {
                            println!("[scarlet_desktop] Action: re-link network (stub)");
                        })
                        .background(Color::rgb(90, 78, 70))
                        .text_color(Color::WHITE)
                        .corner_radius(8),
                    ),
            ),
    )
    .border(1, card_border);

    let window = Window::new("Scarlet Desktop", 1260, 780)
        .min_size(960, 620)
        .background(page_bg)
        .window_type(WindowKind::Desktop)
        .content(
            Padding::new(
                VStack::new()
                    .spacing(16)
                    .alignment(StackAlignment::Start)
                    .child(hero)
                    .child(
                        HStack::new()
                            .spacing(14)
                            .alignment(StackAlignment::Start)
                            .child(system_card)
                            .child(workspace_card)
                            .child(network_card),
                    )
                    .child(
                        HStack::new()
                            .spacing(12)
                            .alignment(StackAlignment::Start)
                            .child(
                                Button::new("Log out", || {
                                    println!("[scarlet_desktop] Action: log out (stub)");
                                })
                                .background(Color::rgb(76, 92, 118))
                                .text_color(Color::WHITE)
                                .corner_radius(10),
                            )
                            .child(
                                Button::new("Sleep", || {
                                    println!("[scarlet_desktop] Action: sleep (stub)");
                                })
                                .background(Color::rgb(96, 76, 92))
                                .text_color(Color::WHITE)
                                .corner_radius(10),
                            )
                            .child(
                                Button::new("Diagnostics", || {
                                    println!("[scarlet_desktop] Action: diagnostics (stub)");
                                })
                                .background(Color::rgb(74, 96, 76))
                                .text_color(Color::WHITE)
                                .corner_radius(10),
                            )
                            .child(Spacer::new()),
                    ),
            )
            .all(24),
        );

    if let Err(e) = app.add_window(window) {
        println!("[scarlet_desktop] Failed to add window: {}", e);
        return 1;
    }

    app.run();
}
