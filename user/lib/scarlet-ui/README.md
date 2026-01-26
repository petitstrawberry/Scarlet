# ScarletUI 2.0

A declarative UI framework for Scarlet OS, inspired by Flutter and SwiftUI.

## Overview

ScarletUI is a reactive UI framework built for Rust in a `no_std` environment. It provides:

- **Declarative Views**: Describe your UI with composable Views
- **State Management**: Reactive `State<T>` with automatic updates
- **Element System**: Efficient runtime representation with reconciliation
- **Layout Engine**: Constraint-based layout system
- **Event Handling**: Pointer and keyboard event routing
- **Platform Abstraction**: Work with SWS, SDL2, Winit, or custom backends

## Architecture

ScarletUI follows a layered architecture:

```
User Code (Views)
       ↓
Element Tree (Runtime)
       ↓
Render Pipeline (Layout/Paint)
       ↓
Platform Window (SWS/SDL/etc.)
```

### Key Concepts

1. **View**: A factory that creates Elements
2. **Element**: The runtime representation of a View
3. **RenderObject**: Handles layout and rendering
4. **State<T>>**: Reactive state with change notifications
5. **Application**: Main entry point with event loop

## Basic Usage

```rust
use scarlet_ui::prelude::*;

struct CounterApp {
    count: State<i32>,
}

impl View for CounterApp {
    fn create_element(&self) -> Box<dyn Element> {
        // Create element from this view
        Text::new(format!("Count: {}", self.count.get()))
            .create_element()
    }
    // ...
}

impl Application for CounterApp {
    fn body(&self) -> impl View {
        Window::new("Counter", Text::new("Hello"))
            .size(Size::new(400.0, 300.0))
    }
}

fn main() {
    let mut app = CounterApp {
        count: State::new(StateId::new(1), 0),
    };
    app.run();
}
```

## Available Views

### Primitive Views

- **`Text`**: Display text with configurable font size and color
- **`Button`**: Interactive button with click callbacks
- **`Rectangle`**: Filled rectangle with color and corner radius
- **`Spacer`**: Flexible or fixed empty space
- **`Image`**: Display images from various sources

### View Modifiers

- **`Padding`**: Add padding around a view
- **`Frame`**: Constrain a view to a specific size
- **`Background`**: Set background color
- **`SetSize`**: Set minimum/maximum size constraints
- **`AlignmentFrame`**: Control alignment within available space

### Layout Containers

- **`Window`**: Top-level window container

## Examples

See the `examples/` directory for complete examples:

- `counter.rs`: Simple counter application
- `colors.rs`: Color palette demonstration

## State Management

```rust
let state = State::new(StateId::new(1), 0);

// Get current value
let value = state.get();

// Update value
state.set(42);

// Clone for sharing
let state2 = state.clone(); // Both point to same underlying state
```

## Event Handling

ScarletUI provides an event dispatcher that routes events to the appropriate views:

```rust
use scarlet_ui::event::{Event, MouseEvent};

// Events are dispatched through the EventDispatcher
// Views can handle events via the handle_event method
```

## Gesture Recognition

Built-in gesture recognizers for common interactions:

- **`TapGestureRecognizer`**: Detect tap/click gestures
- **`DragGestureRecognizer`**: Detect drag gestures
- **`LongPressGestureRecognizer`**: Detect long press gestures

## Platform Integration

ScarletUI abstracts platform window operations:

```rust
use scarlet_ui::platform::{PlatformWindow, SWSPlatformWindow};

// Create platform window (SWS backend)
let window = SWSPlatformWindow::new(
    "com.example.app",
    "My App",
    Size::new(800.0, 600.0)
)?;
```

## Rendering Pipeline

The rendering pipeline consists of three phases:

1. **Build Phase**: Update element tree based on state changes
2. **Layout Phase**: Calculate sizes and positions
3. **Paint Phase**: Render to buffers
4. **Composite Phase**: Combine buffers for display

## Testing

Run the test suite:

```bash
cargo test
```

## Development Status

### Completed (✅)
- Phase 1: Core Foundations (Geometry, Color, State, View trait)
- Phase 2: Element System (ComponentElement, RenderElement, ElementTree)
- Phase 3: Rendering System (Buffer, Compositor, RenderObject)
- Phase 4: Pipeline & Reconciliation (PipelineOwner, RenderingPipeline)
- Phase 5: Primitive Views (Text, Button, Rectangle, Spacer, Image)
- Phase 6: View Modifiers (Padding, Frame, Background, Size, Alignment)
- Phase 7: Window System (Window View, PlatformWindow abstraction, SWS backend)
- Phase 8: Event Handling (EventDispatcher, Gesture recognizers)
- Phase 9: Integration & Testing (Examples, basic tests)

### Future Work

- Container views (VStack, HStack, ZStack)
- View macros (`vstack!`, `hstack!`, etc.)
- `#[derive(View)]` procedural macro
- ForEach view for dynamic lists
- Advanced gesture recognizers (Pinch, Rotation)
- Animation system
- Focus management
- Accessibility support

## License

MIT License - See LICENSE file for details

## Contributing

Contributions are welcome! Please read our contributing guidelines before submitting PRs.

## Design Documents

See `docs/scarletui/design.md` for detailed architecture documentation.
