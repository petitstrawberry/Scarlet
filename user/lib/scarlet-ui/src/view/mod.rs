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
mod containers;
mod controls;
mod window;

pub use traits::{View, ViewBox, Size, IntoViewBox};
pub use containers::{HStack, StackAlignment, VStack, ZStack, Padding, Center};
pub use controls::{
    // Basic views
    Label, Button, Spacer, RectView,
    // Reactive controls (all use Binding<T>)
    ReactiveLabel, TextField, CheckBox, Slider, ProgressBar, Toggle,
};
pub use window::Window;
