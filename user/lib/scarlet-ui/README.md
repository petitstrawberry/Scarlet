# Scarlet UI

A modern, data-first UI framework for Scarlet OS inspired by Druid, Flutter, and AppKit.

## Architecture

ScarletUI follows a **data-first MVC architecture** with unidirectional data flow:

- **Data-first**: All state flows through `DataContext<T>`
- **Unidirectional**: Data flows down, events flow up
- **Phase-separated**: Event → Layout → Draw → Compose → Present
- **Incremental**: Only redraw what changes (dirty tracking)

See [docs/architecture.md](docs/architecture.md) for detailed architecture documentation.

## Core Features

### View System

All UI components implement the `View` trait:

```rust
pub trait View {
    fn id(&self) -> ViewId;
    fn layout(&mut self, ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size;
    fn draw(&self, ctx: &mut PaintCtx, frame: Rect);
    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow;
    fn update(&mut self, ctx: &mut UpdateCtx);
}
```

### Layout Containers

The `child()` method automatically handles `Box::new()` - no need to wrap views!

```rust
use scarlet_ui::{VStack, HStack, ZStack};

// Vertical stack
let vstack = VStack::new()
    .spacing(10)
    .alignment(CrossAxisAlignment::Center)
    .child(Text::new("Title"))
    .child(Text::new("Subtitle"))
    .child(Button::new("Click Me"));

// Horizontal stack
let hstack = HStack::new()
    .spacing(15)
    .alignment(MainAxisAlignment::Center)
    .child(Button::new("OK"))
    .child(Button::new("Cancel"));

// ZStack for overlay
let zstack = ZStack::new()
    .child(Image::new("bg.png"))
    .child(Text::new("Overlay"));
```

#### Using view! macro (SwiftUI-style)

```rust
use scarlet_ui::view;

let ui = view! {
    VStack(spacing: 16) {
        Text("Title")
        HStack(spacing: 10) {
            Button("OK")
            Button("Cancel")
        }
    }
};
```

The `view!` macro:
- Automatically wraps children in `Box` for you
- Supports named parameters: `VStack(spacing: 10)`
- Supports method chaining: `Text("Hello").set_color(Color::BLACK)`
- Handles nested containers naturally

### View Modifiers (SwiftUI-style)

Apply transformations using method chaining:

```rust
let styled = Text::new("Hello")
    .padding(10)
    .frame(200, 50)
    .background(Color::rgb(240, 240, 240))
    .repaint_boundary();
```

Available modifiers:
- `padding(u32)` - Add padding
- `padding_insets(top, right, bottom, left)` - Add different padding per edge
- `frame(width, height)` - Set fixed size
- `frame_constraints(min_w, max_w, min_h, max_h)` - Set size constraints
- `background(Color)` - Set background color
- `repaint_boundary()` - Isolate repaints
- `repaint_boundary_opaque()` - Isolate with opaque hint

### Available Controls

#### Text
```rust
let text = Text::new("Hello, World!")
    .set_font(FontConfig { size: 24, ..Default::default() })
    .set_color(Color::BLACK)
    .set_alignment(TextAlignment::Center);
```

#### Button
```rust
let button = Button::new("Click Me")
    .set_action(Arc::new(|| {
        println!("Clicked!");
    }))
    .set_colors(
        Color::BUTTON_NORMAL,
        Color::BUTTON_HOVER,
        Color::BUTTON_PRESSED
    );
```

#### Image
```rust
let image = Image::with_data(ImageData::new(data, width, height))
    .set_scaling(ImageScaling::Fit);
```

#### TextField
```rust
let field = TextField::new()
    .set_placeholder("Enter text...")
    .set_text(Arc::new("Hello".into()));
```

#### Toggle
```rust
let toggle = Toggle::with_label(true, "Enable feature")
    .set_style(ToggleStyle::Switch);
// or ToggleStyle::Checkbox
```

#### Slider
```rust
let slider = Slider::with_value(0.0, 100.0, 50.0)
    .set_step(1.0)
    .set_label("Volume");
```

### State Management

```rust
use scarlet_ui::{DataContext, Observable};

struct AppState {
    count: i32,
    text: String,
}

let mut data = DataContext::new(AppState {
    count: 0,
    text: String::from("Hello"),
});

// Read
let count = data.get().count;

// Write (auto-invalidation)
data.mutate(|state| {
    state.count += 1;  // Triggers repaint
});

// Observe changes
let observable = data.observe(view_id);
```

## Example Application

```rust
use scarlet_ui::*;
use alloc::sync::Arc;
use alloc::string::String;

struct CounterView {
    id: ViewId,
    count: Arc<std::sync::Mutex<i32>>,
}

impl CounterView {
    fn new() -> Self {
        Self {
            id: ViewId::new(),
            count: Arc::new(std::sync::Mutex::new(0)),
        }
    }
}

impl View for CounterView {
    fn id(&self) -> ViewId { self.id }

    fn layout(&mut self, _ctx: &mut LayoutCtx, constraints: LayoutConstraints) -> Size {
        Size::new(200, 100)
    }

    fn draw(&self, _ctx: &mut PaintCtx, _frame: Rect) {
        // Draw implementation
    }

    fn event(&mut self, ctx: &mut EventCtx, event: &Event) -> ControlFlow {
        match &event.kind {
            EventKind::MouseDown { button, .. } if *button == MouseButton::Left => {
                *self.count.lock().unwrap() += 1;
                ctx.request_paint();
            }
            _ => {}
        }
        ControlFlow::Continue
    }

    fn update(&mut self, _ctx: &mut UpdateCtx) {}
}
```

## Building

Scarlet UI requires:
- Rust nightly with `no_std` support
- `rust-src` component for cross-compilation

Build with:

```bash
cargo build --target riscv64gc-unknown-none-elf
```

## Testing

```bash
cargo test
```

## Design Philosophy

Scarlet UI follows these principles:

1. **Data-first**: State is the single source of truth
2. **Composable**: Build complex UIs from simple components
3. **Efficient**: O(1) operations, incremental updates, repaint boundaries
4. **Predictable**: Unidirectional flow, phase-separated pipeline
5. **Type-safe**: Leverage Rust's type system

## Performance Features

- **O(1) dirty tracking** via HashSet
- **Repaint boundaries** for isolating updates
- **Buffer pooling** with grow-only strategy
- **Constraint-based layout** for responsive UIs
- **Minimal reallocations** with efficient event handling

## License

See repository LICENSE file.
