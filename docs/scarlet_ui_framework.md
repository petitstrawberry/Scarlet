# ScarletUI Framework Design

## Overview

ScarletUI is a high-level UI toolkit for Scarlet OS, designed to provide a modern, declarative-style API similar to AppKit/GTK. The framework handles the event loop internally, so application developers focus only on defining views and responding to user interactions.

## Design Goals

1. **No manual event loop** - Application developers never call `poll_event()` or manage the event loop
2. **Everything is a View** - Window is also a View (the root view with decorations)
3. **View hierarchy** - Compose UIs from nested view components
4. **Automatic layout** - Framework computes layout based on view constraints
5. **Automatic redraw** - Framework redraws only when state changes
6. **Event routing** - Framework routes events to the correct view based on hit-testing

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Application Code                        │
│  - Define view hierarchy                                    │
│  - Set up callbacks for user interactions                   │
│  - Call Application::run() once                             │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                      ScarletUI Framework                    │
│                                                             │
│  ┌───────────────────────────────────────────────────────┐  │
│  │                    Application                        │  │
│  │  - Manages root views (Windows)                       │  │
│  │  - Owns the event loop                                │  │
│  │  - Dispatches events to view hierarchy                │  │
│  └───────────────────────────────────────────────────────┘  │
│                              │                              │
│                              ▼                              │
│  ┌───────────────────────────────────────────────────────┐  │
│  │                   View Hierarchy                      │  │
│  │                                                       │  │
│  │   Window (View)  ─── Root view with decorations       │  │
│  │       │                                               │  │
│  │       ├── TitleBar (View) ── managed internally       │  │
│  │       │                                               │  │
│  │       └── ContentView (user's view hierarchy)         │  │
│  │               │                                       │  │
│  │               ├── VStack (View)                       │  │
│  │               │     ├── Label (View)                  │  │
│  │               │     └── Button (View)                 │  │
│  │               └── ...                                 │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                             │
│  ┌───────────────────────────────────────────────────────┐  │
│  │              Internal Event Loop                      │  │
│  │  1. Dispatch socket events                            │  │
│  │  2. Convert to UI events                              │  │
│  │  3. Route to views (hit-test through hierarchy)       │  │
│  │  4. Call view.handle_event()                          │  │
│  │  5. Re-layout if needed                               │  │
│  │  6. Redraw changed views                              │  │
│  │  7. Commit to SWS                                     │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                      sws-client                             │
│  - Connection to SWS compositor                             │
│  - Surface/SharedMemory management                          │
│  - Low-level frame I/O                                      │
└─────────────────────────────────────────────────────────────┘
```

## Core Concept: Everything is a View

The fundamental design principle is that **everything is a View**. This includes:

| Component | Description |
|-----------|-------------|
| `Window` | Root view with decorations (title bar, border) |
| `VStack` | Container that arranges children vertically |
| `HStack` | Container that arranges children horizontally |
| `Label` | Displays text |
| `Button` | Clickable element with callback |
| ... | All UI elements |

Window is simply a special View that:
- Has a title bar and decorations
- Connects to a SWS surface
- Acts as the root of a view tree

## API Design

### View Trait

All UI components implement the `View` trait:

```rust
pub trait View {
    /// Calculate desired size given available space
    fn layout(&mut self, available: Size) -> Size;
    
    /// Draw the view within the allocated frame
    fn draw(&self, canvas: &mut Canvas, frame: Rect);
    
    /// Handle an input event, return true if consumed
    fn handle_event(&mut self, event: &Event, frame: Rect) -> bool {
        false
    }
    
    /// Get child views for hit-testing (optional)
    fn children(&self) -> &[ViewBox] {
        &[]
    }
}
```

### Window as a View

```rust
pub struct Window {
    title: String,
    content: Option<ViewBox>,
    surface_id: u32,
    // ... internal state
}

impl View for Window {
    fn layout(&mut self, available: Size) -> Size {
        // Layout title bar + content
    }
    
    fn draw(&self, canvas: &mut Canvas, frame: Rect) {
        // Draw decorations + content
    }
    
    fn handle_event(&mut self, event: &Event, frame: Rect) -> bool {
        // Handle close button, then delegate to content
    }
}
```

### Application Lifecycle

```rust
fn main() {
    let app = Application::new().expect("Failed to connect");
    
    // Create a window (which is a View)
    let window = Window::new("My App", 400, 300)
        .content(
            VStack::new()
                .child(Label::new("Hello, World!"))
                .child(Button::new("Click Me", || {
                    // Handle click
                }))
        );
    
    // Add window to application
    app.add_window(window);
    
    // Run - framework takes over, never returns
    app.run();
}
```

### Container Views

| View | Description |
|------|-------------|
| `VStack` | Arranges children vertically |
| `HStack` | Arranges children horizontally |
| `ZStack` | Overlays children (last on top) |
| `Padding` | Adds space around a child |
| `Center` | Centers a child in available space |

### Control Views

| View | Description |
|------|-------------|
| `Label` | Display text |
| `Button` | Clickable button with callback |
| `Spacer` | Flexible space filler |
| `RectView` | Colored rectangle |

## Event Flow

### Event Propagation Model

ScarletUI uses a **two-phase event propagation model** inspired by the DOM event model and Cocoa's Responder Chain:

```
┌─────────────────────────────────────────────────────────────┐
│                    Event Propagation                        │
│                                                             │
│   Phase 1: CAPTURE (トンネリング)                            │
│   ─────────────────────────────                             │
│   Event travels DOWN from root to target                    │
│                                                             │
│       Window ──────────┐                                    │
│          │             │ capture                            │
│          ▼             │                                    │
│       VStack           │                                    │
│          │             │                                    │
│          ▼             ▼                                    │
│       Button ◄──── [Target]                                 │
│          │             │                                    │
│          │             │ bubble                             │
│   Phase 2: BUBBLE (バブリング)                               │
│   ────────────────────────                                  │
│   Event travels UP from target to root                      │
│                                                             │
│       Window ◄─────────┘                                    │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

### Phase Details

| Phase | Direction | Purpose | Can Stop? |
|-------|-----------|---------|-----------|
| **Capture** | Root → Target | Intercept events before reaching target | Yes |
| **Target** | At target | Primary event handling | Yes |
| **Bubble** | Target → Root | Let ancestors react to events | Yes |

### Event Trait

```rust
pub struct Event {
    pub kind: EventKind,
    pub position: Option<Point>,  // For pointer events
    propagation: PropagationState,
}

#[derive(Clone, Copy)]
pub enum EventKind {
    MouseMove,
    MouseDown { button: u8 },
    MouseUp { button: u8 },
    MouseEnter,
    MouseLeave,
    KeyDown { code: u16 },
    KeyUp { code: u16 },
    // ...
}

impl Event {
    /// Stop propagation - event won't continue to next phase/view
    pub fn stop_propagation(&mut self) {
        self.propagation = PropagationState::Stopped;
    }
    
    /// Check if propagation was stopped
    pub fn is_stopped(&self) -> bool {
        self.propagation == PropagationState::Stopped
    }
}
```

### View Event Methods

```rust
pub trait View {
    // ... layout, draw ...
    
    /// Called during capture phase (root → target)
    /// Return true to stop propagation
    fn on_event_capture(&mut self, event: &mut Event, frame: Rect) -> bool {
        false  // Default: don't intercept
    }
    
    /// Called during bubble phase (target → root)
    /// Return true to stop propagation
    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        false  // Default: don't handle
    }
}
```

### Hit Testing

Before event propagation, the framework determines which view is the **target**:

```rust
impl Application {
    fn hit_test(&self, view: &dyn View, point: Point, frame: Rect) -> Option<&dyn View> {
        // Check if point is within this view's frame
        if !frame.contains(point) {
            return None;
        }
        
        // Check children (in reverse order - top-most first)
        for (child, child_frame) in view.children().iter().rev() {
            if let Some(target) = self.hit_test(child, point, child_frame) {
                return Some(target);
            }
        }
        
        // No child matched, this view is the target
        Some(view)
    }
}
```

### Propagation Algorithm

```rust
impl Application {
    fn dispatch_event(&mut self, event: &mut Event, window: &mut Window) {
        let point = event.position.unwrap_or(Point::ZERO);
        let frame = window.frame();
        
        // 1. Build path from root to target via hit-testing
        let path = self.build_event_path(window, point, frame);
        
        // 2. CAPTURE PHASE: Root → Target
        for (view, view_frame) in path.iter() {
            if view.on_event_capture(event, *view_frame) {
                return; // Propagation stopped
            }
            if event.is_stopped() {
                return;
            }
        }
        
        // 3. BUBBLE PHASE: Target → Root
        for (view, view_frame) in path.iter().rev() {
            if view.on_event(event, *view_frame) {
                return; // Propagation stopped
            }
            if event.is_stopped() {
                return;
            }
        }
    }
    
    fn build_event_path(&self, root: &dyn View, point: Point, frame: Rect) -> Vec<(&dyn View, Rect)> {
        let mut path = Vec::new();
        self.build_path_recursive(root, point, frame, &mut path);
        path
    }
}
```

### Common Event Patterns

#### 1. Button Click Handling

```rust
impl View for Button {
    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseDown { button: 0 } => {
                self.is_pressed = true;
                true  // Consume event
            }
            EventKind::MouseUp { button: 0 } => {
                if self.is_pressed {
                    self.is_pressed = false;
                    (self.on_click)();  // Invoke callback
                    true
                } else {
                    false
                }
            }
            _ => false
        }
    }
}
```

#### 2. Parent Intercepting Events (Capture Phase)

```rust
impl View for ScrollView {
    fn on_event_capture(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseMove if self.is_dragging => {
                // Intercept mouse moves while scrolling
                self.update_scroll(event.position);
                true  // Don't let children see this
            }
            _ => false
        }
    }
}
```

#### 3. Event Bubbling for Delegation

```rust
impl View for ListView {
    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseDown { .. } => {
                // Child item was clicked, bubble reached us
                // We can handle selection logic here
                if let Some(pos) = event.position {
                    self.select_item_at(pos);
                }
                true
            }
            _ => false
        }
    }
}
```

### Synthetic Events

The framework generates synthetic events for common patterns:

| Event | Generated When |
|-------|---------------|
| `MouseEnter` | Pointer enters view bounds |
| `MouseLeave` | Pointer leaves view bounds |
| `Click` | MouseDown + MouseUp on same view |
| `Focus` | View gains keyboard focus |
| `Blur` | View loses keyboard focus |

### Focus and Keyboard Events

Keyboard events use a **focus chain** instead of hit-testing:

```rust
impl Application {
    fn dispatch_keyboard_event(&mut self, event: &mut Event) {
        // Keyboard events go to focused view, then bubble up
        if let Some(focused) = self.focused_view {
            let path = self.build_focus_path(focused);
            
            // Bubble from focused view to root
            for (view, frame) in path.iter().rev() {
                if view.on_event(event, *frame) {
                    return;
                }
            }
        }
    }
}
```

### Event Lifecycle Summary

```
1. Input received from SWS
        │
        ▼
2. Convert to Event
        │
        ▼
3. Hit-test to find target view
        │
        ▼
4. Build path: [Window, VStack, Button, ...]
        │
        ▼
5. CAPTURE: Window.on_event_capture()
        │         VStack.on_event_capture()
        │         Button.on_event_capture()
        ▼
6. BUBBLE:  Button.on_event()  ← Target
        │         VStack.on_event()
        │         Window.on_event()
        ▼
7. If any view returned true or called stop_propagation(), stop
        │
        ▼
8. Mark dirty views, schedule redraw
```

### Comparison with Other Frameworks

| Framework | Model | Notes |
|-----------|-------|-------|
| **DOM (Web)** | Capture + Bubble | ScarletUI follows this closely |
| **Cocoa/AppKit** | Responder Chain | Similar to bubble phase only |
| **Qt** | Parent delegation | Event filters + propagation |
| **Flutter** | GestureArena | For gesture disambiguation |
| **SwiftUI** | Declarative handlers | No explicit propagation |

ScarletUI combines the best aspects:
- **Capture phase** from DOM for interception
- **Bubble phase** from DOM/Cocoa for delegation
- **Simple API** like SwiftUI (just implement `on_event`)

## Internal Event Loop (pseudo-code)

```rust
impl Application {
    pub fn run(&mut self) -> ! {
        loop {
            // 1. Dispatch socket I/O
            let _ = self.connection.dispatch();
            
            // 2. Drain all pending events in one shot
            let events = self.connection.drain_events();

            // 3. Convert + dispatch events through the Window(root view)
            for sws_event in events {
                if let Some(event) = self.convert_event(sws_event) {
                    // NOTE: window targeting is still evolving; the important part is
                    // that propagation happens inside the view tree.
                    self.dispatch_to_root_windows(event);
                }
            }

            // 4. Handle lifecycle requests originating from UI (e.g. close/move)
            self.flush_close_requests_to_sws();
            self.flush_move_requests_to_sws();

            // 5. Layout + draw (draw only when needed)
            let mut did_draw = false;
            for window in &mut self.windows {
                let size = Size::new(window.width(), window.height());
                window.layout(size);

                if window.needs_draw() {
                    let mut canvas = window.canvas();
                    let frame = window.frame();
                    window.draw(&mut canvas, frame);
                    window.commit();
                    did_draw = true;
                }
            }

            // 6. Avoid a busy loop when idle
            if !did_draw {
                sleep_ms(1);
            }
        }
    }
}
```

## View Tree Structure

```
Application
    │
    ├── Window (View) ─────────────────────────┐
    │       │                                   │
    │       ├── [TitleBar] (internal View)      │  Window decorations
    │       │       └── CloseButton             │  (managed by Window)
    │       │                                   │
    │       └── ContentArea ───────────────────┘
    │               │
    │               └── User's View Hierarchy
    │                       │
    │                       └── VStack
    │                             ├── Label
    │                             └── Button
    │
    └── Window (View) ... (multiple windows supported)
```

## Module Structure

```
scarlet-ui/
├── lib.rs              # Public API, Application struct
├── application.rs      # Application implementation (event loop)
├── view/
│   ├── mod.rs          # View module exports
│   ├── traits.rs       # View trait, Size, ViewBox
│   ├── window.rs       # Window view (root view with decorations)
│   ├── containers.rs   # VStack, HStack, ZStack, Padding, Center
│   └── controls.rs     # Label, Button, Spacer, RectView
├── graphics.rs         # Canvas, Rect, Point, drawing primitives
├── color.rs            # Color type
└── event.rs            # Event enum
```

## Example Application

```rust
#![no_std]
#![no_main]

extern crate scarlet_std;
use scarlet_ui::{Application, Window, VStack, HStack, Label, Button, Color, Padding};

#[no_mangle]
pub extern "C" fn main() -> ! {
    let mut app = Application::new().expect("Failed to connect to SWS");
    
    // Build the view hierarchy
    let content = Padding::new(
        VStack::new()
            .spacing(16)
            .child(Label::new("Welcome to ScarletUI").color(Color::WHITE))
            .child(
                HStack::new()
                    .spacing(8)
                    .child(Button::new("OK", || { /* handle OK */ }))
                    .child(Button::new("Cancel", || { /* handle Cancel */ }))
            )
    ).all(20);
    
    // Window is just the root View
    let window = Window::new("Demo", 400, 300)
        .background(Color::DARK_GRAY)
        .content(content);
    
    app.add_window(window);
    
    app.run() // Framework takes over - never returns
}
```

## Future: Declarative DSL (Macro-based)

In the future, a proc_macro could enable SwiftUI-style syntax:

```rust
// Future syntax (not yet implemented)
view! {
    Window("Demo", 400, 300) {
        VStack {
            Label("Hello")
            Button("Click") { handle_click() }
        }
    }
}
```

For now, the builder pattern provides a similar experience without macros.
