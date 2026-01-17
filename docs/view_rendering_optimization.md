# Scarlet UI View Rendering Optimization Design

## Overview

This document describes the optimization plan for Scarlet UI's rendering system, focusing on reducing the overhead of dirty view tracking and eliminating unnecessary view tree traversals.

## Current Problems

### Problem 1: Full View Tree Traversal Every Frame

```rust
// application.rs:559 - executed every frame
if managed.window.needs_draw() {  // recursively checks ALL views
    ...
} else {
    Self::collect_dirty_rects(&managed.window, full_frame, &mut rects);  // recursively checks ALL views
    ...
}
```

- `needs_draw()` recursively traverses the entire view tree
- `collect_dirty_rects()` also traverses the entire view tree
- **O(N) traversal per frame even when nothing changed**

### Problem 2: Recursive `needs_draw()` Implementation

```rust
// containers.rs:1288
fn needs_draw(&self) -> bool {
    self.needs_redraw || self.child.needs_draw()  // child checks grandchildren...
}
```

For deep view trees, checking dirty status alone costs **O(depth)**.

### Problem 3: Inefficient Partial Drawing

```rust
// application.rs:581
managed.window.draw(&mut canvas, full_frame);  // draws entire window
// ...
self.connection.commit_region(...);  // only optimizes GPU transfer
```

Dirty regions are collected but the entire window is still drawn. Only the commit is optimized.

## Performance Impact

| View Count | Traversals/Frame | Traversals/Second @ 60fps |
|-----------|------------------|---------------------------|
| 100       | 100-200          | 6,000-12,000              |
| 1,000     | 1,000-2,000      | 60,000-120,000            |

While Scarlet currently has small UIs, this will become a bottleneck for complex applications.

## Proposed Solution: Dirty Set + Frame Caching

### Key Insights

1. **Windows have dedicated buffers** - Window movement is handled by SWS, not by updating view coordinates
2. **Frame coordinates are stable** - Within a window buffer, (0,0) is always (0,0) regardless of window position
3. **Layout is infrequent** - Most events just trigger redraws, not relayouts

### Design Principles

1. **Maintain a dirty view set** - Only track views that need redraw
2. **Cache frame coordinates** - Store frame during layout, reuse during draw
3. **Use absolute coordinates** - Window-relative coordinates simplify dirty region calculation
4. **Eliminate frame parameter** - Views know their own frame from layout

## Architecture

### The `#[view]` Procedural Macro

All views use the `#[view]` attribute to automatically generate boilerplate code:

```rust
#[view]
struct Button {
    text: String,
}
```

**Automatically generates:**

1. **View ID field** - Unique identifier for dirty tracking
2. **Children field** - All views can have children
3. **Trait implementations** - `View`, `Into<ViewRef>`
4. **Builder methods** - `child()`, `id()`, `children()`

```rust
// Expanded code
struct Button {
    __view_id: ViewId,           // Auto-generated
    __children: Vec<ViewRef>,    // Auto-generated
    text: String,
}

impl Button {
    // Auto-generated builder method
    fn child(mut self, child: impl Into<ViewRef>) -> Self {
        self.__children.push(child.into());
        self
    }
}

impl View for Button {
    fn id(&self) -> ViewId {
        self.__view_id
    }

    fn children(&self) -> Vec<ViewRef> {
        self.__children.clone()
    }

    // Other View methods...
}

impl Into<ViewRef> for Button {
    fn into(self) -> ViewRef {
        Rc::new(RefCell::new(self))
    }
}
```

**Key design decisions:**
- **All views can have children** - Unified API, no separate "Container" trait
- **Conversion happens at `.child()` call** - Views are created as raw types, converted when added
- **Unique IDs assigned at creation** - `ViewId::new()` called in constructor

### Data Structures

```rust
use std::rc::Rc;
use std::cell::RefCell;
use std::sync::Mutex;

pub struct Application {
    connection: Connection,
    windows: Vec<ViewRef>,  // Windows managed as ViewRef
    dirty_views: HashSet<ViewId>,  // Only dirty view IDs
    view_registry: HashMap<ViewId, ViewRef>,  // ID to ViewRef mapping
}

impl Application {
    // Private constructor
    fn new() -> Self {
        // ...
    }

    // Initialize singleton (call once at startup)
    pub fn initialize() {
        // Creates or returns singleton instance
        Self::instance();
    }

    // Singleton instance
    fn instance() -> &'static Mutex<Self> {
        // ...
    }

    // Called by views to mark themselves as dirty
    pub fn mark_dirty(view_id: ViewId) {
        Self::instance().lock().dirty_views.insert(view_id);
    }
}

type ViewRef = Rc<RefCell<dyn View>>;

trait View {
    // Layout: calculate and cache frame
    fn layout(&mut self, origin: Point, available: Size) -> Size;

    // Retrieve cached frame
    fn cached_frame(&self) -> Rect;

    // Unique identifier for dirty tracking
    fn id(&self) -> ViewId;

    // Draw without frame parameter (view knows its own frame)
    fn draw(&self, canvas: &mut Canvas);

    // Event handling
    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool;

    // Access children
    fn children(&self) -> Vec<ViewRef>;
}

struct VStack {
    __view_id: ViewId,
    __children: Vec<ViewRef>,  // Children managed as ViewRef
    cached_frame: Rect,  // Cached window-relative coordinates
    cached_size: Size,
}
```

### View Layout with Frame Caching

```rust
impl VStack {
    fn layout(&mut self, origin: Point, available: Size) -> Size {
        let mut y = origin.y;
        let mut max_width = 0;

        for child in &self.children {
            let child_origin = Point::new(origin.x, y);
            let size = child.borrow_mut().layout(child_origin, available);
            y += size.height;
            max_width = max_width.max(size.width);
        }

        // Cache own frame in window coordinates
        self.cached_frame = Rect::new(
            origin.x,
            origin.y,
            max_width,
            y - origin.y
        );
        self.cached_size = Size::new(max_width, y - origin.y);

        self.cached_size
    }

    fn cached_frame(&self) -> Rect {
        self.cached_frame  // Instant access
    }

    fn draw(&self, canvas: &mut Canvas) {
        for child in &self.children {
            child.borrow().draw(canvas);  // Child knows its own frame
        }
    }
}
```

## Complete Flow

### View Creation and Registration

```
1. Define views with #[view] attribute
   ↓
2. Create view hierarchy (raw types)
   ↓
3. Add to Application (converts to ViewRef and registers)
   ↓
4. Initial layout (caches frames)
```

#### Step 1: Define Views

```rust
#[view]
struct Button {
    text: String,
    is_pressed: bool,
}

impl Button {
    fn new(text: impl Into<String>) -> Self {
        Self {
            __view_id: ViewId::new(),  // Auto-generated
            __children: Vec::new(),     // Auto-generated
            text: text.into(),
            is_pressed: false,
        }
    }

    fn on_click(mut self, handler: impl Fn() + 'static) -> Self {
        // Store handler
        self
    }
}
```

#### Step 2: Build View Hierarchy

```rust
// All views are raw types at this point
let button = Button::new("Click")
    .on_click(|| println!("clicked"));

let label = Label::new("Counter: 0");

let stack = VStack::new()
    .child(label)          // label → ViewRef converted here
    .child(button);        // button → ViewRef converted here

// stack: VStack (raw type)
// stack.__children: [ViewRef(label), ViewRef(button)]
```

**Key point**: `.child()` converts the argument to `ViewRef` using `Into<ViewRef>`:
```rust
impl VStack {
    fn child(mut self, child: impl Into<ViewRef>) -> Self {
        self.__children.push(child.into());  // Conversion happens here
        self
    }
}
```

#### Step 3: Register with Application

```rust
let window = Window::new("My App", 400, 300)
    .content(stack);  // stack is still raw type

// add_window converts entire tree to ViewRef and registers
Application::add_window(window)?;
```

```rust
impl Application {
    fn add_window(window: Window) -> Result<()> {
        let mut app = Self::instance().lock();

        // Convert root window to ViewRef
        let window_ref: ViewRef = Rc::new(RefCell::new(window));

        // Register entire tree recursively
        Self::register_views_recursive(&window_ref);

        app.windows.push(window_ref);
        Ok(())
    }

    fn register_views_recursive(view: &ViewRef) {
        let mut app = Self::instance().lock();

        // Register this view
        let id = view.borrow().id();
        app.view_registry.insert(id, view.clone());

        // Recursively register all children
        for child in view.borrow().children() {
            Self::register_views_recursive(&child);
        }
    }
}
```

**After registration:**
```rust
Application::instance().lock().view_registry = {
    window_id -> ViewRef(Window),
    stack_id -> ViewRef(VStack),
    label_id -> ViewRef(Label),
    button_id -> ViewRef(Button),
}
```

#### Step 4: Initial Layout

```rust
// add_window calls layout
fn add_window(window: Window) -> Result<()> {
    let mut app = Self::instance().lock();
    let window_ref = Rc::new(RefCell::new(window));
    drop(app);  // Release lock before layout

    Self::register_views_recursive(&window_ref);

    // Initial layout to cache frames
    let size = Size::new(400, 300);
    window_ref.borrow_mut().layout(Point::new(0, 0), size);

    let mut app = Self::instance().lock();
    app.windows.push(window_ref);
    Ok(())
}
```

**After layout:**
```
Window:   cached_frame = (0, 0, 400, 300)
  └─ VStack: cached_frame = (0, 32, 400, 268)
       ├─ Label:  cached_frame = (10, 32, 380, 20)
       └─ Button: cached_frame = (10, 62, 380, 40)
```

### Type Conversion Summary

| Stage | Type | Example |
|-------|------|---------|
| **Definition** | Raw struct | `Button { ... }` |
| **Construction** | Raw type | `Button::new("Click")` → `Button` |
| **`.child()` call** | Parent raw, child → ViewRef | `VStack.child(button)` |
| **`add_window()`** | Entire tree → ViewRef | `Rc<RefCell<Window>>` |
| **In registry** | ViewRef | `HashMap<ViewId, ViewRef>` |

**Key insight**: Views are created and manipulated as raw types. Conversion to `ViewRef` only happens:
1. When adding via `.child()`
2. When registering with Application

This keeps the API ergonomic (no `Rc<RefCell<>>` noise) while enabling efficient dirty tracking.

### Event Loop

```
【Each Frame】

1. Receive event from SWS
   ↓
2. Application::handle_sws_event()
   - Convert to InputEvent
   ↓
3. dispatch_event_to_view()
   - Find target view using cached frames
   - Call view.on_event()
   - If event handled, view pushes its ViewId to dirty_views
   ↓
4. Draw phase
   - Iterate dirty_views (HashSet<ViewId>)
   - For each ViewId, lookup ViewRef from view_registry
   - Draw each view using cached_frame
   - Clear dirty_views
   ↓
5. Commit dirty region
   - Union all cached frames of dirty views
   - Commit to SWS
```

#### Detailed Event Dispatch

```rust
fn dispatch_event_to_view(view: &ViewRef, mut event: Event, frame: Rect) {
    // Find target using cached frames
    let target = view.borrow().children().iter().find(|child| {
        child.borrow().cached_frame().contains(event.x(), event.y())
    });

    // Recurse to children first
    if let Some(child) = target {
        let child_frame = child.borrow().cached_frame();
        Self::dispatch_event_to_view(child, event, child_frame);
    }

    // Handle event on this view
    view.borrow_mut().on_event(&mut event, frame);
    // View calls Application::mark_dirty() internally if needed
}
```

**Note**: `Application` is a singleton. Views can call `Application::mark_dirty(view_id)` directly to request redraw.

#### View-Side Event Handling

```rust
impl Button {
    fn on_event(&mut self, event: &mut Event, frame: Rect) -> bool {
        match event.kind {
            EventKind::MouseDown { button: MouseButton::Left } if frame.contains(event.x(), event.y()) => {
                self.is_pressed = true;
                Application::mark_dirty(self.__view_id);  // Request redraw
                true
            }
            EventKind::MouseUp { button: MouseButton::Left } => {
                if self.is_pressed {
                    self.is_pressed = false;
                    Application::mark_dirty(self.__view_id);  // Request redraw
                    true
                }
            }
            _ => false,
        }
    }
}
```

**Key point**: Views call `Application::mark_dirty(self.__view_id)` whenever their state changes and they need redraw.

#### Draw Phase

```rust
fn draw_frame(&mut self) {
    for window in &self.windows {
        let surface = self.connection.surface_mut(window.borrow().surface_id());
        let mut canvas = Canvas::new(surface.buffer_mut(), width, height);

        // Iterate dirty views - O(dirty views), not O(all views)!
        for view_id in &self.dirty_views {
            // Lookup ViewRef from registry - O(1)
            if let Some(view) = self.view_registry.get(view_id) {
                let frame = view.borrow().cached_frame();  // Cached frame
                canvas.push_clip(frame);
                view.borrow().draw(&mut canvas);
                canvas.pop_clip();
            }
        }

        self.dirty_views.clear();  // Clear for next frame
    }
}
```

**Key optimization**:
- **No tree traversal** - Uses `view_registry` HashMap for O(1) lookup
- **O(dirty views)** - Only iterates views that actually changed
- **Cached frames** - No frame recalculation needed

### Layout Changes

```
1. Configure event from SWS (window resize)
   ↓
2. on_surface_configure(new_width, new_height)
   ```rust
   let size = Size::new(new_width, new_height);
   self.window.layout(Point::new(0, 0), size);  // Recalculate all frames
   self.window.set_needs_draw();  // Mark entire window dirty
   ```
```

### Scroll/State Changes

**TODO**: To be designed

## Coordinate System

```
┌─────────────────────────────────────────────────────┐
│ SWS (Window Server)                                  │
│  ┌─────────────┐  ┌─────────────┐                   │
│  │ Window A    │  │ Window B    │                   │
│  │ Buffer      │  │ Buffer      │                   │
│  │ (0,0,w,h)   │  │ (0,0,w,h)   │                   │
│  │             │  │             │                   │
│  │ [View]      │  │ [View]      │                   │
│  │ frames in   │  │ frames in   │                   │
│  │ window      │  │ window      │                   │
│  │ coords      │  │ coords      │                   │
│  └─────────────┘  └─────────────┘                   │
│       ↑ Window move doesn't affect frame coords      │
└─────────────────────────────────────────────────────┘
```

**Key Point**: Each window has its own buffer. Window movement is handled by SWS. View frames are coordinates within the window buffer, stable across window movements.

## Implementation Checklist

### Phase 1: ViewRef Migration

- [ ] Add `ViewId` type with auto-increment
- [ ] Add `type ViewRef = Rc<RefCell<dyn View>>`
- [ ] Implement Application as singleton with `instance()` method
- [ ] Add `view_registry: HashMap<ViewId, ViewRef>` to `Application`
- [ ] Add `dirty_views: HashSet<ViewId>` to `Application`
- [ ] Add `mark_dirty(view_id)` static method to Application
- [ ] Add `id()` method to `View` trait
- [ ] Add `children()` method to `View` trait
- [ ] Implement `register_views_recursive()` in `Application` (uses instance())

### Phase 2: Frame Caching

- [ ] Add `cached_frame()` method to `View` trait
- [ ] Update `layout()` signature to accept `origin: Point`
- [ ] Store `cached_frame` in all view implementations
- [ ] Implement frame caching in `Window`
- [ ] Implement frame caching in `VStack`
- [ ] Implement frame caching in `HStack`
- [ ] Implement frame caching in `ZStack`
- [ ] Implement frame caching in `ScrollView`
- [ ] Implement frame caching in control views (Button, Label, etc.)

### Phase 3: Draw Changes

- [ ] Remove `frame: Rect` parameter from `draw()` signature
- [ ] Update all `draw()` implementations to use `self.cached_frame()`
- [ ] Add canvas clipping in `Application::draw()` phase

### Phase 4: Dirty Management

- [ ] Implement Application as singleton with `instance()` method
- [ ] Add `mark_dirty(view_id)` static method to Application
- [ ] Add `dirty_views: HashSet<ViewId>` to Application
- [ ] Update View::on_event() implementations to call `Application::mark_dirty()`
- [ ] Update draw phase to iterate dirty_views and lookup ViewRef from view_registry
- [ ] Implement canvas clipping in draw phase

### Phase 5: Testing

- [ ] Test with existing applications
- [ ] Test window resize (frame recalculation)
- [ ] Test scrolling (child frame updates)
- [ ] Benchmark view tree traversal performance
- [ ] Verify dirty region correctness

## Expected Performance Improvement

### Before

```
Frame with 1 view dirty:
  - needs_draw() traversal: 1,000 views
  - collect_dirty_rects(): 1,000 views
  - Total: 2,000 view visits
```

### After

```
Frame with 1 view dirty:
  - dirty_views iteration: 1 view
  - Total: 1 view visit

Improvement: 2000x reduction for sparse updates
```

## Migration Strategy

### Application Singleton Migration

**Before** (current code):
```rust
fn main() {
    let mut app = Application::new();

    let window = Window::new("My App", 400, 300);
    app.add_window(window).unwrap();

    app.run();
}
```

**After** (with singleton):
```rust
fn main() {
    // Initialize singleton (first call creates instance)
    Application::initialize();

    let window = Window::new("My App", 400, 300);
    Application::add_window(window).unwrap();

    Application::run();
}
```

**Migration steps**:
1. Add `initialize()` method that creates singleton instance
2. Change `Application::new()` to `Application::initialize()` in main()
3. Update `app.add_window()` to `Application::add_window()`
4. Update `app.run()` to `Application::run()`
5. Make `new()` private

### View Migration

1. **Backward compatibility**: Keep old `draw(canvas, frame)` temporarily during migration
2. **Incremental updates**: Migrate one view type at a time
3. **#[view] macro**: Add `#[view]` attribute to each view struct
4. **Testing**: Use existing apps as integration tests
5. **Performance**: Benchmark before/after each major change

## Related Documents

- [Scarlet UI Framework](scarlet_ui_framework.md) - Overall UI architecture
- [SWS IPC Protocol](sws_ipc_protocol.md) - Window server communication
- [Container System](container_system.md) - Layout container details
