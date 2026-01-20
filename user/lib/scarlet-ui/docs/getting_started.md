# ScarletUI Getting Started

ScarletUI is a declarative UI framework for Scarlet OS, providing a SwiftUI-like API with efficient buffer-based rendering.

## Quick Start

### Basic Application

```rust
use scarlet_ui::prelude::*;

struct MyApp;

impl App for MyApp {
    type ViewType = MyView;

    fn build(&self) -> Self::ViewType {
        MyView
    }
}

#[derive(View, Clone)]
struct MyView;

impl MyView {
    fn body(&self) -> impl View {
        Text::new("Hello, ScarletUI!")
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = MyApp;
    Application::new(app)?.run()
}
```

## Core Concepts

### Views

Views are immutable descriptions of your UI. They define what to render, not how to render it.

```rust
#[derive(View, Clone)]
struct CounterView {
    count: State<i32>,
}

impl CounterView {
    fn body(&self) -> impl View {
        VStack {
            children: vec![
                Box::new(Text::new(format!("Count: {}", self.count.get()))),
            ],
            spacing: 10.0,
            alignment: Alignment::Center,
        }
    }
}
```

### State Management

State is reactive and shared via Arc. Changes to state trigger re-renders.

```rust
let count = State::new(0);

// Get current value
let value = count.get();

// Update value
count.set(42);

// Update with function
count.update(|c| *c += 1);

// Subscribe to changes
let _id = count.subscribe(Box::new(|| {
    println!("Count changed!");
}));
```

### Built-in Components

#### Text
```rust
Text::new("Hello, World!")
```

#### Rectangle
```rust
Rectangle::new([255, 0, 0, 255])  // RGBA color
```

#### Button
```rust
Button::new("Click me")
    .on_click(|| {
        println!("Clicked!");
    })
```

#### TextField
```rust
let text = State::new(String::from(""));
TextField::new(text.clone())
    .placeholder("Enter text...")
    .width(200.0)
```

#### Slider
```rust
let value = State::new(50.0);
Slider::new(value.clone())
    .range(0.0, 100.0)
    .width(200.0)
```

#### Toggle
```rust
let is_on = State::new(false);
Toggle::new(is_on.clone())
```

#### Spacer
```rust
Spacer::new()  // Fills available space
```

### Layout Containers

#### VStack
Arranges children vertically:
```rust
VStack {
    children: vec![
        Box::new(Text::new("First")),
        Box::new(Text::new("Second")),
        Box::new(Text::new("Third")),
    ],
    spacing: 10.0,
    alignment: Alignment::Center,
}
```

## Common Patterns

### Counter Example

```rust
use scarlet_ui::prelude::*;

struct CounterApp {
    count: State<i32>,
}

impl App for CounterApp {
    type ViewType = CounterView;

    fn build(&self) -> Self::ViewType {
        CounterView {
            count: self.count.clone(),
        }
    }
}

#[derive(View, Clone)]
struct CounterView {
    count: State<i32>,
}

impl CounterView {
    fn body(&self) -> impl View {
        let count = self.count.clone();

        VStack {
            children: vec![
                Box::new(Text::new(format!("Count: {}", self.count.get()))),
                Box::new(
                    Button::new("Increment")
                        .on_click(move || {
                            count.update(|c| *c += 1);
                        })
                ),
            ],
            spacing: 20.0,
            alignment: Alignment::Center,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = CounterApp {
        count: State::new(0),
    };
    Application::new(app)?.run()
}
```

### State Updates

State updates automatically trigger re-renders of subscribed views:

```rust
let text = State::new(String::from("Hello"));

// In a View
TextField::new(text.clone())

// Update elsewhere
text.set(String::from("Goodbye"));
// TextField will re-render automatically
```

## no_std Environment

ScarletUI is designed for Scarlet OS's no_std environment. Important notes:

1. Use `scarlet_std` re-exports:
   ```rust
   extern crate scarlet_std as std;
   ```

2. Float math requires `libm`:
   ```rust
   use libm;
   let result = libm::sqrtf(x);
   ```

3. No `HashSet` - use Vec with O(n) lookup instead
4. No `RwLock` - use Mutex from scarlet_std

## Next Steps

- Read [Architecture](architecture.md) for deeper understanding
- Read [Event Handling](event_handling.md) for interaction patterns
- Read [Layout System](layout_system.md) for layout details
