# ScarletUI Architecture

ScarletUI uses a dual-layer architecture inspired by SwiftUI and CoreAnimation, optimized for buffer-based composition in a no_std environment.

## Core Architecture

### View/RenderNode Duality

```
View (Immutable Blueprint)
  ↓ build()
RenderNode (Mutable Scene Graph)
  ↓ render()
Buffer (Composited Output)
```

#### View Layer
- **Immutable**: Views are pure data structures describing UI
- **Blueprint**: Define what to render, not how
- **Cheap to Clone**: Use Clone trait liberally
- **Trait Objects**: Stored as `dyn View` for polymorphism

```rust
pub trait View: Clone + 'static {
    fn type_id(&self) -> TypeId;
    fn type_name(&self) -> &'static str;
    fn build(&self) -> Box<dyn RenderNode>;
    fn as_any(&self) -> &dyn Any;
}
```

#### RenderNode Layer
- **Mutable**: Holds interaction state, dirty flags
- **Scene Graph**: Forms the actual render tree
- **Buffer Ownership**: Each node owns its buffer
- **Event Handling**: Receives events via hit testing

```rust
pub trait RenderNode {
    // Identity
    fn id(&self) -> NodeId;

    // Tree structure
    fn parent(&self) -> Option<NodeId>;
    fn set_parent(&mut self, parent: NodeId);
    fn children(&self) -> &[Box<dyn RenderNode>];

    // Lifecycle
    fn layout(&mut self, constraints: LayoutConstraints) -> Size;
    fn render(&mut self);
    fn update(&mut self, new_view: &dyn View) -> UpdateResult;

    // Events
    fn hit_test(&self, point: Point) -> HitResult;
    fn handle_event(&mut self, event: &Event, ctx: &mut EventContext);

    // Dirty tracking
    fn mark_dirty(&mut self, flags: DirtyFlags);
    fn is_dirty(&self) -> bool;
}
```

### Node Identification

**Decision: NodeId-based with parent-managed registry**

```rust
pub type NodeId = u64;

// Each node gets unique ID
impl RenderNode for MyNode {
    fn id(&self) -> NodeId {
        self.id  // Assigned at creation
    }
}

// Hit test returns ID, not reference
pub enum HitResult {
    Handled(NodeId),
    Passthrough,
    Stop,
}
```

**Rationale:**
- O(1) access via HashMap/Vec lookup
- No lifetime issues compared to Arc<Weak>
- Serializable for debugging
- Parent owns children, IDs for routing

### Parent Pointers

**Decision: Parent pointers in RenderNode for O(depth) path construction**

```rust
pub trait RenderNode {
    fn parent(&self) -> Option<NodeId>;  // Added
    fn set_parent(&mut self, parent: NodeId);  // Set by container
}
```

**Event Path Building:**
```rust
fn build_path_to_target(&self, target_id: NodeId) -> Vec<NodeId> {
    let mut path = vec![];
    let mut current_id = Some(target_id);

    // Walk up parent pointers
    while let Some(id) = current_id {
        path.push(id);
        current_id = self.get_node(id).and_then(|n| n.parent());
    }

    path.reverse();  // Root → target
    path
}
```

**Rationale:**
- O(depth) instead of O(n) traversal
- Minimal overhead (one Option<NodeId> per node)
- Enables efficient bubble phase

## Event System

### Three-Phase Propagation

```
Capture Phase: Root → Container → Target
Target Phase:  Target handles event
Bubble Phase:  Target → Container → Root
```

Each phase can stop propagation.

```rust
pub enum EventPhase {
    Capture,  // Root → Target
    Target,   // At target
    Bubble,   // Target → Root
}

pub struct EventContext {
    pub phase: EventPhase,
    pub stop_propagation: bool,   // Stop all phases
    pub stop_immediate: bool,     // Stop current phase
}
```

### Event Flow Example

```rust
// Tree structure:
VStack
├── Text
└── Button

// User clicks button:
// 1. Hit test finds Button
// 2. Capture: VStack → Button (VStack can intercept)
// 3. Target: Button handles click, executes callback
// 4. Bubble: Button → VStack (VStack can see result)
```

### EventDispatcher

**Decision: Application owns root, dispatcher borrows**

```rust
pub struct EventDispatcher<'a> {
    root: &'a mut dyn RenderNode,  // Borrow, don't own
}

impl<'a> EventDispatcher<'a> {
    pub fn dispatch(&mut self, event: &Event) {
        // Build path, dispatch three phases
    }
}

// Usage
let mut dispatcher = EventDispatcher {
    root: root_node.as_mut().unwrap().as_mut(),
};
dispatcher.dispatch(&event);
// Borrow ends here
```

**Rationale:**
- Clear ownership: Application owns tree
- Dispatcher is transient, created per event
- No double-borrow issues

## Layout System

### Constraint-Based Layout

SwiftUI-style constraints with tight/loose/unconstrained:

```rust
pub struct LayoutConstraints {
    pub min: Size,
    pub max: Size,
}

impl LayoutConstraints {
    // Fixed size
    pub fn tight(size: Size) -> Self {
        Self { min: size, max: size }
    }

    // Maximum only (content decides)
    pub fn loose(max: Size) -> Self {
        Self { min: Size::ZERO, max }
    }

    // No limits
    pub fn unconstrained() -> Self {
        Self { min: Size::ZERO, max: Size::INFINITE }
    }
}
```

### Two-Pass Layout

**VStack Example:**

1. **Pass 1: Measure**
   ```rust
   let loose_constraints = LayoutConstraints::loose(constraints.max);
   let min_heights: Vec<f32> = self.children.iter()
       .map(|c| c.layout(loose_constraints).height)
       .collect();
   ```

2. **Pass 2: Distribute**
   ```rust
   if min_total <= available_height {
       // Space available: distribute equally
       let per_child = remaining / n as f32;
       for (i, child) in self.children.iter_mut().enumerate() {
           child.layout(LayoutConstraints {
               min: Size::new(min_width, min_heights[i]),
               max: Size::new(max_width, min_heights[i] + per_child),
           });
       }
   }
   ```

## State Management

### Reactive State with Arc

```rust
pub struct State<T: Clone> {
    inner: Arc<StateInner<T>>,
}

struct StateInner<T> {
    value: RwLock<T>,
    version: AtomicU64,
    subscribers: Mutex<Vec<(SubscriptionId, SubscriberCallback)>>,
}
```

**Key Features:**
- **Clone = Cheap**: Cloning shares Arc
- **Version Tracking**: Detect changes without comparing values
- **Reactive**: Subscribers notified on change
- **Thread-Safe**: RwLock for read-heavy workloads

### Subscription Safety

**Decision: SubscriptionId with deferred removal**

```rust
pub struct StateInner<T> {
    subscribers: Mutex<Vec<(SubscriptionId, SubscriberCallback)>>,
    pending_unsubscribe: Mutex<Vec<SubscriptionId>>,  // Deferred
}

impl<T: Clone> StateInner<T> {
    fn notify(&self) {
        // Copy callbacks to avoid holding lock
        let callbacks = self.subscribers.lock()
            .iter()
            .map(|(id, cb)| (*id, cb.clone()))
            .collect::<Vec<_>>();

        // Process outside lock (re-entry safe)
        for (id, callback) in callbacks {
            if !self.pending_unsubscribe.lock().contains(&id) {
                callback();
            }
        }

        // Process pending unsubscriptions
        let mut pending = self.pending_unsubscribe.lock();
        let mut subscribers = self.subscribers.lock();
        subscribers.retain(|(id, _)| !pending.contains(id));
        pending.clear();
    }
}
```

**Rationale:**
- Re-entry safe: Callbacks run without holding lock
- Unsubscribe during callback: Deferred to prevent invalidation
- SubscriptionId: Simple u64, no lifetime issues

## Update/Reconciliation

### Type-Safe Update

**Decision: TypeId + type_name() for robust update**

```rust
pub trait View {
    fn type_id(&self) -> TypeId;  // Fast comparison
    fn type_name(&self) -> &'static str;  // Debug messages
}

pub trait RenderNode {
    fn update(&mut self, new_view: &dyn View) -> UpdateResult {
        // Fast path: TypeId check
        if self.type_id() != new_view.type_id() {
            return UpdateResult::Replaced(new_view.build());
        }

        // Type matches: try property update
        self.try_update(new_view)
            .unwrap_or_else(|| UpdateResult::Replaced(new_view.build()))
    }
}
```

**Rationale:**
- TypeId is collision-proof (unlike strings)
- Fast comparison (single u64)
- type_name() for debugging
- Defense in depth

### Update Result

```rust
pub enum UpdateResult {
    Unchanged,                    // No update needed
    Changed(DirtyFlags),          // Properties updated
    Replaced(Box<dyn RenderNode>), // Type mismatch
}
```

## Rendering

### Buffer-Based Composition

Each node manages its own buffer, parents composite children:

```rust
pub struct Buffer {
    data: Vec<u8>,  // RGBA
    size: Size,
    stride: usize,
}

impl Buffer {
    // Fill rect with color (LOCAL coordinates)
    pub fn fill_rect(&mut self, rect: Rect, color: Color);

    // Blit child buffer into parent (PARENT coordinates)
    pub fn blit_from(&mut self, src: &Buffer, src_rect: Rect);
}
```

**Render Flow:**

```rust
fn render(&mut self) {
    if !self.is_dirty() { return; }

    self.buffer = Some(Buffer::new(self.frame.size));

    // Draw own content
    self.buffer.as_mut().unwrap().fill_rect(self.frame, self.color);

    // Composite children
    for child in &mut self.children {
        child.render();
        if let Some(child_buffer) = child.get_buffer() {
            self.buffer.as_mut().unwrap()
                .blit_from(child_buffer, child.frame());
        }
    }

    self.clear_dirty();
}
```

**Rationale:**
- **Local coordinates**: Each buffer is (0,0) to (width,height)
- **Partial render**: Only dirty nodes re-render
- **Composition**: Parents blit children into their buffer
- **Parallelizable**: Independent subtrees can render in parallel (future)

## Dirty Tracking

```rust
bitflags! {
    pub struct DirtyFlags: u8 {
        const LAYOUT = 1;   // Layout changed
        const PAINT = 2;    // Appearance changed
        const CHILDREN = 4; // Children added/removed
    }
}
```

**Dirty Propagation:**

```rust
// State change triggers callback
state.update(|v| *v += 1);

// Callback marks view dirty
view.mark_dirty(DirtyFlags::PAINT);

// Parent marks children dirty (if needed)
parent.mark_dirty(DirtyFlags::CHILDREN);
```

**Render Loop:**

```rust
loop {
    // 1. Wait for events (with timeout)
    if let Some(event) = bridge.next_event_timeout(Duration::from_millis(16)) {
        dispatcher.dispatch(&event);
    }

    // 2. Check for dirty nodes
    if has_dirty_nodes() {
        // 3. Re-layout dirty roots
        relayout_dirty();

        // 4. Render dirty subtrees
        render_dirty();

        // 5. Present to screen
        bridge.present()?;
    }
}
```

## no_std Considerations

### Import Rules

```rust
// ✅ CORRECT - Use in src/lib.rs
#![no_std]
extern crate scarlet_std as std;

// scarlet_std re-exports core and alloc
use std::sync::atomic::{AtomicU64, Ordering};  // From core
use std::any::TypeId;  // From core
use std::vec::Vec;  // From alloc
use std::boxed::Box;  // From alloc
use std::sync::{Arc, Mutex};  // From scarlet_std

// ❌ WRONG
extern crate std;  // This will fail in no_std
```

### Constraints

1. **scarlet_std re-exports core & alloc** - Use `std::` prefix
2. **Float math via `libm`** - f32 methods require:
   ```toml
   [dependencies]
   libm = "0.2"
   ```
3. **No HashSet** - Use Vec with O(n) lookup
4. **No RwLock in scarlet_std** - Use Mutex instead
5. **Explicit type annotations** - Float literals need `: f32`

## Performance Characteristics

### Time Complexity

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| Hit test | O(depth) | Tree traversal |
| Event dispatch | O(depth) | Path-based routing |
| Layout | O(n) | Two-pass tree traversal |
| Render | O(dirty) | Only dirty subtrees |
| Update | O(1) | TypeId comparison |
| Child lookup | O(n) | Vec-based (no HashSet) |

### Space Complexity

| Component | Space | Notes |
|-----------|-------|-------|
| RenderNode | ~100 bytes | Includes buffer, state |
| State<T> | ~64 bytes + T | Arc overhead |
| Buffer | width × height × 4 | RGBA per pixel |
| NodeId | 8 bytes | u64 |

### Optimization Opportunities

1. **Dirty culling**: Only render dirty subtrees
2. **Buffer pooling**: Reuse allocated buffers
3. **Parallel layout**: Independent subtrees (future)
4. **Incremental render**: Partial buffer updates (future)
