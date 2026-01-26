# ScrollView Quick Reference Guide

## Import
```rust
use scarlet_ui::ScrollView;
```

## Basic Usage

### Vertical Scrolling (Default)
```rust
let scroll_view = ScrollView::new(
    VStack::new()
        .child(Label::new("Item 1"))
        .child(Label::new("Item 2"))
        // ... more items
);
```

### Horizontal Scrolling
```rust
let scroll_view = ScrollView::new(
    HStack::new()
        .child(Label::new("Column 1"))
        .child(Label::new("Column 2"))
        // ... more columns
)
.shows_horizontal_scrollbar(true)
.shows_vertical_scrollbar(false);
```

### Bidirectional Scrolling
```rust
let scroll_view = ScrollView::new(large_grid)
    .shows_vertical_scrollbar(true)
    .shows_horizontal_scrollbar(true);
```

## Configuration Methods

### Scrollbar Visibility
- `.shows_vertical_scrollbar(bool)` - Enable/disable vertical scrollbar (default: true)
- `.shows_horizontal_scrollbar(bool)` - Enable/disable horizontal scrollbar (default: false)

### Appearance
- `.scrollbar_width(u32)` - Set scrollbar width in pixels (default: 12)
- `.scrollbar_color(Color)` - Set thumb color (default: rgb(140, 140, 140))
- `.scrollbar_track_color(Color)` - Set track color (default: rgb(70, 70, 70))

### Behavior
- `.wheel_scroll_speed(i32)` - Set pixels per wheel tick (default: 30)

## Runtime Control

### Get Current Position
```rust
let y_offset = scroll_view.scroll_offset_y();
let x_offset = scroll_view.scroll_offset_x();
```

### Set Position (Absolute)
```rust
scroll_view.set_scroll_offset_y(100);  // Scroll to Y=100
scroll_view.set_scroll_offset_x(50);   // Scroll to X=50
```

### Scroll (Relative)
```rust
scroll_view.scroll_by_y(-20);  // Scroll up 20 pixels
scroll_view.scroll_by_y(20);   // Scroll down 20 pixels
scroll_view.scroll_by_x(-10);  // Scroll left 10 pixels
scroll_view.scroll_by_x(10);   // Scroll right 10 pixels
```

## Event Handling

The ScrollView automatically handles:
- **Mouse Wheel**: Scrolls content (vertical/horizontal based on delta)
- **Scrollbar Thumb Drag**: Drag to scroll
- **Scrollbar Track Click**: Jump to position
- **Child Events**: Forwards to child with adjusted coordinates

No additional event handling code needed!

## Common Patterns

### Scrollable List
```rust
ScrollView::new(
    VStack::new()
        .spacing(8)
        .children(items.iter().map(|item| {
            Label::new(item).font_size(14)
        }))
)
```

### Scrollable Form
```rust
ScrollView::new(
    VStack::new()
        .spacing(16)
        .child(TextField::new("Name"))
        .child(TextField::new("Email"))
        .child(TextField::new("Address"))
        .child(Button::new("Submit", || { /* ... */ }))
)
```

### Scrollable Image/Canvas
```rust
ScrollView::new(
    RectView::large_image()
)
.shows_horizontal_scrollbar(true)
.shows_vertical_scrollbar(true)
```

### Custom Styled Scrollbar
```rust
ScrollView::new(content)
    .scrollbar_width(16)
    .scrollbar_color(Color::rgb(66, 133, 244))   // Blue
    .scrollbar_track_color(Color::rgb(30, 30, 30))
    .wheel_scroll_speed(50)  // Faster scrolling
```

## Integration with Layout

### In VStack (expands to fill)
```rust
VStack::new()
    .child(Label::new("Header"))
    .child(ScrollView::new(content))  // Expands
    .child(Label::new("Footer"))
```

### With Fixed Height
```rust
// Note: ScrollView has flex_factor=1, so it will expand
// To constrain height, wrap in a container with fixed size
VStack::new()
    .child(ScrollView::new(content))
    .spacing(0)
```

## Notes

- ScrollView always expands to fill available space (flex_factor = 1)
- Scrollbars appear automatically when content exceeds bounds
- Child views receive events with coordinates adjusted for scroll offset
- Currently no clipping (TODO: add to Canvas)

## Full Example

```rust
use scarlet_ui::*;

fn create_scrollable_list() -> ScrollView {
    let mut list = VStack::new().spacing(12);

    for i in 0..100 {
        list = list.child(
            HStack::new()
                .spacing(8)
                .child(
                    RectView::new(Color::rgb(100, 150, 200))
                        .width(40)
                        .height(40)
                        .corner_radius(8)
                )
                .child(
                    VStack::new()
                        .spacing(4)
                        .child(Label::new(format!("Item {}", i)))
                        .child(Label::new("Description").color(Color::GRAY))
                )
        );
    }

    ScrollView::new(list)
        .scrollbar_width(12)
        .wheel_scroll_speed(40)
}
```
