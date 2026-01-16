# ScrollView Usage Examples

This document demonstrates how to use the ScrollView container in ScarletUI.

## Basic Vertical Scrolling

```rust
use scarlet_ui::{ScrollView, VStack, Label};

// Create a scrollable list of items
let scroll_view = ScrollView::new(
    VStack::new()
        .spacing(8)
        .child(Label::new("Item 1"))
        .child(Label::new("Item 2"))
        .child(Label::new("Item 3"))
        // ... many more items
);
```

## Customizing Scrollbar Appearance

```rust
use scarlet_ui::{ScrollView, Color, VStack};

ScrollView::new(VStack::new())
    .scrollbar_width(16)                      // Wider scrollbar
    .scrollbar_color(Color::rgb(100, 150, 200))  // Blue thumb
    .scrollbar_track_color(Color::rgb(40, 40, 40))  // Darker track
    .shows_vertical_scrollbar(true);
```

## Horizontal Scrolling

```rust
use scarlet_ui::{ScrollView, HStack};

ScrollView::new(
    HStack::new()
        .spacing(16)
        // ... many horizontal items
)
.shows_horizontal_scrollbar(true)
.shows_vertical_scrollbar(false);
```

## Bidirectional Scrolling

```rust
use scarlet_ui::{ScrollView, ZStack};

ScrollView::new(content)
    .shows_vertical_scrollbar(true)
    .shows_horizontal_scrollbar(true);
```

## Mouse Wheel Scrolling

The ScrollView automatically handles mouse wheel events. The default scroll speed is 30 pixels per wheel tick, but you can customize it:

```rust
ScrollView::new(content)
    .wheel_scroll_speed(50);  // Faster scrolling
```

## Programmatic Scrolling

```rust
let mut scroll_view = ScrollView::new(content);

// Get current scroll position
let y_offset = scroll_view.scroll_offset_y();
let x_offset = scroll_view.scroll_offset_x();

// Scroll to specific position
scroll_view.set_scroll_offset_y(100);
scroll_view.set_scroll_offset_x(50);

// Scroll by relative amount
scroll_view.scroll_by_y(-20);  // Scroll up 20 pixels
scroll_view.scroll_by_x(10);   // Scroll right 10 pixels
```

## Features

1. **Automatic scrollbars**: Scrollbars appear automatically when content exceeds the visible area
2. **Mouse wheel support**: Use the mouse wheel to scroll vertically or horizontally
3. **Drag-to-scroll**: Click and drag the scrollbar thumb to scroll
4. **Click-to-jump**: Click anywhere on the scrollbar track to jump to that position
5. **Proper event forwarding**: Child views receive mouse events with coordinates adjusted for scroll offset
6. **Composable**: Works with any View type as content

## Implementation Details

### Event Handling

The ScrollView intercepts these events:
- `MouseWheel`: Scrolls the content (stops propagation when handled)
- `MouseDown`: Handles scrollbar thumb and track clicks
- `MouseMove`: Handles drag-to-scroll on scrollbar thumbs
- `MouseUp`: Ends drag operations

All other events are forwarded to child views with adjusted coordinates.

### Layout

The ScrollView:
1. Measures its child with unconstrained space
2. Takes all available space (flex_factor = 1)
3. Shows scrollbars when content exceeds visible area
4. Reserves space for scrollbars when they're visible

### Known Limitations

- Clipping is not yet implemented in Canvas, so content may draw outside the ScrollView bounds
- This will be fixed once Canvas supports clipping regions
