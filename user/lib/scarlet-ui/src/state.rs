//! Reactive state management for automatic view updates
//!
//! This module provides SwiftUI-style state management with automatic
//! view updates when state changes.
//!
//! # Example
//!
//! ```no_run
//! use scarlet_ui::{State, Label, Button, VStack};
//!
//! let counter = State::new(0);
//! let counter_clone = counter.clone();
//!
//! VStack::new()
//!     .child(Label::new(format!("Count: {}", counter.get())))
//!     .child(Button::new("Increment", move || {
//!         counter_clone.set(counter_clone.get() + 1);
//!     }))
//! ```

use scarlet_std::sync::{Arc, Mutex};
use scarlet_std::vec::Vec;
use scarlet_std::boxed::Box;

/// Callback type for state change notifications
type StateCallback = Box<dyn FnMut() + Send + 'static>;

/// Internal state container with change notification
struct StateInner<T> {
    value: T,
    callbacks: Vec<StateCallback>,
}

impl<T> StateInner<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            callbacks: Vec::new(),
        }
    }

    fn set(&mut self, new_value: T) {
        self.value = new_value;
        // Notify all observers
        for callback in &mut self.callbacks {
            callback();
        }
    }

    fn get(&self) -> &T {
        &self.value
    }

    fn add_callback(&mut self, callback: StateCallback) {
        self.callbacks.push(callback);
    }
}

/// Reactive state that triggers view updates on changes
///
/// `State` is similar to SwiftUI's `@State` property wrapper.
/// When the value changes, all observers are notified.
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::State;
///
/// let counter = State::new(0);
/// counter.set(counter.get() + 1);
/// assert_eq!(counter.get(), 1);
/// ```
#[derive(Clone)]
pub struct State<T: Clone> {
    inner: Arc<Mutex<StateInner<T>>>,
}

impl<T: Clone> State<T> {
    /// Create a new state with an initial value
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(StateInner::new(value))),
        }
    }

    /// Get the current value (clones the value)
    pub fn get(&self) -> T {
        self.inner.lock().get().clone()
    }

    /// Set a new value and notify observers
    pub fn set(&self, value: T) {
        self.inner.lock().set(value);
    }

    /// Modify the value in place
    pub fn modify<F>(&self, f: F)
    where
        F: FnOnce(&mut T),
    {
        let mut inner = self.inner.lock();
        f(&mut inner.value);
        // Notify all observers
        for callback in &mut inner.callbacks {
            callback();
        }
    }

    /// Register a callback to be called when the state changes
    pub fn on_change<F>(&self, callback: F)
    where
        F: FnMut() + Send + 'static,
    {
        self.inner.lock().add_callback(Box::new(callback));
    }

    /// Create a binding for two-way data flow
    pub fn binding(&self) -> Binding<T> {
        Binding::new(self.clone())
    }
}

/// Two-way binding for connecting state to UI controls
///
/// Similar to SwiftUI's `Binding`, this allows UI controls
/// to both read and write state values.
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{State, TextField};
///
/// let text = State::new(String::from(""));
/// let binding = text.binding();
///
/// // TextField can read and write through the binding
/// TextField::new("Enter text...")
///     .bind(binding)
/// ```
pub struct Binding<T: Clone> {
    state: State<T>,
}

impl<T: Clone> Binding<T> {
    fn new(state: State<T>) -> Self {
        Self { state }
    }

    /// Get the current value
    pub fn get(&self) -> T {
        self.state.get()
    }

    /// Set a new value
    pub fn set(&self, value: T) {
        self.state.set(value);
    }
}

impl<T: Clone> Clone for Binding<T> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

/// Observable value for reactive UI updates
///
/// This trait allows values to be observed for changes.
pub trait Observable<T> {
    /// Get the current value
    fn get(&self) -> T;
    
    /// Set a new value
    fn set(&self, value: T);
    
    /// Register a change callback
    fn on_change<F: FnMut() + Send + 'static>(&self, callback: F);
}

impl<T: Clone> Observable<T> for State<T> {
    fn get(&self) -> T {
        self.get()
    }
    
    fn set(&self, value: T) {
        self.set(value)
    }
    
    fn on_change<F: FnMut() + Send + 'static>(&self, callback: F) {
        self.on_change(callback)
    }
}
