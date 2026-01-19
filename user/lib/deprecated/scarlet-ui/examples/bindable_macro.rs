//! #[bindable] Macro Usage Example
//!
//! This example demonstrates the #[bindable] macro for automatic data binding.

extern crate alloc;
use alloc::string::String;
use alloc::sync::Arc;

use scarlet_ui::{bindable, DataContext};
use scarlet_ui::view::id::ViewId;

/// Example 1: Using #[bindable] macro
///
/// This automatically generates:
/// - `data: Option<Arc<DataContext<bool>>>` field
/// - `bind()` method that subscribes to DataContext
#[bindable(bool)]
struct Toggle {
    #[id]
    id: ViewId,
    #[value]
    is_on: bool,
    label: Option<String>,
}

impl Toggle {
    /// Create a new toggle
    pub fn new(is_on: bool) -> Self {
        Self {
            id: ViewId::new(),
            is_on,
            label: None,
        }
    }

    /// Get the toggle state
    pub fn is_on(&self) -> bool {
        self.is_on
    }
}

/// Example 2: Slider with bindable
#[bindable(f32)]
struct Slider {
    #[id]
    id: ViewId,
    #[value]
    value: f32,
    minimum: f32,
    maximum: f32,
}

impl Slider {
    /// Create a new slider
    pub fn new(minimum: f32, maximum: f32) -> Self {
        Self {
            id: ViewId::new(),
            value: minimum,
            minimum,
            maximum,
        }
    }

    /// Set the value (also updates DataContext if bound)
    pub fn set_value(&mut self, mut value: f32) {
        value = value.clamp(self.minimum, self.maximum);
        self.value = value;
        // Update bound DataContext if present
        if let Some(ref data) = self.data {
            data.set(value);
        }
    }
}

/// Example 3: TextField with bindable
#[bindable(String)]
struct TextField {
    #[id]
    id: ViewId,
    #[value]
    text: String,
    placeholder: String,
}

impl TextField {
    /// Create a new text field
    pub fn new() -> Self {
        Self {
            id: ViewId::new(),
            text: String::new(),
            placeholder: String::new(),
        }
    }

    /// Set the text (also updates DataContext if bound)
    pub fn set_text(&mut self, text: String) {
        self.text = text.clone();
        // Update bound DataContext if present
        if let Some(ref data) = self.data {
            data.set(text);
        }
    }
}

/// Example usage
fn example_usage() {
    // Create DataContext
    let enabled = Arc::new(DataContext::new(false));
    let volume = Arc::new(DataContext::new(50.0));

    // Create controls and bind to data
    let toggle = Toggle::new(false).bind(&enabled);
    let slider = Slider::new(0.0, 100.0).bind(&volume);

    // When data changes, controls are automatically redrawn
    enabled.set(true);
    volume.set(75.0);

    // Controls can update the data
    // toggle.set_on(true);  // Would call enabled.set(true)
    // slider.set_value(80.0);  // Would call volume.set(80.0)
}

/// Comparison: Manual vs Macro
///
/// BEFORE (Manual implementation):
/// ```ignore
/// struct Toggle {
///     id: ViewId,
///     is_on: bool,
///     label: Option<String>,
///     data: Option<Arc<DataContext<bool>>>,  // Manual
/// }
///
/// impl Toggle {
///     pub fn bind(mut self, data: &Arc<DataContext<bool>>) -> Self {  // Manual
///         self.is_on = data.get();
///         data.subscribe(self.id);  // Manual
///         self.data = Some(Arc::clone(data));  // Manual
///         self
///     }
/// }
/// ```
///
/// AFTER (With macro):
/// ```ignore
/// #[bindable(bool)]
/// struct Toggle {
///     #[id]
///     id: ViewId,
///     #[value]
///     is_on: bool,
///     label: Option<String>,
///     // data field generated automatically!
/// }
/// // bind() method generated automatically!
/// ```

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bindable_toggle() {
        let data = Arc::new(DataContext::new(false));
        let toggle = Toggle::new(true).bind(&data);

        // Initial value from DataContext
        assert_eq!(toggle.is_on(), false);

        // Toggle is subscribed
        assert!(data.is_dirty(toggle.id));
    }

    #[test]
    fn test_bindable_slider() {
        let data = Arc::new(DataContext::new(50.0));
        let slider = Slider::new(0.0, 100.0).bind(&data);

        // Initial value from DataContext
        assert_eq!(slider.value, 50.0);

        // set_value updates DataContext
        let mut slider = slider;
        slider.set_value(75.0);
        assert_eq!(data.get(), 75.0);
    }

    #[test]
    fn test_bindable_text_field() {
        let data = Arc::new(DataContext::new(String::from("hello")));
        let mut tf = TextField::new().bind(&data);

        // Initial value from DataContext
        assert_eq!(tf.text, "hello");

        // set_text updates DataContext
        tf.set_text(String::from("world"));
        assert_eq!(data.get(), "world");
    }
}
