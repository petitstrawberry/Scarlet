# ViewExt Trait - SwiftUI-Style Method Chaining

## Overview

The `ViewExt` trait provides SwiftUI-style method chaining for all views in ScarletUI. It is automatically implemented for all types that implement the `View` trait, so you can use these methods on any view without any additional imports or setup.

## Usage

### Basic Usage

Before ViewExt, you had to write:
```rust
let view = Padding::new(
    Background::new(
        Frame::new(Text::new("Hello"), 200.0, 50.0),
        Color::BLUE
    ),
    10.0
);
```

With ViewExt, you can now write:
```rust
let view = Text::new("Hello")
    .padding(10.0)
    .background(Color::BLUE)
    .frame(200.0, 50.0);
```

## Available Methods

### Padding

Add padding around a view:

```rust
// Uniform padding on all edges
let view = Text::new("Hello").padding(10.0);

// Custom padding for each edge
let view = Text::new("Hello")
    .padding_insets(EdgeInsets {
        top: 5.0,
        left: 10.0,
        bottom: 15.0,
        right: 20.0,
    });
```

### Background

Add a background color:

```rust
let view = Text::new("Hello")
    .background(Color::BLUE);

// With custom color
let view = Text::new("Hello")
    .background(Color::rgb(0.2, 0.3, 0.4));
```

### Frame

Set fixed size constraints:

```rust
// Fixed width and height
let view = Text::new("Hello").frame(200.0, 50.0);

// Fixed width only
let view = Text::new("Hello").frame_width(200.0);

// Fixed height only
let view = Text::new("Hello").frame_height(50.0);
```

### Size Constraints

Set minimum and maximum size constraints:

```rust
let view = Text::new("Hello")
    .size_constraints(
        100.0,  // min_width
        50.0,   // min_height
        500.0,  // max_width
        200.0   // max_height
    );
```

### Alignment

Align a view within its container:

```rust
let view = Text::new("Hello").alignment(Alignment::Center);
```

## Complex Chains

You can chain multiple modifiers together:

```rust
let view = Text::new("Complex Layout")
    .padding(20.0)
    .background(Color::rgb(0.2, 0.3, 0.4))
    .frame(400.0, 100.0)
    .alignment(Alignment::Center);
```

## Works with All Views

Since ViewExt is automatically implemented for all `View` types, it works with:

- `Text`
- `Button`
- `Rectangle`
- `Image`
- `Spacer`
- `VStack`, `HStack`, `ZStack`
- Any custom view implementing `View`

## Type Safety

The ViewExt trait maintains type safety through Rust's type system. Each modifier returns a wrapper type that also implements `View`, allowing you to continue chaining:

```rust
Text::new("Hello")              // Text
    .padding(10.0)              // Padding<Text>
    .background(Color::BLUE)    // Background<Padding<Text>>
    .frame(200.0, 50.0);        // Frame<Background<Padding<Text>>>
```

All these types implement `View`, so they can be used anywhere a `View` is expected.
