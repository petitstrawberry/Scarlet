//! State management modules
//!
//! This module provides state management for ScarletUI.

// New data-first architecture
pub mod data;
pub mod observable;

// Re-exports
pub use data::{DataContext, Lens, FnLens};
pub use observable::{Observable, ObservableBuilder};

// Legacy State type (for backward compatibility)
// This will be deprecated once the new architecture is complete
extern crate alloc;
use alloc::sync::Arc;

use scarlet_std::sync::Mutex;
use core::ops::Deref;
use core::fmt;

/// Legacy State type for backward compatibility
///
/// This is a simplified version that will be deprecated once the new
/// DataContext-based architecture is complete.
pub struct State<T> {
    inner: Arc<Mutex<T>>,
}

impl<T> State<T> {
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(value)),
        }
    }

    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.inner.lock().clone()
    }

    pub fn set(&self, value: T) {
        *self.inner.lock() = value;
    }

    pub fn update<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        let mut guard = self.inner.lock();
        f(&mut guard)
    }

    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let guard = self.inner.lock();
        f(&guard)
    }
}

impl<T> Clone for State<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for State<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("value", &*self.inner.lock())
            .finish()
    }
}

impl<T: Default> Default for State<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> Deref for State<T> {
    type Target = Mutex<T>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
