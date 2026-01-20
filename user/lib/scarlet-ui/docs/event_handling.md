# ScarletUI Event Handling

ScarletUI implements a three-phase event propagation system inspired by the DOM, enabling sophisticated interaction patterns and gesture composition.

## Event Phases

Events propagate through the UI tree in three distinct phases:

```
1. Capture Phase (Root → Target)
   Root → Container → Container → Target

2. Target Phase
   Target handles event

3. Bubble Phase (Target → Root)
   Target → Container → Container → Root
```

Each phase can be stopped independently.

## Event Types

### MouseEvent

```rust
pub struct MouseEvent {
    pub position: Point,      // Position in window coordinates
    pub buttons: MouseButtons, // Pressed buttons
    pub kind: MouseEventKind,
}

pub enum MouseEventKind {
    Press,                  // Mouse button pressed
    Release,                // Mouse button released
    Move,                   // Mouse moved
    Scroll { delta: Point }, // Mouse wheel scrolled
}
```

### KeyEvent

```rust
pub struct KeyEvent {
    pub key: Key,
    pub pressed: bool,
}

pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Escape,
    // ... more keys
}
```

### Focus Event

```rust
pub enum Event {
    Mouse(MouseEvent),
    Key(KeyEvent),
    Focus(bool),  // Gained (true) or lost (false) focus
}
```

## EventContext

Controls event propagation:

```rust
pub struct EventContext {
    pub phase: EventPhase,
    pub stop_propagation: bool,   // Stop all remaining phases
    pub stop_immediate: bool,     // Stop current phase only
}

pub enum EventPhase {
    Capture,  // Root → Target
    Target,   // At target node
    Bubble,   // Target → Root
}
```

## Handling Events

### Basic Event Handling

```rust
impl RenderNode for MyButton {
    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
        match event {
            Event::Mouse(e) if ctx.phase == EventPhase::Target => {
                match e.kind {
                    MouseEventKind::Press => {
                        println!("Button pressed");
                        self.mark_dirty(DirtyFlags::PAINT);
                    }
                    MouseEventKind::Release => {
                        println!("Button released");
                        self.mark_dirty(DirtyFlags::PAINT);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
```

### Phase-Specific Handling

```rust
fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
    match ctx.phase {
        EventPhase::Capture => {
            // Can intercept event before target
            // Useful for gesture recognition
        }
        EventPhase::Target => {
            // Primary event handling
            self.handle_target_event(event);
        }
        EventPhase::Bubble => {
            // Can see event after target handled it
            // Useful for delegation
        }
    }
}
```

## Hit Testing

Hit testing determines which node receives an event:

```rust
pub enum HitResult {
    Handled(NodeId),  // This node handles it
    Passthrough,      // Continue searching children
    Stop,             // Stop searching, don't handle
}
```

### Hit Test Implementation

```rust
impl RenderNode for MyNode {
    fn hit_test(&self, point: Point) -> HitResult {
        if self.frame.contains(point) {
            HitResult::Handled(self.id)
        } else {
            HitResult::Passthrough
        }
    }
}
```

### Container Hit Testing

```rust
impl RenderNode for VStackRenderNode {
    fn hit_test(&self, point: Point) -> HitResult {
        // Check children first (reverse for z-order)
        for child in self.children.iter().rev() {
            let local_point = point - child.frame().origin;
            match child.hit_test(local_point) {
                HitResult::Handled(id) => return HitResult::Handled(id),
                HitResult::Stop => return HitResult::Stop,
                HitResult::Passthrough => continue,
            }
        }
        HitResult::Passthrough
    }
}
```

## Event Propagation Examples

### Button Click

```rust
// Tree: VStack → Button
// User clicks button

// Phase 1: Capture (VStack → Button)
VStack.handle_event(click, phase: Capture)  // Can intercept
Button.handle_event(click, phase: Capture)  // Can intercept

// Phase 2: Target
Button.handle_event(click, phase: Target)   // Executes callback

// Phase 3: Bubble (Button → VStack)
Button.handle_event(click, phase: Bubble)   // Can see result
VStack.handle_event(click, phase: Bubble)   // Can see result
```

### Stopping Propagation

```rust
fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
    match event {
        Event::Mouse(e) if ctx.phase == EventPhase::Target => {
            if e.kind == MouseEventKind::Press {
                // Stop event from reaching parent
                ctx.stop_propagation = true;
                println!("Handled exclusively");
            }
        }
        _ => {}
    }
}
```

### Stop Immediate

```rust
fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
    match event {
        Event::Mouse(e) if ctx.phase == EventPhase::Capture => {
            if some_condition {
                // Stop only capture phase, allow target/bubble
                ctx.stop_immediate = true;
            }
        }
        _ => {}
    }
}
```

## Common Patterns

### Hover State

```rust
struct InteractionState {
    hovered: bool,
    pressed: bool,
}

fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
    match event {
        Event::Mouse(e) if ctx.phase == EventPhase::Target => {
            match e.kind {
                MouseEventKind::Move => {
                    let was_hovered = self.interaction_state.hovered;
                    self.interaction_state.hovered = self.frame.contains(e.position);

                    if was_hovered != self.interaction_state.hovered {
                        self.mark_dirty(DirtyFlags::PAINT);
                    }
                }
                MouseEventKind::Press => {
                    if self.interaction_state.hovered {
                        self.interaction_state.pressed = true;
                        self.mark_dirty(DirtyFlags::PAINT);
                    }
                }
                MouseEventKind::Release => {
                    self.interaction_state.pressed = false;
                    self.mark_dirty(DirtyFlags::PAINT);
                }
                _ => {}
            }
        }
        _ => {}
    }
}
```

### Click Detection

```rust
fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
    match event {
        Event::Mouse(e) if ctx.phase == EventPhase::Target => {
            match e.kind {
                MouseEventKind::Press => {
                    if self.frame.contains(e.position) {
                        self.interaction_state.pressed = true;
                    }
                }
                MouseEventKind::Release => {
                    if self.interaction_state.pressed && self.frame.contains(e.position) {
                        // Click complete
                        if let Some(ref action) = self.view.action {
                            action();
                        }
                    }
                    self.interaction_state.pressed = false;
                }
                _ => {}
            }
        }
        _ => {}
    }
}
```

### Drag Detection

```rust
struct DragState {
    dragging: bool,
    start_position: Point,
}

fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
    match event {
        Event::Mouse(e) if ctx.phase == EventPhase::Target => {
            match e.kind {
                MouseEventKind::Press => {
                    if self.frame.contains(e.position) {
                        self.drag_state.dragging = true;
                        self.drag_state.start_position = e.position;
                    }
                }
                MouseEventKind::Move => {
                    if self.drag_state.dragging {
                        let delta = e.position - self.drag_state.start_position;
                        self.handle_drag(delta);
                    }
                }
                MouseEventKind::Release => {
                    self.drag_state.dragging = false;
                }
                _ => {}
            }
        }
        _ => {}
    }
}
```

### Focus Management

```rust
fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
    match event {
        Event::Mouse(e) if ctx.phase == EventPhase::Target => {
            if e.kind == MouseEventKind::Press {
                if self.frame.contains(e.position) {
                    self.interaction_state.focused = true;
                } else {
                    self.interaction_state.focused = false;
                }
                self.mark_dirty(DirtyFlags::PAINT);
            }
        }
        Event::Key(key) if ctx.phase == EventPhase::Target => {
            if self.interaction_state.focused {
                self.handle_key_input(key);
            }
        }
        _ => {}
    }
}
```

## Gesture Composition

The three-phase system enables powerful gesture composition:

### Intercept in Capture

```rust
// Container intercepts events before children
impl RenderNode for GestureContainer {
    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
        match ctx.phase {
            EventPhase::Capture => {
                // Recognize swipe gesture
                if let Event::Mouse(e) = event {
                    if e.kind == MouseEventKind::Move {
                        if self.recognize_swipe(e) {
                            ctx.stop_propagation = true;  // Don't pass to children
                            self.handle_swipe();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
```

### Delegate in Bubble

```rust
// Parent sees events after child handles them
impl RenderNode for FormContainer {
    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
        match ctx.phase {
            EventPhase::Bubble => {
                // See if child handled submit
                if let Event::Mouse(e) = event {
                    if e.kind == MouseEventKind::Release {
                        if self.child_submitted {
                            self.validate_form();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
```

## Event Dispatcher

The EventDispatcher orchestrates three-phase propagation:

```rust
impl<'a> EventDispatcher<'a> {
    pub fn dispatch(&mut self, event: &Event) {
        // 1. Find target via hit test
        let target_id = match self.find_target(event) {
            Some(id) => id,
            None => return,
        };

        // 2. Capture phase (root → target)
        self.capture_phase(event, target_id);

        // 3. Target phase
        self.target_phase(event, target_id);

        // 4. Bubble phase (target → root)
        self.bubble_phase(event, target_id);
    }
}
```

## Best Practices

### 1. Always Check Phase

```rust
// ❌ BAD - Handles in all phases
fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
    if let Event::Mouse(e) = event {
        // Handles 3 times!
        self.do_something();
    }
}

// ✅ GOOD - Handles only in target phase
fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
    if let Event::Mouse(e) = event {
        if ctx.phase == EventPhase::Target {
            self.do_something();
        }
    }
}
```

### 2. Mark Dirty on State Changes

```rust
fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
    match event {
        Event::Mouse(e) if ctx.phase == EventPhase::Target => {
            if e.kind == MouseEventKind::Move {
                let was_hovered = self.hovered;
                self.hovered = self.frame.contains(e.position);

                // Only mark if actually changed
                if was_hovered != self.hovered {
                    self.mark_dirty(DirtyFlags::PAINT);
                }
            }
        }
        _ => {}
    }
}
```

### 3. Use Passthrough Correctly

```rust
// Container should passthrough to children
fn hit_test(&self, point: Point) -> HitResult {
    if self.frame.contains(point) {
        HitResult::Passthrough  // Let children handle
    } else {
        HitResult::Passthrough
    }
}

// Leaf node should handle
fn hit_test(&self, point: Point) -> HitResult {
    if self.frame.contains(point) {
        HitResult::Handled(self.id)  // I handle it
    } else {
        HitResult::Passthrough
    }
}
```

### 4. Stop Propagation Sparingly

```rust
// ❌ BAD - Always stops
fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
    ctx.stop_propagation = true;  // Parents never see events
}

// ✅ GOOD - Stops only when necessary
fn handle_event(&mut self, event: &Event, ctx: &mut EventContext) {
    if let Event::Mouse(e) = event {
        if e.kind == MouseEventKind::Press {
            if self.is_exclusive {
                ctx.stop_propagation = true;
            }
        }
    }
}
```

## Advanced Topics

### Custom Event Types

You can extend the Event enum for application-specific events:

```rust
pub enum Event {
    Mouse(MouseEvent),
    Key(KeyEvent),
    Focus(bool),
    Custom(MyCustomEvent),  // Add your own
}
```

### Event Recording

For debugging, you can record events:

```rust
struct EventLogger {
    events: Vec<(Event, NodeId)>,
}

impl EventLogger {
    fn log(&mut self, event: &Event, target: NodeId) {
        self.events.push((event.clone(), target));
    }
}
```

### Event Filtering

Filter events before dispatch:

```rust
impl EventDispatcher<'_> {
    pub fn dispatch_filtered(&mut self, event: &Event) {
        if self.should_filter(event) {
            return;
        }
        self.dispatch(event);
    }
}
```
