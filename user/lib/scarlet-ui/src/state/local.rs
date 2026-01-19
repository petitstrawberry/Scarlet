//! Local state - View-owned local state (@State equivalent)
//!
//! Local<T> represents state owned by a View, equivalent to SwiftUI's @State.
//!
//! # Example
//!
//! ```ignore
//! struct MyView {
//!     counter: Local<u32>,  // @State equivalent
//! }
//!
//! impl MyView {
//!     fn new() -> Self {
//!         Self {
//!             counter: Local::new(0),
//!         }
//!     }
//!
//!     fn build(&self) -> impl View {
//!         // Use .bind() to create a binding ($counter equivalent)
//!         VStack::new()
//!             .child(Text::new(format!("Count: {}", self.counter.get())))
//!             .child(Button::new("Increment").action(|| {
//!                 self.counter.set(self.counter.get() + 1);
//!             }))
//!     }
//! }
//! ```

extern crate alloc;
use alloc::sync::Arc;

use crate::state::data::DataContext;

/// View-owned local state (equivalent to SwiftUI's @State)
///
/// `Local<T>` is used for value types owned by a View.
/// The View creates the data and is responsible for its lifetime.
///
/// # Type Parameters
///
/// * `T` - The data type (must be Clone)
pub struct Local<T: Clone> {
    inner: Arc<DataContext<T>>,
}

impl<T: Clone> Local<T> {
    /// Create a new local state with an initial value
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(DataContext::new(value)),
        }
    }

    /// Get the current value
    pub fn get(&self) -> T {
        self.inner.get()
    }

    /// Read the value with a closure (more efficient than cloning)
    pub fn read<U, F>(&self, f: F) -> U
    where
        F: FnOnce(&T) -> U,
    {
        self.inner.read(f)
    }

    /// Set a new value
    pub fn set(&self, value: T)
    where
        T: PartialEq,
    {
        self.inner.set(value)
    }

    /// Modify the value
    pub fn modify<U, F>(&self, f: F) -> U
    where
        F: FnOnce(&mut T) -> U,
    {
        self.inner.modify(f)
    }

    /// Create a binding to this state ($operator equivalent)
    ///
    /// Returns an Arc that can be passed to child views.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let enabled = Local::new(false);
    /// let toggle = Toggle::new("Enable").bind(enabled.bind());
    /// ```
    pub fn bind(&self) -> Arc<DataContext<T>> {
        Arc::clone(&self.inner)
    }

    /// Get the underlying DataContext (advanced usage)
    pub fn data(&self) -> &Arc<DataContext<T>> {
        &self.inner
    }
}

impl<T: Clone> Clone for Local<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_new() {
        let local = Local::new(42);
        assert_eq!(local.get(), 42);
    }

    #[test]
    fn test_local_set() {
        let local = Local::new(42);
        local.set(100);
        assert_eq!(local.get(), 100);
    }

    #[test]
    fn test_local_modify() {
        let local = Local::new(42);
        local.modify(|v| *v += 1);
        assert_eq!(local.get(), 43);
    }

    #[test]
    fn test_local_bind() {
        let local = Local::new(42);
        let binding = local.bind();

        // Binding should give access to the same data
        assert_eq!(binding.get(), 42);

        // Changes through binding should reflect in local
        binding.set(100);
        assert_eq!(local.get(), 100);
        assert_eq!(binding.get(), 100);

        // Changes through local should reflect in binding
        local.set(200);
        assert_eq!(binding.get(), 200);
    }

    #[test]
    fn test_local_clone() {
        let local1 = Local::new(42);
        let local2 = local1.clone();

        assert_eq!(local1.get(), 42);
        assert_eq!(local2.get(), 42);

        local1.set(100);
        assert_eq!(local1.get(), 100);
        assert_eq!(local2.get(), 100); // Same underlying data
    }

    #[test]
    fn test_local_read() {
        let local = Local::new(42);
        let result = local.read(|v| *v * 2);
        assert_eq!(result, 84);
    }
}
