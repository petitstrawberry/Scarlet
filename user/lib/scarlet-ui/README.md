# Scarlet UI

A modern UI toolkit for Scarlet OS with SwiftUI-inspired APIs.

## Features

### Core View System

- **Declarative view composition**: Build UIs by composing views hierarchically
- **Automatic layout**: Views handle their own layout within constraints
- **Event handling**: Two-phase event system (capture and bubble)
- **Window management**: Built-in window decorations with modern design

### Layout Containers

- **VStack**: Vertical stack layout
- **HStack**: Horizontal stack layout
- **ZStack**: Overlay layout (z-order)
- **Padding**: Add padding around views
- **Center**: Center views within available space
- **Spacer**: Flexible spacing in stacks

### Control Widgets

#### Label
Display text with customizable color and font size:

```rust
Label::new("Hello, World!")
    .color(Color::BLACK)
    .font_size(24)
```

#### Button
Interactive button with click handler:

```rust
Button::new("Click Me", || {
    println!("Button clicked!");
})
.background(Color::BLUE)
.text_color(Color::WHITE)
```

#### TextField
Text input control with placeholder:

```rust
TextField::new("Enter text...")
    .text_color(Color::BLACK)
    .background(Color::WHITE)
```

#### CheckBox
Boolean toggle with label:

```rust
CheckBox::new("Enable feature", true)
    .on_toggle(|checked| {
        println!("Checked: {}", checked);
    })
```

#### Slider
Value selection with range:

```rust
Slider::new(0.5, 0.0, 1.0)
    .on_change(|value| {
        println!("Value: {}", value);
    })
```

#### ProgressBar
Progress indicator:

```rust
ProgressBar::new(0.75)
    .fill_color(Color::BLUE)
    .height(20)
```

#### Toggle
Switch-style boolean control:

```rust
Toggle::new(true)
    .on_toggle(|enabled| {
        println!("Enabled: {}", enabled);
    })
```

### View Modifiers

Apply styles using method chaining:

```rust
view
    .corner_radius(8)
    .border(2, Color::GRAY)
    .background_color(Color::WHITE)
```

Available modifiers:
- `corner_radius(radius)` - Add rounded corners
- `border(width, color)` - Add border
- `background_color(color)` - Set background color

### Modern Window Design

Windows feature:
- Gradient title bar
- Modern close button with hover effects
- Customizable background
- Size constraints support

```rust
Window::new("My App", 600, 400)
    .min_size(400, 300)
    .max_size(1024, 768)
    .background(Color::WHITE)
    .content(/* your views */)
```

## Example Application

```rust
#![no_std]
#![no_main]

extern crate scarlet_std as std;

use scarlet_ui::{
    Application, Window, VStack, HStack, Label, Button, 
    CheckBox, Slider, Padding, Color, ViewModifier,
};

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    let mut app = Application::new().expect("Failed to connect");
    
    let window = Window::new("Demo", 600, 400)
        .background(Color::rgb(245, 245, 250))
        .content(
            Padding::new(
                VStack::new()
                    .spacing(16)
                    .child(Label::new("Welcome!").font_size(32))
                    .child(CheckBox::new("Enable feature", true))
                    .child(Slider::new(0.5, 0.0, 1.0))
                    .child(HStack::new()
                        .child(Button::new("OK", || println!("OK")))
                        .child(Button::new("Cancel", || println!("Cancel")))
                    )
            ).all(20)
        );
    
    app.add_window(window).unwrap();
    app.run(); // Never returns
}
```

## Design Philosophy

Scarlet UI is inspired by SwiftUI and follows these principles:

1. **Declarative**: Describe what you want, not how to build it
2. **Composable**: Build complex UIs from simple components
3. **Type-safe**: Leverage Rust's type system for correctness
4. **Efficient**: Minimal allocations, `no_std` compatible

## Building

Scarlet UI requires:
- Rust nightly with `no_std` support
- `rust-src` component for cross-compilation
- Vector font file at `/fonts/Mplus1-Regular.ttf` in the system

Build with cargo-make:

```bash
cargo make build-userlib-debug-riscv64
```

## Testing

Run tests with:

```bash
cargo make test-riscv64  # For RISC-V
cargo make test-aarch64  # For AArch64
```

## License

See repository LICENSE file.
