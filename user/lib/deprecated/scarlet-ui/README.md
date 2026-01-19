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
    .set_text("Hello");
```

#### TextField (with data binding)
```rust
let text_data = bindable!(String::from("Hello"));

// TextField automatically updates text_data
let field = TextField::bind(&text_data);

// Other views can observe the same data
let display = Text::bind(&text_data, |s| format!("Text: {}", s));
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

ScarletUI provides reactive state management with `DataContext<T>`:

```rust
use scarlet_ui::DataContext;

struct AppState {
    count: i32,
    text: String,
}

let data = DataContext::new(AppState {
    count: 0,
    text: String::from("Hello"),
});

// Read
let count = data.get().count;

// Modify (auto-invalidation)
data.modify(|state| {
    state.count += 1;  // Triggers repaint
});
```

#### bindable! Macro (SwiftUI-style @State)

Create reactive state variables with minimal syntax:

```rust
use scarlet_ui::bindable;

fn build_ui() {
    let enabled = bindable!(false);
    let volume = bindable!(50.0);

    // Bind UI controls
    let toggle = Toggle::bind(&enabled);
    let slider = Slider::bind(&volume, 0.0, 100.0);
}
```

This is equivalent to SwiftUI's `@State` property wrapper.

### Two-Way Data Binding

UI controls can bind to `DataContext` for automatic synchronization:

```rust
use scarlet_ui::*;

// Create state with bindable! macro
let enabled = bindable!(false);
let volume = bindable!(50.0);

// Bind controls to state
let toggle = Toggle::bind(&enabled);
let slider = Slider::bind(&volume, 0.0, 100.0);

// Display current values
let enabled_text = Text::bind(&enabled, |e| if *e { "ON" } else { "OFF" });
let volume_text = Text::bind(&volume, |v| format!("Volume: {}", v));
```

**Data Flow:**
```
User clicks Toggle
    ↓
Toggle updates DataContext<bool>
    ↓
Text automatically redraws with new value
```

#### Binding with Structs

For complex state, use structs with Lenses:

```rust
use scarlet_ui::*;

struct AudioState {
    volume: f32,
    bass: f32,
    treble: f32,
}

let state = bindable!(AudioState {
    volume: 50.0,
    bass: 30.0,
    treble: 70.0,
});

// Create lenses for each field
let volume_lens = FnLens::new(
    |s: &AudioState| &s.volume,
    |s: &mut AudioState| &mut s.volume
);

// Create child DataContext for volume
let volume_data = state.child(volume_lens);

// Bind to child
let slider = Slider::bind(&volume_data, 0.0, 100.0);
let display = Text::bind(&volume_data, |v| format!("Volume: {}", v));
```

## Example Application

### Counter with Data Binding

```rust
use scarlet_ui::*;

fn build_counter() -> impl View {
    let count = bindable!(0i32);

    VStack::new()
        .spacing(16)
        .alignment(CrossAxisAlignment::Center)
        .child(Text::bind(&count, |c| format!("Count: {}", c)))
        .child(
            Button::new("Increment")
                .set_action(Arc::new(|| {
                    count.modify(|c| *c += 1);
                }))
        )
}
```

### Audio Settings UI

```rust
use scarlet_ui::*;

struct AudioState {
    volume: f32,
    bass: f32,
    treble: f32,
}

fn build_audio_ui() -> impl View {
    let state = bindable!(AudioState {
        volume: 50.0,
        bass: 30.0,
        treble: 70.0,
    });

    VStack::new()
        .spacing(16)
        .child(Text::new("Audio Settings"))
        .child(build_slider(&state, "Volume", |s| &mut s.volume, 0.0, 100.0))
        .child(build_slider(&state, "Bass", |s| &mut s.bass, 0.0, 100.0))
        .child(build_slider(&state, "Treble", |s| &mut s.treble, 0.0, 100.0))
}

fn build_slider<F>(
    state: &DataContext<AudioState>,
    label: &str,
    lens_fn: F,
    min: f32,
    max: f32,
) -> VStack
where
    F: Fn(&mut AudioState) -> &mut f32 + 'static + Copy,
{
    let lens = FnLens::new(
        |s: &AudioState| lens_fn(s),
        |s: &mut AudioState| lens_fn(s)
    );

    let child_data = state.child(lens);

    VStack::new()
        .spacing(8)
        .child(Text::bind(&child_data, |v| format!("{}: {}", label, *v)))
        .child(Slider::bind(&child_data, min, max))
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
