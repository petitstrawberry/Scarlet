//! Scarlet Desktop Notepad
//!
//! A simple text editor application built with ScarletUI.

#![no_std]
#![no_main]

extern crate alloc;

use scarlet_ui::{
    Application, Window, WindowBuilder,
    VStack, HStack, Spacer,
    Text, TextField, Button,
    View, ViewExt,
    Color,
    LayoutConstraints, Size, ControlFlow,
};
use alloc::{string::String, format, boxed::Box, sync::Arc};
use scarlet_std::println;

/// Notepad view
struct NotepadView {
    id: scarlet_ui::ViewId,
}

impl NotepadView {
    fn new() -> Self {
        Self {
            id: scarlet_ui::ViewId::new(),
        }
    }

    fn build(&self) -> impl View {
        VStack::new()
            .spacing(0)
            // Menu bar
            .child(
                HStack::new()
                    .spacing(8)
                    .child(
                        Text::new("File")
                            .font_size(13)
                    )
                    .child(
                        Button::new("New")
                            .action(|| {
                                println!("[notepad] New document");
                            })
                            .padding(6)
                    )
                    .child(
                        Button::new("Open")
                            .action(|| {
                                println!("[notepad] Open file");
                            })
                            .padding(6)
                    )
                    .child(
                        Button::new("Save")
                            .action(|| {
                                println!("[notepad] Save file");
                            })
                            .padding(6)
                    )
                    .child(Spacer::new())
                    .child(
                        Text::new("Ctrl+N: New | Ctrl+O: Open | Ctrl+S: Save")
                            .font_size(11)
                    )
                    .child(Spacer::new())
                    .padding(12)
            )
            // Separator (using background color for now)
            .child(
                HStack::new()
                    .background(Color::rgb(200, 200, 200))
                    .frame(1, 1)
            )
            // File info bar
            .child(
                HStack::new()
                    .spacing(12)
                    .child(
                        Text::new("File:")
                            .font_size(12)
                    )
                    .child(
                        Text::new("Untitled")
                            .font_size(12)
                    )
                    .child(Spacer::new())
                    .padding(16)
                    .padding(8)
            )
            // Text editing area
            .child(
                TextField::new()
                    .placeholder("Type your text here...")
                    .padding(16)
            )
            .child(Spacer::new())
            // Status bar separator
            .child(
                HStack::new()
                    .background(Color::rgb(200, 200, 200))
                    .frame(1, 1)
            )
            // Status bar
            .child(
                HStack::new()
                    .spacing(16)
                    .child(
                        Text::new("Line:")
                            .font_size(11)
                    )
                    .child(
                        Text::new("1")
                            .font_size(11)
                    )
                    .child(
                        Text::new("Column:")
                            .font_size(11)
                    )
                    .child(
                        Text::new("1")
                            .font_size(11)
                    )
                    .child(Spacer::new())
                    .child(
                        Text::new("Ready - New document")
                            .font_size(11)
                    )
                    .child(Spacer::new())
                    .child(
                        Text::new("UTF-8")
                            .font_size(11)
                    )
                    .padding(12)
            )
    }
}

impl scarlet_ui::view::render::RenderObject for NotepadView {
    fn id(&self) -> scarlet_ui::ViewId {
        self.id
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn layout(&mut self, _ctx: &mut scarlet_ui::LayoutCtx, _constraints: scarlet_ui::LayoutConstraints) -> scarlet_ui::Size {
        Size::new(950, 680)
    }

    fn draw(&self, ctx: &mut scarlet_ui::PaintCtx, frame: scarlet_ui::graphics::Rect) {
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

impl View for NotepadView {}

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    println!("[notepad] Starting Scarlet Desktop Notepad");

    let mut app = match Application::new() {
        Ok(mut a) => {
            a.app_id("org.scarlet-os.desktop.notepad");
            a
        }
        Err(e) => {
            println!("[notepad] Failed to create application: {}", e);
            return 1;
        }
    };

    // Build the UI tree
    let ui_content = VStack::new()
        .spacing(0)
        // Menu bar
        .child(
            HStack::new()
                .spacing(8)
                .child(
                    Text::new("File")
                        .font_size(13)
                )
                .child(
                    Button::new("New")
                        .action(|| {
                            println!("[notepad] New document");
                        })
                        .padding(6)
                )
                .child(
                    Button::new("Open")
                        .action(|| {
                            println!("[notepad] Open file");
                        })
                        .padding(6)
                )
                .child(
                    Button::new("Save")
                        .action(|| {
                            println!("[notepad] Save file");
                        })
                        .padding(6)
                )
                .child(Spacer::new())
                .child(
                    Text::new("Ctrl+N: New | Ctrl+O: Open | Ctrl+S: Save")
                        .font_size(11)
                )
                .child(Spacer::new())
                .padding(12)
        )
        // Separator
        .child(
            HStack::new()
                .background(Color::rgb(200, 200, 200))
                .frame(1, 1)
        )
        // File info bar
        .child(
            HStack::new()
                .spacing(12)
                .child(
                    Text::new("File:")
                        .font_size(12)
                )
                .child(
                    Text::new("Untitled")
                        .font_size(12)
                )
                .child(Spacer::new())
                .padding(16)
                .padding(8)
        )
        // Text editing area
        .child(
            TextField::new()
                .placeholder("Type your text here...")
                .padding(16)
        )
        .child(Spacer::new())
        // Status bar separator
        .child(
            HStack::new()
                .background(Color::rgb(200, 200, 200))
                .frame(1, 1)
        )
        // Status bar
        .child(
            HStack::new()
                .spacing(16)
                .child(
                    Text::new("Line:")
                        .font_size(11)
                )
                .child(
                    Text::new("1")
                        .font_size(11)
                )
                .child(
                    Text::new("Column:")
                        .font_size(11)
                )
                .child(
                    Text::new("1")
                        .font_size(11)
                )
                .child(Spacer::new())
                .child(
                    Text::new("Ready - New document")
                        .font_size(11)
                )
                .child(Spacer::new())
                .child(
                    Text::new("UTF-8")
                        .font_size(11)
                )
                .padding(12)
        );

    let window = Window::builder()
        .title("Scarlet Notepad")
        .size(950, 680)
        .min_size(550, 450)
        .build()
        .content(ui_content);

    if let Err(e) = app.add_window(window) {
        println!("[notepad] Failed to add window: {}", e);
        return 1;
    }

    println!("[notepad] Running application...");
    app.run();
}
