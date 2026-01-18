//! View system - the core of ScarletUI's component model
//!
//! Views are the building blocks of UI. Each view knows how to:
//! - Layout itself within given bounds
//! - Draw itself to a canvas
//! - Handle input events (capture and bubble phases)
//!
//! Views can be composed into hierarchies using container views
//! like `VStack` and `HStack`. `Window` is the root view with decorations.

mod traits;
mod buffer;
mod node;
mod containers;
mod controls;
mod navigation;
mod window;

pub use traits::{View, ViewBox, Size, IntoViewBox, Focus, Hoverable};
pub use buffer::ViewBuffer;
pub use node::{ViewId, ViewNode, ViewRegistry, DirtyNotifier};
pub use containers::{HStack, StackAlignment, VStack, ZStack, Padding, Center, ScrollView};
pub use controls::{
    // Basic views
    Label, Text, Button, Spacer, RectView,
    // Reactive controls (all use Binding<T>)
    ReactiveLabel, TextField, CheckBox, Slider, ProgressBar, Toggle,
    // List controls
    ListView, SelectionMode,
};
pub use navigation::{NavigationView, NavigationItem};
pub use window::{Window, WindowKind};
