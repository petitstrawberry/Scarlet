# ScarletUI Layout System

ScarletUI uses a constraint-based layout system inspired by SwiftUI, enabling flexible and responsive UI design.

## Layout Constraints

### Core Concepts

Layout constraints define the minimum and maximum size for a node:

```rust
pub struct LayoutConstraints {
    pub min: Size,  // Minimum size
    pub max: Size,  // Maximum size
}

pub struct Size {
    pub width: f32,
    pub height: f32,
}
```

### Constraint Types

```rust
impl LayoutConstraints {
    // Fixed size: node must be exactly this size
    pub fn tight(size: Size) -> Self {
        Self { min: size, max: size }
    }

    // Maximum size: node can be any size up to max
    pub fn loose(max: Size) -> Self {
        Self { min: Size::ZERO, max }
    }

    // No constraints: node decides its own size
    pub fn unconstrained() -> Self {
        Self { min: Size::ZERO, max: Size::INFINITE }
    }

    // Clamp size to constraints
    pub fn clamp(&self, size: Size) -> Size {
        Size::new(
            size.width.clamp(self.min.width, self.max.width),
            size.height.clamp(self.min.height, self.max.height),
        )
    }
}
```

### Examples

```rust
// Fixed 200x100 size
let constraints = LayoutConstraints::tight(Size::new(200.0, 100.0));

// Up to 800x600, content decides actual size
let constraints = LayoutConstraints::loose(Size::new(800.0, 600.0));

// Width fixed 200, height up to 100
let constraints = LayoutConstraints {
    min: Size::new(200.0, 0.0),
    max: Size::new(200.0, 100.0),
};
```

## Layout Process

### Two-Pass Layout

ScarletUI uses a two-pass layout algorithm:

1. **Measure Pass**: Determine minimum requirements with loose constraints
2. **Layout Pass**: Distribute available space according to constraints

### VStack Layout Example

```rust
impl RenderNode for VStackRenderNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let n = self.children.len();
        if n == 0 { return Size::ZERO; }

        let available_height = constraints.max.height;
        let total_spacing = self.spacing * (n - 1) as f32;
        let available_for_children = available_height - total_spacing;

        // Pass 1: Measure minimum requirements
        let loose = LayoutConstraints::loose(constraints.max);
        let min_heights: Vec<f32> = self.children.iter()
            .map(|c| c.layout(loose).height)
            .collect();

        let min_total: f32 = min_heights.iter().sum();

        // Pass 2: Distribute space
        if min_total <= available_for_children {
            // Space available: distribute equally
            let remaining = available_for_children - min_total;
            let per_child = remaining / n as f32;

            for (i, child) in self.children.iter_mut().enumerate() {
                let child_constraints = LayoutConstraints {
                    min: Size::new(constraints.min.width, min_heights[i]),
                    max: Size::new(constraints.max.width, min_heights[i] + per_child),
                };
                child.layout(child_constraints);
            }
        } else {
            // Not enough space: clamp proportionally
            for (i, child) in self.children.iter_mut().enumerate() {
                let ratio = min_heights[i] / min_total;
                let child_height = (available_for_children * ratio).min(min_heights[i]);
                let child_constraints = LayoutConstraints::tight(Size::new(
                    constraints.max.width,
                    child_height,
                ));
                child.layout(child_constraints);
            }
        }

        // Calculate total size
        let max_width = constraints.max.width;
        let total_height = min_total.min(available_for_children) + total_spacing;
        Size::new(max_width, total_height)
    }
}
```

## Built-in Layout Containers

### VStack

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

**Layout Behavior:**
- Stacks children vertically
- Spacing between children
- Aligns children horizontally
- Distributes available space equally when possible

### HStack

Arranges children horizontally (not yet implemented):

```rust
HStack {
    children: vec![
        Box::new(Text::new("Left")),
        Box::new(Text::new("Center")),
        Box::new(Text::new("Right")),
    ],
    spacing: 10.0,
    alignment: Alignment::Center,
}
```

**Layout Behavior:**
- Stacks children horizontally
- Spacing between children
- Aligns children vertically
- Distributes available width equally

### ZStack

Layers children on top of each other (not yet implemented):

```rust
ZStack {
    children: vec![
        Box::new(Rectangle::new([255, 0, 0, 255])),
        Box::new(Text::new("Overlay")),
    ],
    alignment: Alignment::Center,
}
```

**Layout Behavior:**
- Layers children back-to-front
- All children fill available space (unless constrained)
- Alignment determines positioning

## Alignment

```rust
pub enum Alignment {
    Leading,    // Top/Left
    Center,     // Center
    Trailing,   // Bottom/Right
}
```

### VStack Alignment

Controls horizontal alignment of children:

```rust
VStack {
    children: vec![
        Box::new(Text::new("Aligned Left")),
        Box::new(Text::new("Aligned Center")),
        Box::new(Text::new("Aligned Right")),
    ],
    spacing: 10.0,
    alignment: Alignment::Leading,  // or Center, Trailing
}
```

### HStack Alignment

Controls vertical alignment of children:

```rust
HStack {
    children: vec![
        Box::new(Text::new("Top")),
        Box::new(Text::new("Middle")),
        Box::new(Text::new("Bottom")),
    ],
    spacing: 10.0,
    alignment: Alignment::Leading,  // Top aligned
}
```

## Custom Layout Behavior

### Fixed-Size Component

```rust
impl RenderNode for FixedSizeNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Ignore constraints, use fixed size
        let size = Size::new(200.0, 100.0);
        self.frame = Rect::new(Point::ZERO, size);
        size
    }
}
```

### Fill-Available-Space Component

```rust
impl RenderNode for FillNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Fill all available space
        let size = Size::new(
            constraints.max.width,
            constraints.max.height,
        );
        self.frame = Rect::new(Point::ZERO, size);
        size
    }
}
```

### Content-Driven Component

```rust
impl RenderNode for ContentNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Measure content first
        let content_size = self.measure_content();

        // Apply constraints
        let size = Size::new(
            content_size.width.min(constraints.max.width),
            content_size.height.min(constraints.max.height),
        );
        self.frame = Rect::new(Point::ZERO, size);
        size
    }
}
```

### Aspect-Ratio Component

```rust
impl RenderNode for AspectRatioNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let aspect_ratio = 16.0 / 9.0;

        // Calculate width from height
        let height = constraints.max.height;
        let width = height * aspect_ratio;

        // Or calculate height from width
        let width = constraints.max.width;
        let height = width / aspect_ratio;

        let size = Size::new(width, height);
        self.frame = Rect::new(Point::ZERO, size);
        size
    }
}
```

## Spacer

Spacer fills available space:

```rust
VStack {
    children: vec![
        Box::new(Text::new("Top")),
        Box::new(Spacer::new()),  // Pushes next item to bottom
        Box::new(Text::new("Bottom")),
    ],
    spacing: 0.0,
    alignment: Alignment::Center,
}
```

**Implementation:**

```rust
impl RenderNode for Spacer {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Fill all available space
        let size = Size::new(
            constraints.max.width,
            constraints.max.height,
        );
        self.frame = Rect::new(Point::ZERO, size);
        size
    }
}
```

## Common Layout Patterns

### Center Content

```rust
VStack {
    children: vec![
        Box::new(
            HStack {
                children: vec![
                    Box::new(Spacer::new()),
                    Box::new(Text::new("Centered")),
                    Box::new(Spacer::new()),
                ],
                spacing: 0.0,
                alignment: Alignment::Center,
            }
        ),
    ],
    spacing: 0.0,
    alignment: Alignment::Center,
}
```

### Aspect-Ratio Container

```rust
struct AspectRatioContainer {
    aspect_ratio: f32,
    child: Box<dyn View>,
}

impl RenderNode for AspectRatioContainerRenderNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let width = constraints.max.width;
        let height = width / self.aspect_ratio;

        let child_constraints = LayoutConstraints::tight(Size::new(width, height));
        self.child.layout(child_constraints);

        Size::new(width, height)
    }
}
```

### Responsive Layout

```rust
struct ResponsiveLayout {
    breakpoint: f32,
    portrait: Box<dyn View>,
    landscape: Box<dyn View>,
}

impl RenderNode for ResponsiveLayoutRenderNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        if constraints.max.width < self.breakpoint {
            // Portrait layout
            self.portrait.layout(constraints)
        } else {
            // Landscape layout
            self.landscape.layout(constraints)
        }
    }
}
```

### Grid Layout

```rust
struct Grid {
    columns: usize,
    spacing: f32,
    children: Vec<Box<dyn View>>,
}

impl RenderNode for GridRenderNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let available_width = constraints.max.width;
        let column_spacing = self.spacing * (self.columns - 1) as f32;
        let column_width = (available_width - column_spacing) / self.columns as f32;

        let mut y_offset = 0.0;
        let mut max_row_height = 0.0;

        for (i, child) in self.children.iter_mut().enumerate() {
            let column = i % self.columns;
            let x_offset = column as f32 * (column_width + self.spacing);

            let child_constraints = LayoutConstraints::tight(Size::new(column_width, constraints.max.height));
            let child_size = child.layout(child_constraints);

            if column == 0 {
                y_offset += max_row_height + self.spacing;
                max_row_height = 0.0;
            }
            max_row_height = max_row_height.max(child_size.height);

            child.set_frame(Rect::new(Point::new(x_offset, y_offset), child_size));
        }

        Size::new(available_width, y_offset + max_row_height)
    }
}
```

## Layout Debugging

### Visualize Frames

```rust
impl RenderNode for DebugNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let size = self.child.layout(constraints);

        // Draw frame border
        self.buffer = Some(Buffer::new(size));
        self.buffer.as_mut().unwrap()
            .fill_rect(Rect::new(Point::ZERO, size), [255, 0, 0, 255]);

        size
    }
}
```

### Print Constraints

```rust
impl RenderNode for DebugNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        println!("Layout constraints: min={:?} max={:?}", constraints.min, constraints.max);

        let size = self.inner.layout(constraints);

        println!("Layout result: {:?}", size);

        size
    }
}
```

## Best Practices

### 1. Respect Constraints

```rust
// ❌ BAD - Ignores constraints
fn layout(&mut self, constraints: LayoutConstraints) -> Size {
    let size = Size::new(1000.0, 1000.0);  // Too big!
    size
}

// ✅ GOOD - Respects constraints
fn layout(&mut self, constraints: LayoutConstraints) -> Size {
    let desired = Size::new(1000.0, 1000.0);
    let size = constraints.clamp(desired);
    self.frame = Rect::new(Point::ZERO, size);
    size
}
```

### 2. Provide Minimum Size

```rust
// ✅ GOOD - Specifies minimum requirements
fn layout(&mut self, constraints: LayoutConstraints) -> Size {
    let min_size = Size::new(100.0, 50.0);  // Minimum viable size

    let size = Size::new(
        constraints.max.width.max(min_size.width),
        constraints.max.height.max(min_size.height),
    );

    self.frame = Rect::new(Point::ZERO, size);
    size
}
```

### 3. Handle Overflow

```rust
// ✅ GOOD - Handles constrained space
fn layout(&mut self, constraints: LayoutConstraints) -> Size {
    let ideal_size = self.calculate_ideal_size();

    if ideal_size.width > constraints.max.width {
        // Clip or scale down
        let scale = constraints.max.width / ideal_size.width;
        let size = Size::new(constraints.max.width, ideal_size.height * scale);
        self.frame = Rect::new(Point::ZERO, size);
        return size;
    }

    self.frame = Rect::new(Point::ZERO, ideal_size);
    ideal_size
}
```

### 4. Cache Layout Results

```rust
struct CachedLayoutNode {
    cached_size: Option<Size>,
    last_constraints: Option<LayoutConstraints>,
}

impl RenderNode for CachedLayoutNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Return cached if constraints unchanged
        if let (Some(cached), Some(last)) = (self.cached_size, &self.last_constraints) {
            if last == &constraints {
                return cached;
            }
        }

        let size = self.calculate_layout(constraints);
        self.cached_size = Some(size);
        self.last_constraints = Some(constraints);
        size
    }
}
```

## Advanced Topics

### Custom Layout Parameters

```rust
struct FlexibleLayout {
    flex_factors: Vec<f32>,  // Relative sizing
}

impl RenderNode for FlexibleLayoutRenderNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let total_flex: f32 = self.flex_factors.iter().sum();
        let available = constraints.max.width;

        for (i, child) in self.children.iter_mut().enumerate() {
            let flex = self.flex_factors[i];
            let child_width = (flex / total_flex) * available;

            let child_constraints = LayoutConstraints::tight(Size::new(child_width, constraints.max.height));
            child.layout(child_constraints);
        }

        Size::new(available, constraints.max.height)
    }
}
```

### Animation Support

```rust
struct AnimatedLayout {
    target_size: Size,
    current_size: Size,
    animation_duration: Duration,
}

impl RenderNode for AnimatedLayoutRenderNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        let target = self.target_size;
        let current = self.current_size;

        // Animate towards target
        let progress = self.elapsed() / self.animation_duration;
        let size = Size::new(
            lerp(current.width, target.width, progress),
            lerp(current.height, target.height, progress),
        );

        self.frame = Rect::new(Point::ZERO, size);
        size
    }
}
```

### Layout Invalidation

```rust
impl RenderNode for MyNode {
    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        // Mark as needing layout
        self.mark_dirty(DirtyFlags::LAYOUT);

        let size = self.calculate_layout(constraints);

        // Clear layout dirty flag
        self.dirty_flags.remove(DirtyFlags::LAYOUT);

        size
    }
}
```
