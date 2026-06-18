# ScarletUI API Reference

ScarletUI is a declarative UI framework for Scarlet OS, inspired by Flutter and SwiftUI.

## Table of Contents

- [Getting Started](#getting-started)
- [Core Concepts](#core-concepts)
- [Geometry](#geometry)
- [Colors](#colors)
- [State Management](#state-management)
- [Views](#views)
  - [Basic Views](#basic-views)
  - [Layout Containers](#layout-containers)
  - [Interactive Views](#interactive-views)
- [View Modifiers](#view-modifiers)
- [Events](#events)
- [Application](#application)
- [Platform Integration](#platform-integration)

---

## Getting Started

### Basic Application

```rust
use scarlet_ui::prelude::*;

#[derive(View, Clone)]
struct CounterApp {
    count: State<i32>,
}

impl CounterApp {
    fn new() -> Self {
        Self {
            ..Default::default()
        }
    }

    fn counter_view(&self) -> impl View {
        vstack! {
            Text::new("Counter"),
            Button::new("Increment")
                .on_click(|_| self.count.update(|c| *c += 1)),
            Text::new(&format!("Count: {}", self.count.get())),
        }
        .spacing(10.0)
    }
}

impl Application for CounterApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new("main", Window::new("Counter", self.counter_view()))
    }
}

fn main() {
    let mut app = CounterApp::new();
    app.run();
}
```

---

## Core Concepts

### View Trait

The `View` trait is the core abstraction for UI components. Views are immutable blueprints that create `Element` instances at runtime.

```rust
pub trait View: Any {
    fn create_element(&self) -> Box<dyn Element>;
    fn listenables(&self) -> Vec<&dyn Listenable> { Vec::new() }
    fn as_any(&self) -> &dyn Any;
    fn type_id(&self) -> TypeId;
    fn type_name(&self) -> &str;
}
```

### Creating Custom Views

Use the `#[derive(View)]` macro for custom views that compose other views:

```rust
use scarlet_ui::prelude::*;

#[derive(View, Clone)]
struct MyCustomView {
    title: String,
    count: State<i32>,
}

impl MyCustomView {
    fn body(&self) -> impl View {
        vstack! {
            Text::new(&self.title).font_size(20.0),
            Text::new(&format!("Count: {}", self.count.get())),
            Button::new("Increment")
                .on_click(|| self.count.update(|c| *c += 1)),
        }
        .spacing(10.0)
    }
}
```

**Key Points:**
- Add `#[derive(View, Clone)]` to your struct
- Implement a `body(&self) -> impl View` method
- Use State fields for reactive data
- Compose built-in views with macros like `vstack!`, `hstack!`

For custom rendering logic, implement the `View` trait directly with `create_element()`.

### Element

Elements are the runtime representation of views, handling layout and rendering.

### Application Trait

Applications implement the `Application` trait to define their UI and behavior.

---

## Geometry

### Size

2D size with width and height.

```rust
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const ZERO: Self;
    pub const fn new(width: f32, height: f32) -> Self;
    pub fn is_infinite(&self) -> bool;
    pub fn is_zero(&self) -> bool;
    pub fn constrain(&self, min: Size, max: Size) -> Size;
}
```

**Example:**
```rust
let size = Size::new(100.0, 200.0);
let constrained = size.constrain(Size::new(50.0, 50.0), Size::new(300.0, 300.0));
```

### Point

2D point with x and y coordinates.

```rust
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Self;
    pub const fn new(x: f32, y: f32) -> Self;
    pub fn distance_to(&self, other: Point) -> f32;
    pub fn translate(&self, offset: Offset) -> Point;
}

impl Add<Offset> for Point { ... }
impl AddAssign<Offset> for Point { ... }
```

**Example:**
```rust
let point = Point::new(10.0, 20.0);
let translated = point.translate(Offset::new(5.0, 5.0));
```

### Offset

2D offset with dx and dy components.

```rust
pub struct Offset {
    pub dx: f32,
    pub dy: f32,
}

impl Offset {
    pub const ZERO: Self;
    pub const fn new(dx: f32, dy: f32) -> Self;
    pub fn from_size(size: Size) -> Self;
}

impl Add for Offset { ... }
impl Sub for Offset { ... }
```

### Rect

2D rectangle with origin and size.

```rust
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub const fn zero() -> Self;
    pub const fn new(origin: Point, size: Size) -> Self;
    pub const fn from_xywh(x: f32, y: f32, width: f32, height: f32) -> Self;
    pub fn from_point_size(point: Point, size: Size) -> Self;
    pub fn left(&self) -> f32;
    pub fn right(&self) -> f32;
    pub fn top(&self) -> f32;
    pub fn bottom(&self) -> f32;
    pub fn width(&self) -> f32;
    pub fn height(&self) -> f32;
    pub fn contains(&self, point: Point) -> bool;
    pub fn overlaps(&self, other: &Rect) -> bool;
    pub fn inset(&self, insets: EdgeInsets) -> Rect;
    pub fn center(&self) -> Point;
}
```

**Example:**
```rust
let rect = Rect::from_xywh(10.0, 20.0, 100.0, 50.0);
let center = rect.center();
```

### EdgeInsets

Insets for padding/border on each side.

```rust
pub struct EdgeInsets {
    pub top: f32,
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
}

impl EdgeInsets {
    pub const ZERO: Self;
    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self;
    pub const fn all(value: f32) -> Self;
    pub const fn symmetric(vertical: f32, horizontal: f32) -> Self;
    pub const fn only_top(value: f32) -> Self;
    pub const fn only_left(value: f32) -> Self;
    pub const fn only_right(value: f32) -> Self;
    pub const fn only_bottom(value: f32) -> Self;
    pub const fn horizontal(value: f32) -> Self;
    pub const fn vertical(value: f32) -> Self;
}
```

**Example:**
```rust
let insets = EdgeInsets::all(10.0);
let symmetric = EdgeInsets::symmetric(8.0, 16.0); // top/bottom: 8, left/right: 16
```

### Alignment

Alignment for positioning children in containers.

```rust
pub enum Alignment {
    Center,        // Center in both axes
    Leading,       // Leading edge (left for LTR)
    Trailing,      // Trailing edge (right for LTR)
    Top,           // Top edge
    Bottom,        // Bottom edge
    TopLeading,    // Top-left corner
    TopTrailing,   // Top-right corner
    BottomLeading, // Bottom-left corner
    BottomTrailing,// Bottom-right corner
}

impl Alignment {
    pub fn apply(&self, parent_size: Size, child_size: Size) -> Point;
    pub fn align_x(&self, parent_width: f32, child_width: f32) -> f32;
    pub fn align_y(&self, parent_height: f32, child_height: f32) -> f32;
}
```

---

## Colors

### Color

RGBA color with floating-point components (0.0 - 1.0).

```rust
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    // Constructors
    pub fn rgb<T1, T2, T3>(r: T1, g: T2, b: T3) -> Self
    where T1: ColorComponent, T2: ColorComponent, T3: ColorComponent;

    pub fn rgba<T1, T2, T3, T4>(r: T1, g: T2, b: T3, a: T4) -> Self
    where T1: ColorComponent, T2: ColorComponent, T3: ColorComponent, T4: ColorComponent;

    pub const fn rgb_f32(r: f32, g: f32, b: f32) -> Self;
    pub const fn rgba_f32(r: f32, g: f32, b: f32, a: f32) -> Self;
    pub const fn gray(gray: f32) -> Self;

    // Predefined colors
    pub const BLACK: Self;
    pub const WHITE: Self;
    pub const RED: Self;
    pub const GREEN: Self;
    pub const BLUE: Self;
    pub const YELLOW: Self;
    pub const CYAN: Self;
    pub const MAGENTA: Self;
    pub const TRANSPARENT: Self;
    pub const CLEAR: Self; // Alias for TRANSPARENT

    // Conversion
    pub fn to_bgra(&self) -> u32;
    pub fn to_rgba(&self) -> u32;
    pub fn from_bgra(bgra: u32) -> Self;

    // Manipulation
    pub fn blend_over(&self, background: Color) -> Color;
    pub fn lighten(&self, factor: f32) -> Color;
    pub fn darken(&self, factor: f32) -> Color;

    // Opacity
    pub fn opacity(&self) -> f32;
    pub fn with_opacity(&self, opacity: f32) -> Color;
}
```

**Example:**
```rust
let red = Color::rgb(255, 0, 0);
let semi_transparent_blue = Color::rgba(0, 0, 255, 128);
let custom = Color::rgb_f32(0.5, 0.7, 0.9);
let faded_red = red.with_opacity(0.5);
```

### Color Palette & Scheme

```rust
pub struct ColorPalette;
pub struct ColorScheme;

impl ColorPalette {
    pub fn default() -> Self;
    pub fn text_primary(&self) -> Color;
    pub fn text_secondary(&self) -> Color;
    pub fn button_background(&self) -> Color;
    pub fn border(&self) -> Color;
    // ... more methods
}

pub enum ColorScheme {
    Light,
    Dark,
}
```

### System Colors

macOS-style system colors for semantic UI elements.

```rust
pub struct SystemColors;
pub struct GrayColors;
pub struct BlueColors;
// ... other color groups

impl SystemColors {
    pub fn system_gray() -> Color;
    pub fn system_blue() -> Color;
    // ... more colors
}
```

---

## State Management

### State

Reactive state container with subscription notifications.

```rust
#[derive(Clone)]
pub struct State<T> {
    // ...
}

impl<T> State<T> {
    pub fn new(id: StateId, value: T) -> Self;
    pub fn id(&self) -> StateId;
    pub fn get(&self) -> T where T: Clone;
    pub fn set(&self, value: T) where T: Clone;
    pub fn update<F>(&self, f: F) where F: FnOnce(&mut T), T: Clone;
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool;
}

impl<T: Default> State<T> {
    pub fn initial(id: StateId) -> Self;
}
```

**Example:**
```rust
let counter: State<i32> = State::initial(StateId::new(1));
counter.set(0);
counter.update(|c| *c += 1);
let value = counter.get();
```

### StateId

Unique identifier for State instances.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StateId(u32);

impl StateId {
    pub const fn new(id: u32) -> Self;
    pub const fn get(self) -> u32;
}

pub fn generate_state_id() -> StateId;
```

### Listenable Trait

Trait for types that can be subscribed to for change notifications.

```rust
pub trait Listenable: Any {
    fn subscribe_any(&self, callback: Arc<dyn Fn() + Send + Sync>) -> SubscriptionId;
    fn unsubscribe(&self, id: SubscriptionId) -> bool;
}
```

---

## Views

### Basic Views

#### Text

Displays text with configurable font size and color.

```rust
pub struct Text {
    // private fields
}

impl Text {
    pub fn new(content: impl Into<String>) -> Self;
    pub fn font_size(mut self, size: f32) -> Self;
    pub fn color(mut self, color: Color) -> Self;
    pub fn content(&self) -> &str;
    pub fn font_size_value(&self) -> f32;
    pub fn text_color(&self) -> Color;
}
```

**Example:**
```rust
let hello = Text::new("Hello, World!");
let styled = Text::new("Styled Text")
    .font_size(24.0)
    .color(Color::BLUE);
```

#### Button

Interactive button with click callback.

```rust
pub struct Button {
    // private fields
}

impl Button {
    pub fn new(label: impl Into<String>) -> Self;
    pub fn on_click(mut self, callback: impl Fn() + 'static) -> Self;
    pub fn background_color(mut self, color: Color) -> Self;
    pub fn text_color(mut self, color: Color) -> Self;
    pub fn border_color(mut self, color: Color) -> Self;
    pub fn font_size(mut self, size: f32) -> Self;
    pub fn padding(mut self, padding: f32) -> Self;

    // Getters
    pub fn label(&self) -> &str;
    pub fn get_background_color(&self) -> Color;
    pub fn get_text_color(&self) -> Color;
    pub fn get_border_color(&self) -> Color;
    pub fn get_font_size(&self) -> f32;
    pub fn get_padding(&self) -> f32;
}
```

**Example:**
```rust
let button = Button::new("Click Me")
    .on_click(|| {
        println!("Button clicked!");
    })
    .background_color(Color::BLUE)
    .text_color(Color::WHITE);
```

#### Rectangle

Filled rectangle with color and corner radius.

```rust
pub struct Rectangle {
    // private fields
}

impl Rectangle {
    pub fn new() -> Self;
    pub fn color(mut self, color: Color) -> Self;
    pub fn corner_radius(mut self, radius: f32) -> Self;
}
```

**Example:**
```rust
let rect = Rectangle::new()
    .color(Color::BLUE)
    .corner_radius(8.0);
```

#### Spacer

Flexible or fixed empty space.

```rust
pub struct Spacer {
    // private fields
}

impl Spacer {
    pub fn new() -> Self;
    pub fn width(mut self, width: f32) -> Self;
    pub fn height(mut self, height: f32) -> Self;
    pub fn flex(mut self, flex: u32) -> Self;
}
```

**Example:**
```rust
let fixed_spacer = Spacer::new().width(20.0);
let flexible_spacer = Spacer::new().flex(1);
```

#### Image

Display images from various sources.

```rust
pub struct Image {
    // private fields
}

impl Image {
    pub fn new() -> Self;
    pub fn from_path(path: impl Into<String>) -> Self;
    pub fn from_bytes(data: Vec<u8>, width: u32, height: u32) -> Self;
    pub fn resizable(mut self, resizable: bool) -> Self;
}
```

#### CanvasView

Custom drawing canvas.

```rust
pub struct CanvasView<F>
where
    F: Fn(&mut Canvas, u32, u32) + 'static,
{
    // private fields
}

impl<F> CanvasView<F>
where
    F: Fn(&mut Canvas, u32, u32) + 'static,
{
    pub fn new(draw_fn: F) -> Self;
    pub fn width(mut self, width: f32) -> Self;
    pub fn height(mut self, height: f32) -> Self;
}
```

**Example:**
```rust
let canvas = CanvasView::new(|canvas, width, height| {
    canvas.fill_rect(0, 0, width, height, Color::WHITE);
    canvas.draw_circle(width / 2, height / 2, 50, Color::RED);
})
.width(200.0)
.height(200.0);
```

#### Window

Top-level window container with decorations (title bar, borders, buttons).

```rust
pub struct Window<V: View> {
    // private fields
}

impl<V: View> Window<V> {
    pub fn new(title: impl Into<String>, content: V) -> Self;
    pub fn app_id(mut self, app_id: impl Into<String>) -> Self;
    pub fn size(mut self, size: Size) -> Self;
    pub fn min_size(mut self, size: Size) -> Self;
    pub fn max_size(mut self, size: Size) -> Self;
    pub fn size_limits(mut self, min: Size, max: Size) -> Self;
    pub fn resizable(mut self, resizable: bool) -> Self;
    pub fn movable(mut self, movable: bool) -> Self;
    pub fn decorated(mut self, decorated: bool) -> Self;
    pub fn background_color(mut self, color: Color) -> Self;
    pub fn opaque(mut self, opaque: bool) -> Self;
    pub fn window_type(mut self, window_type: u32) -> Self;
    pub fn focus_on_create(mut self, focus_on_create: bool) -> Self;
    pub fn active_on_focus(mut self, active_on_focus: bool) -> Self;
    pub fn menu_bar(mut self, menu_bar: MenuBarModel) -> Self;

    // Getters
    pub fn get_app_id(&self) -> &str;
    pub fn get_title(&self) -> &str;
    pub fn get_window_size(&self) -> Size;
    pub fn get_min_size(&self) -> Option<Size>;
    pub fn get_max_size(&self) -> Option<Size>;
    pub fn is_resizable(&self) -> bool;
    pub fn is_decorated(&self) -> bool;
    pub fn is_opaque(&self) -> bool;
}
```

**Example:**
```rust
Window::new("My App",
    vstack! {
        Text::new("Hello, World!"),
    }
    .spacing(10.0)
)
.size(Size::new(800.0, 600.0))
.resizable(true);
```

### Layout Containers

#### VStack

Vertical stack arranges children in a column.

```rust
pub struct VStack<C: ViewTuple> {
    // private fields
}

impl<C: ViewTuple> VStack<C> {
    pub fn new(content: C) -> Self;
    pub fn spacing(mut self, spacing: f32) -> Self;
    pub fn alignment(mut self, alignment: Alignment) -> Self;
    pub fn get_spacing(&self) -> f32;
    pub fn get_alignment(&self) -> Alignment;
}
```

**Example:**
```rust
let stack = vstack! {
    Text::new("Title"),
    Text::new("Subtitle"),
    Button::new("Action"),
}
.spacing(10.0)
.alignment(Alignment::Center);
```

#### HStack

Horizontal stack arranges children in a row.

```rust
pub struct HStack<C: ViewTuple> {
    // private fields
}

impl<C: ViewTuple> HStack<C> {
    pub fn new(content: C) -> Self;
    pub fn spacing(mut self, spacing: f32) -> Self;
    pub fn alignment(mut self, alignment: Alignment) -> Self;
    pub fn get_spacing(&self) -> f32;
    pub fn get_alignment(&self) -> Alignment;
}
```

**Example:**
```rust
let row = hstack! {
    Text::new("Left"),
    Spacer::new().flex(1),
    Text::new("Right"),
}
.spacing(8.0);
```

#### ZStack

Overlays children on top of each other.

```rust
pub struct ZStack<C: ViewTuple> {
    // private fields
}

impl<C: ViewTuple> ZStack<C> {
    pub fn new(content: C) -> Self;
    pub fn alignment(mut self, alignment: Alignment) -> Self;
}
```

**Example:**
```rust
let overlay = zstack! {
    Image::from_path("background.png"),
    vstack! {
        Text::new("Overlay Text").color(Color::WHITE),
    }
    .alignment(Alignment::Center),
}
.alignment(Alignment::Center);
```

### Navigation

#### NavigationView

Sidebar navigation with dynamic content switching.

```rust
pub struct NavigationView<T: NavigationLinkTuple> {
    // private fields
}

impl<T: NavigationLinkTuple> NavigationView<T> {
    pub fn new(links: T) -> Self;
    pub fn sidebar_width(mut self, width: f32) -> Self;
    pub fn get_sidebar_width(&self) -> f32;
    pub fn link_count(&self) -> usize;
    pub fn get_label(&self, index: usize) -> &str;
    pub fn get_icon(&self, index: usize) -> &Icon;
    pub fn selected_index_state(&self) -> &State<usize>;
}
```

**Example:**
```rust
let nav = NavigationView::new((
    NavigationLink::new("Home", Icon::Home, || Text::new("Home View")),
    NavigationLink::new("Settings", Icon::Settings, || Text::new("Settings View")),
))
.sidebar_width(250.0);
```

#### NavigationLink

Navigation item for NavigationView.

```rust
pub struct NavigationLink {
    // private fields
}

impl NavigationLink {
    pub fn new<V>(label: impl Into<String>, icon: Icon, content_builder: impl Fn() -> V + 'static) -> Self
    where V: View + 'static;
    pub fn label(&self) -> &str;
    pub fn icon(&self) -> &Icon;
    pub fn build_content(&self) -> Box<dyn View>;
}
```

#### Icon

Icon type for navigation items.

```rust
pub enum Icon {
    Home,
    Settings,
    Info,
    Search,
    User,
    File,
    Folder,
}
```

### Interactive Views

#### Toggle

Toggle switch with on/off states.

```rust
pub struct Toggle {
    // private fields
}

impl Toggle {
    pub fn new(is_on: State<bool>) -> Self;
    pub fn on_toggle(mut self, callback: impl Fn(bool) + 'static) -> Self;
}
```

#### Slider

Slider for numeric input.

```rust
pub struct Slider {
    // private fields
}

impl Slider {
    pub fn new(value: State<f32>) -> Self;
    pub fn range(mut self, min: f32, max: f32) -> Self;
    pub fn on_change(mut self, callback: impl Fn(f32) + 'static) -> Self;
}
```

#### ProgressView

Progress indicator.

```rust
pub struct ProgressView {
    // private fields
}

impl ProgressView {
    pub fn new(value: f32) -> Self;
}
```

#### Divider

Visual separator with orientation options.

```rust
pub enum DividerOrientation {
    Horizontal,
    Vertical,
}

pub struct Divider {
    // private fields
}

impl Divider {
    pub fn new() -> Self;
    pub fn orientation(mut self, orientation: DividerOrientation) -> Self;
    pub fn color(mut self, color: Color) -> Self;
    pub fn thickness(mut self, thickness: f32) -> Self;
}
```

---

## View Modifiers

View modifiers are available on all views through the `ViewExt` trait.

### Padding

```rust
fn padding(self, insets: f32) -> Padding<Self>;
fn padding_insets(self, insets: EdgeInsets) -> Padding<Self>;
```

**Example:**
```rust
Text::new("Padded").padding(16.0);
Text::new("Custom Padding").padding_insets(EdgeInsets::symmetric(8.0, 16.0));
```

### Background

```rust
fn background(self, color: Color) -> Background<Self>;
```

**Example:**
```rust
Text::new("On Blue").background(Color::BLUE);
```

### Frame

```rust
fn frame(self, width: f32, height: f32) -> Frame<Self>;
fn frame_width(self, width: f32) -> Frame<Self>;
fn frame_height(self, height: f32) -> Frame<Self>;
```

**Example:**
```rust
Text::new("Fixed Size").frame(200.0, 50.0);
Text::new("Fixed Width").frame_width(300.0);
```

### Size Constraints

```rust
fn size_constraints(
    self,
    min_width: f32,
    min_height: f32,
    max_width: f32,
    max_height: f32,
) -> SetSize<Self>;
```

### Alignment

```rust
fn alignment(self, alignment: Alignment) -> AlignmentFrame<Self>;
```

### Event Handlers

```rust
fn on_click<F: Fn() + Clone + 'static>(self, callback: F) -> OnClick<Self, F>;
fn on_hover<F: Fn() + Clone + 'static>(self, callback: F) -> OnHover<Self, F>;
fn on_exit<F: Fn() + Clone + 'static>(self, callback: F) -> OnExit<Self, F>;
```

**Example:**
```rust
Rectangle::new()
    .on_click(|| println!("Clicked!"))
    .on_hover(|| println!("Hovered!"))
    .on_exit(|| println!("Exited!"));
```

### Clip

```rust
fn clip(self) -> Clip<Self>;
fn clip_radius(self, radius: f32) -> Clip<Self>;
```

---

## Events

### Event Type

```rust
pub enum Event {
    Quit,
    Resize { width: u32, height: u32 },
    Mouse(MouseEvent),
    Keyboard(KeyEvent),
    Input(InputEvent),
    Focus(FocusEvent),
    Lifecycle(LifecycleEvent),
    Custom { event_type: u32, data: Vec<u8> },
    Window(WindowEvent),
    MenuItemActivated { window_id: u32, menu_item_id: String },
}
```

### MouseEvent

```rust
pub enum MouseEvent {
    Moved { x: i32, y: i32 },
    Entered { x: i32, y: i32 },
    Exited { x: i32, y: i32 },
    ButtonPressed { button: MouseButton, x: i32, y: i32 },
    ButtonReleased { button: MouseButton, x: i32, y: i32 },
    Wheel { delta_x: i32, delta_y: i32 },
}
```

### MouseButton

```rust
pub enum MouseButton {
    Left,
    Middle,
    Right,
}
```

### KeyEvent

```rust
pub enum KeyEvent {
    Pressed { keycode: KeyCode },
    Released { keycode: KeyCode },
    Char { c: char },
}
```

### KeyCode

```rust
pub enum KeyCode {
    Unknown,
    Escape,
    Enter,
    Tab,
    Backspace,
    Space,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
    Char(char),
}
```

### FocusEvent

```rust
pub enum FocusEvent {
    Gained,
    Lost,
}
```

### LifecycleEvent

```rust
pub enum LifecycleEvent {
    Mount,
    Unmount,
    Appear,
    Disappear,
}
```

### WindowEvent

```rust
pub enum WindowEvent {
    CloseRequested,
    MaximizeRequested,
    RestoreRequested,
    MinimizeRequested,
    MoveRequested,
}
```

---

## Application

### Application Trait

Main entry point for ScarletUI applications.

```rust
pub trait Application: Clone + 'static {
    fn scenes(&self) -> impl Scene;

    fn on_focus_changed(&mut self, _window_id: u32, _app_name: &str, _menu_titles: &str) {
        // Default: do nothing
    }

    fn on_active_app_changed(&mut self, _window_id: u32, _app_name: &str, _menu_titles: &str) {
        // Default: do nothing
    }

    fn register_states(&self) -> Vec<StateId> {
        Vec::new()
    }

    fn init(&mut self) {
        // Default: do nothing
    }

    fn debug_logging(&self) -> bool {
        false
    }

    fn exit_when_all_windows_closed(&self) -> bool {
        true
    }
}
```

**Example:**
```rust
#[derive(View, Clone)]
struct MyApp {
    count: State<i32>,
}

impl MyApp {
    fn content(&self) -> impl View {
        Text::new(&format!("Count: {}", self.count.get()))
    }
}

impl Application for MyApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new("main", Window::new("My App", self.content()))
    }

    fn init(&mut self) {
        self.count.set(0);
    }
}

fn main() {
    let mut app = MyApp {
        count: State::initial(StateId::new(1)),
    };
    app.run().unwrap();
}
```

---

## Platform Integration

### PlatformWindow Trait

```rust
pub trait PlatformWindow {
    fn poll_event(&mut self) -> Option<Event>;
    fn present(&mut self, buffer: Buffer) -> Result<()>;
    fn resize(&mut self, width: u32, height: u32) -> Result<()>;
    fn close(&mut self) -> Result<()>;
    fn surface_id(&self) -> u32;
    // ... more methods
}
```

### SWSPlatformWindow

Implementation for Scarlet Window Server.

```rust
impl SWSPlatformWindow {
    pub fn new(app_id: &str, title: &str, size: Size) -> Result<Self>;
    pub fn new_with_menu(
        app_id: &str,
        title: &str,
        size: Size,
        menu_json: &str,
    ) -> Result<Self>;
    pub fn create_with_type(
        app_id: &str,
        title: &str,
        size: Size,
        window_type: WindowType,
    ) -> Result<Self>;
    // ... more methods
}
```

---

## Macros

### viewstack! Macros

```rust
// Vertical stack
vstack! {
    Text::new("Item 1"),
    Text::new("Item 2"),
}

// Horizontal stack
hstack! {
    Text::new("Left"),
    Text::new("Right"),
}

// Z-stack
zstack! {
    Rectangle::new().color(Color::BLUE),
    Text::new("Overlay"),
}
```

### View Derive Macro

```rust
#[derive(View, Clone)]
struct MyView {
    counter: State<i32>,
}
```

### navigation! Macro

```rust
navigation! {
    NavigationLink::new("Home", Icon::Home, || Text::new("Home View")),
    NavigationLink::new("Settings", Icon::Settings, || Text::new("Settings View")),
}
```

---

## Utilities

### Canvas

Graphics utilities for custom drawing.

```rust
pub struct Canvas<'a> {
    // private fields
}

impl<'a> Canvas<'a> {
    pub fn new(data: &mut [u32], width: u32, height: u32) -> Self;
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: Color);
    pub fn draw_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: Color);
    pub fn draw_line(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, color: Color);
    pub fn draw_circle(&mut self, cx: i32, cy: i32, radius: i32, color: Color);
    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: Color);
    pub fn draw_text_sized(&mut self, x: i32, y: i32, text: &str, color: Color, size: f32);
    pub fn clear(&mut self, color: Color);
}
```

### measure_text_sized

```rust
pub fn measure_text_sized(text: &str, font_size: f32) -> (u32, u32);
```

### set_default_font

```rust
pub fn set_default_font(font_data: &[u8]);
```

---

## Prelude

The `prelude` module provides convenient imports:

```rust
use scarlet_ui::prelude::*;

// Includes:
// - Geometry types (Size, Point, Rect, Offset, EdgeInsets, Alignment)
// - Colors (Color, ColorScheme, ColorPalette, SemanticColor)
// - Error types (Error, Result)
// - State management (State, StateId, SubscriptionId, Listenable)
// - View traits (View, ViewExt)
// - Element types (Element, ElementId, LayoutConstraints, ElementRenderObject, DirtyFlags)
// - Events (Event, MouseEvent, KeyEvent, FocusEvent, LifecycleEvent)
// - Application (Application)
// - Views (Window, Text, Button, Rectangle, Spacer, Image, VStack, HStack, ZStack)
// - Interactive views (Divider, DividerOrientation, Toggle, Slider, ProgressView, CanvasView)
// - Menu components (MenuItem, MenuBar, Menu, MenuItemContent, MenuAction)
// - Modifiers (Padding, Frame, Background, Clip)
// - Menu model (MenuBarModel, MenuEntry, MenuItemModel)
```

---

## Examples

### Counter App

```rust
use scarlet_ui::prelude::*;

#[derive(View, Clone)]
struct CounterApp {
    count: State<i32>,
}

impl CounterApp {
    fn counter_view(&self) -> impl View {
        vstack! {
            Text::new("Counter").font_size(24.0),
            Text::new(&format!("Count: {}", self.count.get()))
                .font_size(32.0),
            hstack! {
                Button::new("-")
                    .on_click(|| self.count.update(|c| *c -= 1)),
                Spacer::new().flex(1),
                Button::new("+")
                    .on_click(|| self.count.update(|c| *c += 1)),
            }
            .spacing(10.0),
        }
        .spacing(20.0)
        .alignment(Alignment::Center)
    }
}

impl Application for CounterApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new("main", Window::new("Counter", self.counter_view()))
    }
}

fn main() {
    let mut app = CounterApp {
        count: State::initial(StateId::new(1)),
    };
    app.run().unwrap();
}
```

### TODO List

```rust
use scarlet_ui::prelude::*;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(View, Clone)]
struct TodoApp {
    items: State<Vec<String>>,
    input: State<String>,
}

impl TodoApp {
    fn content(&self) -> impl View {
        vstack! {
            Text::new("TODO List").font_size(20.0),
            Divider::new(),
            // TODO: Add TextField when available
            Button::new("Add Item")
                .on_click(|| {
                    // Add item logic
                }),
            Divider::new(),
            // TODO: Add list rendering
        }
        .spacing(10.0)
    }
}

impl Application for TodoApp {
    fn scenes(&self) -> impl Scene {
        WindowGroup::new("main", Window::new("TODO List", self.content()))
    }
}
```

---

## See Also

- [ScarletUI Design Document](./design.md) - Architecture and design decisions
- [VIEWEXT_USAGE.md](../../../user/lib/scarlet-ui/VIEWEXT_USAGE.md) - ViewExt usage guide
- [README.md](../../../user/lib/scarlet-ui/README.md) - Project overview
