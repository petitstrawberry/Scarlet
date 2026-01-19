//! ScarletUI State Management Usage Example
//!
//! This example demonstrates the SwiftUI-style state management pattern.
//!
//! # SwiftUI Pattern
//!
//! ```swift
//! struct ContentView: View {
//!     @State private var isOn = false
//!     @State private var volume = 50.0
//!
//!     var body: some View {
//!         VStack {
//!             Toggle("Enable", isOn: $isOn)
//!             Slider(value: $volume, in: 0...100)
//!         }
//!     }
//! }
//! ```
//!
//! # ScarletUI Pattern
//!
//! ```ignore
//! use scarlet_ui::{View, Local, VStack, Toggle, Slider};
//!
//! struct ContentView {
//!     // @State equivalent - owned by the View
//!     is_on: Local<bool>,
//!     volume: Local<f32>,
//! }
//!
//! impl ContentView {
//!     fn new() -> Self {
//!         Self {
//!             // Initialize state (like @State private var isOn = false)
//!             is_on: Local::new(false),
//!             volume: Local::new(50.0),
//!         }
//!     }
//!
//!     fn build(&self) -> impl View {
//!         VStack::new()
//!             // .bind() creates a binding (like $isOn)
//!             .child(Toggle::new("Enable").bind(self.is_on.bind()))
//!             .child(Slider::new(0.0, 100.0).bind(self.volume.bind()))
//!     }
//! }
//! ```

use scarlet_ui::{View, ViewId, Local, LayoutCtx, LayoutConstraints, Size, VStack, Toggle, Slider};

/// Example View demonstrating state management
pub struct ExampleView {
    // Local state - @State equivalent
    counter: Local<u32>,
    enabled: Local<bool>,
    volume: Local<f32>,
}

impl ExampleView {
    /// Create a new example view with initial state
    pub fn new() -> Self {
        Self {
            counter: Local::new(0),
            enabled: Local::new(false),
            volume: Local::new(50.0),
        }
    }

    /// Build the UI with bindings
    pub fn build(&self) -> impl View {
        VStack::new()
            // Toggle bound to enabled state
            // .bind() is equivalent to SwiftUI's $ operator
            .child(Toggle::with_label(false, "Enable Feature").bind(self.enabled.bind()))

            // Slider bound to volume state
            .child(Slider::new(0.0, 100.0).bind(self.volume.bind()))

            // Note: Text doesn't support binding yet
            // You would use: self.counter.get() to read the value
    }

    /// Example: Update state programmatically
    pub fn increment(&self) {
        // Read current value and increment
        let current = self.counter.get();
        self.counter.set(current + 1);
    }

    /// Example: Modify state with a closure
    pub fn increment_modify(&self) {
        self.counter.modify(|v| *v += 1);
    }

    /// Example: Read state
    pub fn count(&self) -> u32 {
        self.counter.get()
    }

    /// Example: Read state efficiently (no clone)
    pub fn print_count(&self) {
        self.counter.read(|v| {
            // v is &u32, no cloning
            println!("Current count: {}", v);
        });
    }
}

impl View for ExampleView {
    fn id(&self) -> ViewId {
        // Each View implementation needs an ID
        // For simplicity, we could store one in the struct
        ViewId::new() // Note: This creates a new ID each time, not ideal
    }

    fn layout(&mut self, _ctx: &mut LayoutCtx, _constraints: LayoutConstraints) -> Size {
        Size::new(400, 300)
    }

    fn draw(&self, _ctx: &mut scarlet_ui::PaintCtx, _frame: scarlet_ui::Rect) {
        // Drawing would be implemented here
    }
}

/// Advanced Example: Sharing state between views
pub mod advanced {
    use super::*;
    use scarlet_ui::HStack;

    /// Child view that receives state through binding
    pub struct ChildView {
        // No state ownership here
        // The state is provided by the parent through binding
    }

    impl ChildView {
        pub fn new() -> Self {
            Self {}
        }

        /// Build UI that accepts a binding from parent
        ///
        /// In SwiftUI:
        /// ```swift
        /// struct ChildView: View {
        ///     @Binding var isOn: Bool
        /// }
        /// ```
        ///
        /// In ScarletUI, we pass the binding during construction:
        pub fn build_with_binding(&self, is_on: &scarlet_ui::DataContext<bool>) -> impl View {
            // is_on is Arc<DataContext<bool>> passed from parent
            Toggle::new("Child Toggle").bind(is_on)
        }
    }

    /// Parent view that shares state with children
    pub struct ParentView {
        shared_enabled: Local<bool>,
    }

    impl ParentView {
        pub fn new() -> Self {
            Self {
                shared_enabled: Local::new(false),
            }
        }

        pub fn build(&self) -> impl View {
            let child = ChildView::new();

            VStack::new()
                // Parent's toggle
                .child(Toggle::new("Parent Toggle").bind(self.shared_enabled.bind()))

                // Child's toggle - shares the same state
                // .bind() returns Arc<DataContext<bool>> which can be passed around
                .child(child.build_with_binding(&self.shared_enabled.bind()))
        }
    }
}

/// Counter Example: Complete working example
pub mod counter_example {
    use super::*;

    /// Simple counter view
    pub struct CounterView {
        count: Local<u32>,
    }

    impl CounterView {
        pub fn new() -> Self {
            Self {
                count: Local::new(0),
            }
        }

        pub fn build(&self) -> impl View {
            VStack::new()
                // Would use Text::new(format!("Count: {}", self.count.get()))
                // when Text supports dynamic content
                .child(Toggle::new("Increment").bind(self.count.bind()))
        }

        pub fn increment(&self) {
            self.count.modify(|v| *v += 1);
        }

        pub fn get_count(&self) -> u32 {
            self.count.get()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_state() {
        let state = Local::new(42);
        assert_eq!(state.get(), 42);

        state.set(100);
        assert_eq!(state.get(), 100);
    }

    #[test]
    fn test_binding() {
        let state = Local::new(false);

        // Create binding (equivalent to $state)
        let binding = state.bind();

        // Binding points to the same data
        assert_eq!(binding.get(), false);

        // Update through binding
        binding.set(true);
        assert_eq!(state.get(), true);
        assert_eq!(binding.get(), true);

        // Update through state
        state.set(false);
        assert_eq!(binding.get(), false);
    }

    #[test]
    fn test_example_view() {
        let view = ExampleView::new();

        // Initial state
        assert_eq!(view.count(), 0);
        assert_eq!(view.enabled.get(), false);
        assert_eq!(view.volume.get(), 50.0);

        // Update state
        view.increment();
        assert_eq!(view.count(), 1);

        view.increment_modify();
        assert_eq!(view.count(), 2);

        // Modify through binding
        view.enabled.bind().set(true);
        assert_eq!(view.enabled.get(), true);

        // Modify volume
        view.volume.bind().set(75.0);
        assert_eq!(view.volume.get(), 75.0);
    }

    #[test]
    fn test_counter_example() {
        let counter = CounterView::new();
        assert_eq!(counter.get_count(), 0);

        counter.increment();
        assert_eq!(counter.get_count(), 1);

        counter.increment();
        assert_eq!(counter.get_count(), 2);
    }
}
