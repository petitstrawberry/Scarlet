//! UI Demo - ScarletUI Modern Architecture Demo
//!
//! This demo showcases:
//! - New View/RenderObject architecture
//! - Declarative UI composition
//! - Method chaining for View configuration
//! - Navigation controls (ScrollView, TabView, NavigationView)

#![no_std]
#![no_main]

extern crate alloc;

use scarlet_ui::{
    Application, Window, WindowBuilder,
    VStack, HStack,
    Text, Button, Toggle, Slider, TextField,
    Local, View, ViewExt,
    Color,
};
use alloc::{string::String, format, boxed::Box, sync::Arc};
use scarlet_std::println;

/// Demo view - Shows various UI components
struct DemoView {
    id: scarlet_ui::ViewId,
    counter: Local<i32>,
}

impl DemoView {
    fn new() -> Self {
        Self {
            id: scarlet_ui::ViewId::new(),
            counter: Local::new(0),
        }
    }

    fn build(&self) -> impl View {
        VStack::new()
            .spacing(16)
            // Header
            .child(
                Text::new("ScarletUI Modern Architecture Demo")
                    .font_size(28)
            )
            .child(
                Text::new("View/RenderObject separation with SwiftUI-style API")
                    .font_size(14)
            )
            .child(
                Text::new("Counter: 0")
                    .font_size(24)
            )
            .child(
                HStack::new()
                    .spacing(10)
                    .child(
                        Button::new("Decrement")
                            .padding(10)
                    )
                    .child(
                        Button::new("Increment")
                            .padding(10)
                    )
            )
            .child(
                Toggle::new(true)
            )
            .child(
                Slider::new(0.0, 100.0)
                    .value(50.0)
            )
            .child(
                TextField::new()
            )
            .child(
                Button::new("Open TabView Demo")
                    .action(|| {
                        println!("[ui_demo] TabView demo requested");
                    })
                    .padding(10)
            )
            .child(
                Button::new("Open NavigationView Demo")
                    .action(|| {
                        println!("[ui_demo] NavigationView demo requested");
                    })
                    .padding(10)
            )
    }
}

impl scarlet_ui::view::render::RenderObject for DemoView {
    fn id(&self) -> scarlet_ui::ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, ctx: &mut scarlet_ui::LayoutCtx, constraints: scarlet_ui::LayoutConstraints) -> scarlet_ui::Size {
        // Default size
        let size = scarlet_ui::Size::new(600, 700);
        size
    }

    fn draw(&self, ctx: &mut scarlet_ui::PaintCtx, frame: scarlet_ui::graphics::Rect) {
        // Draw the body
        let body = self.build();
        body.draw(ctx, frame);
    }

    fn event(&mut self, ctx: &mut scarlet_ui::EventCtx, event: &scarlet_ui::Event) -> scarlet_ui::ControlFlow {
        self.build().event(ctx, event)
    }

    fn update(&mut self, ctx: &mut scarlet_ui::UpdateCtx) {
        self.build().update(ctx)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[ui_demo] Starting ScarletUI Modern Architecture Demo");

    let mut app = match Application::new() {
        Ok(mut a) => {
            a.app_id("org.scarlet.ui_demo");
            a
        }
        Err(e) => {
            println!("[ui_demo] Failed to create application: {}", e);
            return 1;
        }
    };

    // Create main window
    let demo_view = DemoView::new();

    // Build the UI tree
    let ui_content = VStack::new()
        .spacing(16)
        .child(
            Text::new("ScarletUI Modern Architecture Demo")
                .font_size(28)
        )
        .child(
            Text::new("View/RenderObject separation with SwiftUI-style API")
                .font_size(14)
        )
        .child(
            Text::new("Counter: 0")
                .font_size(24)
        )
        .child(
            HStack::new()
                .spacing(10)
                .child(
                    Button::new("Decrement")
                        .padding(10)
                )
                .child(
                    Button::new("Increment")
                        .padding(10)
                )
        )
        .child(
            Toggle::new(true)
        )
        .child(
            Slider::new(0.0, 100.0)
                .value(50.0)
        )
        .child(
            TextField::new()
        );

    let window = Window::builder()
        .title("ScarletUI Modern Architecture Demo")
        .size(650, 720)
        .min_size(400, 500)
        .build()
        .content(ui_content);

    if let Err(e) = app.add_window(window) {
        println!("[ui_demo] Failed to add window: {}", e);
        return 1;
    }

    println!("[ui_demo] Running application...");
    app.run();
}
