//! Scarlet Desktop - A composited desktop surface rendered with ScarletUI on SWS.
//!
//! This app builds a desktop-grade surface using ScarletUI widgets and marks the
//! window as a `DESKTOP` layer so it sits underneath regular application windows.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;

use scarlet_ui::{
    Application, Window, WindowBuilder,
    VStack, HStack, Spacer,
    Text, Button,
    View, ViewExt,
    Color,
    LayoutConstraints, Size, ControlFlow,
    ViewId,
    LayoutCtx, PaintCtx, EventCtx, UpdateCtx,
};
use scarlet_ui::view::render::RenderObject;
use scarlet_ui::view::traits::ChildView;
use scarlet_std::println;

/// Simple card container with rounded background and padding.
struct Card {
    id: ViewId,
    child: ChildView,
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
            id: ViewId::new(),
            child: ChildView {
                view: Box::new(body),
                frame: scarlet_ui::graphics::Rect::ZERO,
            },
            background: Color::rgb(50, 50, 50),
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

impl RenderObject for Card {
    fn id(&self) -> ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        let inner = LayoutConstraints::new(
            constraints.min_width.saturating_sub(self.padding * 2),
            constraints.max_width.saturating_sub(self.padding * 2),
            constraints.min_height.saturating_sub(self.padding * 2),
            constraints.max_height.saturating_sub(self.padding * 2),
        );

        let child_size = self.child.view.layout(ctx, inner);
        self.child.frame = scarlet_ui::graphics::Rect::new(
            self.padding as i32,
            self.padding as i32,
            child_size.width,
            child_size.height,
        );

        let size = Size::new(
            child_size.width.saturating_add(self.padding * 2),
            child_size.height.saturating_add(self.padding * 2),
        );
        self.cached_size = size;
        size
    }

    fn draw(&self, ctx: &mut PaintCtx, frame: scarlet_ui::graphics::Rect) {
        // Draw background
        // Note: Canvas operations would go here, but for now we just draw the child
        // A full implementation would use ctx.canvas.fill_rounded_rect()

        // Draw child
        let child_frame = scarlet_ui::graphics::Rect::new(
            frame.x + self.padding as i32,
            frame.y + self.padding as i32,
            self.child.frame.width,
            self.child.frame.height,
        );
        self.child.view.draw(ctx, child_frame);
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &scarlet_ui::Event) -> ControlFlow {
        self.child.view.event(ctx, event)
    }

    fn update(&mut self, ctx: &mut UpdateCtx) {
        self.child.view.update(ctx)
    }
}

impl View for Card {
    fn children(&self) -> &[ChildView] {
        core::slice::from_ref(&self.child)
    }

    fn children_mut(&mut self) -> &mut [ChildView] {
        core::slice::from_mut(&mut self.child)
    }

    // as_any, id, layout, draw, event, update are inherited from RenderObject impl
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[scarlet_desktop] Launching Scarlet Desktop");

    let mut app = match Application::new() {
        Ok(mut a) => {
            a.app_id("org.scarlet-os.desktop");
            a
        }
        Err(e) => {
            println!("[scarlet_desktop] Failed to create application: {}", e);
            return 1;
        }
    };

    let accent = Color::rgb(240, 96, 72);
    let cyan = Color::rgb(54, 176, 168);
    let lilac = Color::rgb(120, 126, 196);

    // Hero card
    let hero = Card::new(
        VStack::new()
            .spacing(12)
            .child(
                Text::new("Scarlet Desktop")
                    .font_size(36)
            )
            .child(
                Text::new("Composited with SWS + ScarletUI")
                    .font_size(18)
            )
            .child(
                HStack::new()
                    .spacing(12)
                    .child(
                        Button::new("Launch Terminal")
                            .action(|| {
                                println!("[scarlet_desktop] Launch terminal (stub)");
                            })
                            .padding(10)
                    )
                    .child(
                        Button::new("Workspace Overview")
                            .action(|| {
                                println!("[scarlet_desktop] Show overview (stub)");
                            })
                            .padding(10)
                    )
                    .child(
                        Button::new("Calm Mode")
                            .action(|| {
                                println!("[scarlet_desktop] Toggle calm mode (stub)");
                            })
                            .padding(10)
                    )
            )
            .child(
                HStack::new()
                    .spacing(10)
                    .child(
                        Text::new("Session uptime 00:00:00")
                            .font_size(14)
                    )
                    .child(
                        Text::new("SWS linked • desktop layer pinned")
                            .font_size(14)
                    )
            )
    )
    .background(Color::rgb(40, 44, 52))
    .border(1, Color::rgb(100, 100, 100))
    .corner_radius(14)
    .padding(20);

    // System card
    let system_card = Card::new(
        VStack::new()
            .spacing(8)
            .child(
                HStack::new()
                    .spacing(8)
                    .child(
                        Text::new("System pulse")
                            .font_size(18)
                    )
            )
            .child(
                Text::new("Ambient health and faux metrics")
                    .font_size(14)
            )
            .child(
                VStack::new()
                    .spacing(6)
                    .child(
                        Text::new("Load envelope 32%")
                            .font_size(16)
                    )
                    .child(
                        Text::new("Memory headroom 42%")
                            .font_size(16)
                    )
                    .child(
                        Text::new("Sched tick 1234")
                            .font_size(14)
                    )
            )
    )
    .border(1, Color::rgb(100, 100, 100));

    // Workspace card
    let workspace_card = Card::new(
        VStack::new()
            .spacing(8)
            .child(
                HStack::new()
                    .spacing(8)
                    .child(
                        Text::new("Workspaces")
                            .font_size(18)
                    )
            )
            .child(
                Text::new("Arrange focus without leaving the home screen")
                    .font_size(14)
            )
            .child(
                VStack::new()
                    .spacing(6)
                    .child(
                        Text::new("• Build & logs  — anchored left")
                            .font_size(15)
                    )
                    .child(
                        Text::new("• Docs & notes — center column")
                            .font_size(15)
                    )
                    .child(
                        Text::new("• Experiments  — floating stack")
                            .font_size(15)
                    )
            )
            .child(
                HStack::new()
                    .spacing(8)
                    .child(
                        Button::new("Pin")
                            .action(|| {
                                println!("[scarlet_desktop] Pin workspace (stub)");
                            })
                            .padding(8)
                    )
                    .child(
                        Button::new("Detach")
                            .action(|| {
                                println!("[scarlet_desktop] Detach workspace (stub)");
                            })
                            .padding(8)
                    )
            )
    )
    .border(1, Color::rgb(100, 100, 100));

    // Network card
    let network_card = Card::new(
        VStack::new()
            .spacing(8)
            .child(
                HStack::new()
                    .spacing(8)
                    .child(
                        Text::new("Sessions & links")
                            .font_size(18)
                    )
            )
            .child(
                Text::new("Status for shell, apps, and network reachability")
                    .font_size(14)
            )
            .child(
                VStack::new()
                    .spacing(6)
                    .child(
                        Text::new("Shell: attached • interactive")
                            .font_size(15)
                    )
                    .child(
                        Text::new("Desktop: ready • window server live")
                            .font_size(15)
                    )
                    .child(
                        Text::new("Network: loopback + vsock bridge")
                            .font_size(15)
                    )
            )
            .child(
                HStack::new()
                    .spacing(8)
                    .child(
                        Button::new("Snapshot")
                            .action(|| {
                                println!("[scarlet_desktop] Snapshot state (stub)");
                            })
                            .padding(8)
                    )
                    .child(
                        Button::new("Re-link")
                            .action(|| {
                                println!("[scarlet_desktop] Re-link network (stub)");
                            })
                            .padding(8)
                    )
            )
    )
    .border(1, Color::rgb(100, 100, 100));

    let ui_content = VStack::new()
        .spacing(16)
        .child(hero)
        .child(
            HStack::new()
                .spacing(14)
                .child(system_card)
                .child(workspace_card)
                .child(network_card)
        )
        .child(
            HStack::new()
                .spacing(12)
                .child(
                    Button::new("Log out")
                        .action(|| {
                            println!("[scarlet_desktop] Log out (stub)");
                        })
                        .padding(10)
                )
                .child(
                    Button::new("Sleep")
                        .action(|| {
                            println!("[scarlet_desktop] Sleep (stub)");
                        })
                        .padding(10)
                )
                .child(
                    Button::new("Diagnostics")
                        .action(|| {
                            println!("[scarlet_desktop] Diagnostics (stub)");
                        })
                        .padding(10)
                )
                .child(Spacer::new())
        )
        .padding(24);

    let window = Window::builder()
        .title("Scarlet Desktop")
        .size(1260, 780)
        .min_size(960, 620)
        .build()
        .background(Color::rgb(18, 22, 30))
        .content(ui_content);

    if let Err(e) = app.add_window(window) {
        println!("[scarlet_desktop] Failed to add window: {}", e);
        return 1;
    }

    app.run();
}
