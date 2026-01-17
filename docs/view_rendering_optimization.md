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

### Data Structures

```rust
pub struct Application {
    connection: Connection,
    windows: Vec<ManagedWindow>,
    dirty_views: HashSet<ViewId>,  // Only dirty view IDs
}

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
}

struct VStack {
    children: Vec<(ViewBox, Size)>,
    cached_frame: Rect,  // Cached window-relative coordinates
    cached_size: Size,
}
```

### View Layout with Frame Caching

```rust
impl VStack {
    fn layout(&mut self, origin: Point, available: Size) -> Size {
        let mut y = origin.y;

        for (child, size) in &mut self.children {
            let child_origin = Point::new(origin.x, y);
            *size = child.layout(child_origin, /* available */);
            y += size.height;
        }

        // Cache own frame in window coordinates
        self.cached_frame = Rect::new(
            origin.x,
            origin.y,
            total_width,
            total_height
        );
        self.cached_size = Size::new(total_width, total_height);

        self.cached_size
    }

    fn cached_frame(&self) -> Rect {
        self.cached_frame  // Instant access
    }

    fn draw(&self, canvas: &mut Canvas) {
        for (child, _) in &self.children {
            child.draw(canvas);  // Child knows its own frame
        }
    }
}
```

## Complete Flow

### Initialization

```
1. Window creation → surface creation
2. Initial layout:
   window.layout(Point(0, 0), Size(width, height))
     ↓
   Each view caches its frame:
   - VStack: cached_frame = (0, 32, 400, 268)
   - Button: cached_frame = (10, 62, 380, 40)
   - Label:  cached_frame = (10, 32, 380, 20)
```

### Event Loop

```
【Each Frame】

1. Receive event from SWS
   ↓
2. Application::handle_sws_event()
   - Convert to InputEvent
   ↓
3. dispatch_event_to_view()
   ```rust
   fn dispatch_event_to_view(view: &mut dyn View, mut event: Event, frame: Rect) {
       // Find target view using cached frames
       for (child, child_frame) in view.children() {
           if child_frame.contains(event.x(), event.y()) {
               dispatch_event_to_view(child, event, child_frame);
           }
       }

       // Bubble phase
       if view.on_event(&mut event, frame) {
           view.set_needs_draw();  // Add to dirty set
           dirty_set.insert(view.id());
       }
   }
   ```
   ↓
4. Draw phase
   ```rust
   // Iterate dirty set - O(dirty views) not O(all views)!
   for view_id in &self.dirty_views {
       if let Some(view) = self.get_view_mut(*view_id) {
           let frame = view.cached_frame();  // Instant lookup

           canvas.push_clip(frame);
           view.draw(&mut canvas);  // No frame parameter
           canvas.pop_clip();
       }
   }
   self.dirty_views.clear();
   ```
   ↓
5. Commit dirty region
   ```rust
   let dirty_rect = union_all_frames();  // Union of cached frames
   connection.commit_region(surface_id, dirty_rect);
   ```
```

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

```
1. ScrollView scrolls
   ↓
2. Update child view frames
   ```rust
   fn set_scroll_offset_y(&mut self, offset: i32) {
       self.scroll_offset_y = offset;

       // Update child frames with scroll offset
       let mut y = -offset;
       for (child, size) in &mut self.children {
           child.layout_relative(Point::new(0, y));
           y += size.height;
       }

       self.needs_redraw = true;
       dirty_set.insert(self.id());
       // Mark children dirty too
       for child in &self.children {
           dirty_set.insert(child.id());
       }
   }
   ```
```

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

### Phase 1: Core Changes

- [ ] Add `ViewId` trait to `View`
- [ ] Add `cached_frame()` method to `View` trait
- [ ] Add `id()` method to `View` trait
- [ ] Add `dirty_views: HashSet<ViewId>` to `Application`
- [ ] Implement frame caching in `Window`
- [ ] Implement frame caching in `VStack`
- [ ] Implement frame caching in `HStack`
- [ ] Implement frame caching in `ZStack`
- [ ] Implement frame caching in `ScrollView`
- [ ] Implement frame caching in control views (Button, Label, etc.)

### Phase 2: Layout Changes

- [ ] Update `layout()` signature to accept `origin: Point`
- [ ] Store `cached_frame` in all view implementations
- [ ] Update container layouts to pass origin to children
- [ ] Update `Window::layout()` to start at `Point(0, 0)`

### Phase 3: Draw Changes

- [ ] Remove `frame: Rect` parameter from `draw()` signature
- [ ] Update all `draw()` implementations to use `self.cached_frame()`
- [ ] Add canvas clipping in `Application::draw()` phase

### Phase 4: Dirty Set Management

- [ ] Implement `set_needs_draw()` to add to `dirty_views`
- [ ] Replace `needs_draw()` checks with `dirty_views` iteration
- [ ] Update event dispatch to use dirty set
- [ ] Remove `collect_dirty_rects()` traversal
- [ ] Implement dirty region union from cached frames

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

1. **Backward compatibility**: Keep old `draw(canvas, frame)` temporarily during migration
2. **Incremental updates**: Migrate one view type at a time
3. **Testing**: Use existing apps as integration tests
4. **Performance**: Benchmark before/after each major change

## Related Documents

- [Scarlet UI Framework](scarlet_ui_framework.md) - Overall UI architecture
- [SWS IPC Protocol](sws_ipc_protocol.md) - Window server communication
- [Container System](container_system.md) - Layout container details
