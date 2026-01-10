//! Reactive state management for automatic view updates
//!
//! This module provides SwiftUI-style state management with automatic
//! view updates when state changes. State and Binding work together
//! to provide a reactive, declarative UI paradigm.
//!
//! # Key Features
//!
//! - **Deref access**: Read values directly without `.get()` in most contexts
//! - **Automatic updates**: Views subscribed to state are automatically refreshed
//! - **Two-way binding**: Binding allows UI controls to read and write values
//! - **Selective updates**: Only views that depend on changed state are redrawn
//!
//! # Example
//!
//! ```no_run
//! use scarlet_ui::{State, Binding, ReactiveLabel, Button, VStack};
//!
//! let counter = State::new(0);
//!
//! VStack::new()
//!     .child(ReactiveLabel::new(counter.clone(), |c| format!("Count: {}", c)))
//!     .child(Button::new("Increment", {
//!         let counter = counter.clone();
//!         move || counter.update(|c| *c += 1)
//!     }))
//! ```

use scarlet_std::sync::Mutex;
use scarlet_std::vec::Vec;
use scarlet_std::boxed::Box;
use core::ops::Deref;
use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

// We need Arc and Weak from alloc for weak references
extern crate alloc;
use alloc::sync::{Arc, Weak};

/// Global view refresh coordinator
/// 
/// This tracks which views need to be refreshed when state changes.
/// Views register themselves with their associated state, and when
/// state changes, only the registered views are marked for redraw.
static VIEW_REFRESH_QUEUE: Mutex<Vec<ViewRefreshHandle>> = Mutex::new(Vec::new());

/// Handle to request a view refresh
#[derive(Clone)]
pub struct ViewRefreshHandle {
    #[allow(dead_code)]
    id: u64,
    needs_refresh: Arc<AtomicBool>,
}

impl ViewRefreshHandle {
    /// Create a new refresh handle
    pub fn new() -> Self {
        static NEXT_ID: Mutex<u64> = Mutex::new(1);
        let id = {
            let mut next = NEXT_ID.lock();
            let id = *next;
            *next += 1;
            id
        };
        Self {
            id,
            needs_refresh: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Mark this view as needing refresh
    pub fn mark_dirty(&self) {
        self.needs_refresh.store(true, Ordering::SeqCst);
    }

    /// Check and clear dirty flag
    pub fn take_dirty(&self) -> bool {
        self.needs_refresh.swap(false, Ordering::SeqCst)
    }

    /// Check if dirty without clearing
    pub fn is_dirty(&self) -> bool {
        self.needs_refresh.load(Ordering::SeqCst)
    }
}

impl Default for ViewRefreshHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Callback type for state change notifications
type StateCallback = Box<dyn FnMut() + Send + 'static>;

/// Subscription ID for unsubscribing from state changes
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubscriptionId(u64);

/// Internal state container with change notification
struct StateInner<T> {
    value: T,
    callbacks: Vec<(u64, StateCallback)>,
    next_callback_id: u64,
    /// Weak references to view refresh handles
    view_handles: Vec<Weak<AtomicBool>>,
}

impl<T> StateInner<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            callbacks: Vec::new(),
            next_callback_id: 1,
            view_handles: Vec::new(),
        }
    }

    fn notify_observers(&mut self) {
        // Notify callbacks
        for (_, callback) in &mut self.callbacks {
            callback();
        }
        
        // Mark all subscribed view handles as dirty
        // Clean up dropped handles
        self.view_handles.retain(|weak: &Weak<AtomicBool>| {
            if let Some(strong) = weak.upgrade() {
                strong.store(true, Ordering::SeqCst);
                true
            } else {
                false
            }
        });
    }

    fn add_callback(&mut self, callback: StateCallback) -> u64 {
        let id = self.next_callback_id;
        self.next_callback_id += 1;
        self.callbacks.push((id, callback));
        id
    }

    fn remove_callback(&mut self, id: u64) {
        self.callbacks.retain(|(cid, _)| *cid != id);
    }

    fn subscribe_view(&mut self, handle: &ViewRefreshHandle) {
        self.view_handles.push(Arc::downgrade(&handle.needs_refresh));
    }
}

/// Reactive state that triggers view updates on changes
///
/// `State<T>` is similar to SwiftUI's `@State` property wrapper.
/// When the value changes, all observers are notified and subscribed
/// views are marked for redraw.
///
/// # Direct Access
///
/// State implements `Deref` so you can access the value directly
/// in many contexts without calling `.get()`:
///
/// ```no_run
/// use scarlet_ui::State;
///
/// let name = State::new(String::from("Hello"));
/// println!("Length: {}", name.len()); // Deref to &String
/// ```
///
/// # Cloning
///
/// Cloning a `State` creates another reference to the same underlying
/// value. All clones share the same state and observers.
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::State;
///
/// let counter = State::new(0);
/// 
/// // Update and notify observers
/// counter.update(|c| *c += 1);
/// 
/// // Get current value
/// assert_eq!(*counter, 1);
/// ```
pub struct State<T> {
    inner: Arc<Mutex<StateInner<T>>>,
}

impl<T> State<T> {
    /// Create a new state with an initial value
    pub fn new(value: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(StateInner::new(value))),
        }
    }

    /// Update the value and notify observers
    ///
    /// This is the primary way to modify state. The closure receives
    /// a mutable reference to the value, and observers are notified
    /// after the closure completes.
    ///
    /// # Example
    ///
    /// ```no_run
    /// let counter = State::new(0);
    /// counter.update(|c| *c += 1);
    /// ```
    pub fn update<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        let mut inner = self.inner.lock();
        let result = f(&mut inner.value);
        inner.notify_observers();
        result
    }

    /// Set a new value and notify observers
    pub fn set(&self, value: T) {
        self.update(|v| *v = value);
    }

    /// Register a callback to be called when the state changes
    ///
    /// Returns a subscription ID that can be used to unsubscribe.
    pub fn subscribe<F>(&self, callback: F) -> SubscriptionId
    where
        F: FnMut() + Send + 'static,
    {
        let id = self.inner.lock().add_callback(Box::new(callback));
        SubscriptionId(id)
    }

    /// Unsubscribe a previously registered callback
    pub fn unsubscribe(&self, id: SubscriptionId) {
        self.inner.lock().remove_callback(id.0);
    }

    /// Subscribe a view handle to receive refresh notifications
    ///
    /// When this state changes, the view will be marked as needing redraw.
    pub fn subscribe_view(&self, handle: &ViewRefreshHandle) {
        self.inner.lock().subscribe_view(handle);
    }

    /// Create a binding for two-way data flow
    pub fn binding(&self) -> Binding<T>
    where
        T: Clone + Send + Sync + 'static,
    {
        Binding::from_state(self.clone())
    }

    /// Access the value with a closure (useful when Deref doesn't work)
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        f(&self.inner.lock().value)
    }

    /// Map the current value to a new value
    pub fn map<F, U>(&self, f: F) -> U
    where
        F: FnOnce(&T) -> U,
    {
        self.with(f)
    }
}

impl<T: Clone> State<T> {
    /// Get a clone of the current value
    ///
    /// This is useful when you need an owned copy of the value.
    pub fn get(&self) -> T {
        self.inner.lock().value.clone()
    }

    /// Legacy modify method (use `update` instead)
    #[deprecated(note = "Use `update` instead")]
    pub fn modify<F>(&self, f: F)
    where
        F: FnOnce(&mut T),
    {
        self.update(f);
    }

    /// Legacy on_change method (use `subscribe` instead)
    #[deprecated(note = "Use `subscribe` instead")]
    pub fn on_change<F>(&self, callback: F)
    where
        F: FnMut() + Send + 'static,
    {
        self.subscribe(callback);
    }
}

impl<T> Clone for State<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Guard for accessing state value
/// 
/// This provides Deref access to the underlying value while holding the lock.
pub struct StateGuard<'a, T> {
    inner: scarlet_std::sync::MutexGuard<'a, StateInner<T>>,
}

impl<'a, T> Deref for StateGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner.value
    }
}

impl<T> State<T> {
    /// Borrow the state value
    /// 
    /// Returns a guard that provides Deref access to the value.
    /// The lock is held for the duration of the guard's lifetime.
    pub fn borrow(&self) -> StateGuard<'_, T> {
        StateGuard {
            inner: self.inner.lock(),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for State<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("State")
            .field("value", &*self.borrow())
            .finish()
    }
}

impl<T: fmt::Display> fmt::Display for State<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&*self.borrow(), f)
    }
}

impl<T: Default> Default for State<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

// ============================================================================
// Binding - Two-way data binding
// ============================================================================

/// Two-way binding for connecting state to UI controls
///
/// `Binding<T>` is similar to SwiftUI's `Binding`. It allows UI controls
/// to both read and write state values, creating a bidirectional data flow.
///
/// Bindings can be created from:
/// - A `State<T>` using `state.binding()`
/// - Custom getter/setter closures using `Binding::new(get, set)`
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{State, Binding, TextField};
///
/// let text = State::new(String::from(""));
///
/// // Create a binding from state
/// let binding = text.binding();
///
/// // TextField can read and write through the binding
/// TextField::new("Enter text...").bind(binding)
/// ```
pub struct Binding<T> {
    getter: Arc<dyn Fn() -> T + Send + Sync>,
    setter: Arc<dyn Fn(T) + Send + Sync>,
}

impl<T: Clone + Send + Sync + 'static> Binding<T> {
    /// Create a binding from a State
    pub fn from_state(state: State<T>) -> Self {
        let getter_state = state.clone();
        let setter_state = state;
        
        Self {
            getter: Arc::new(move || getter_state.get()),
            setter: Arc::new(move |value| setter_state.set(value)),
        }
    }
}

impl<T: 'static> Binding<T> {
    /// Create a custom binding with getter and setter closures
    ///
    /// # Example
    ///
    /// ```no_run
    /// use scarlet_ui::Binding;
    /// use std::sync::{Arc, Mutex};
    ///
    /// let data = Arc::new(Mutex::new(42));
    /// let data_get = data.clone();
    /// let data_set = data.clone();
    ///
    /// let binding = Binding::custom(
    ///     move || *data_get.lock().unwrap(),
    ///     move |v| *data_set.lock().unwrap() = v,
    /// );
    /// ```
    pub fn custom<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn() -> T + Send + Sync + 'static,
        S: Fn(T) + Send + Sync + 'static,
    {
        Self {
            getter: Arc::new(getter),
            setter: Arc::new(setter),
        }
    }

    /// Get the current value
    pub fn get(&self) -> T {
        (self.getter)()
    }

    /// Set a new value
    pub fn set(&self, value: T) {
        (self.setter)(value)
    }

    /// Update the value using a closure
    pub fn update<F>(&self, f: F)
    where
        T: Clone,
        F: FnOnce(&mut T),
    {
        let mut value = self.get();
        f(&mut value);
        self.set(value);
    }
}

impl<T> Clone for Binding<T> {
    fn clone(&self) -> Self {
        Self {
            getter: self.getter.clone(),
            setter: self.setter.clone(),
        }
    }
}

impl<T: fmt::Debug + 'static> fmt::Debug for Binding<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Binding")
            .field("value", &self.get())
            .finish()
    }
}

// ============================================================================
// Computed - Derived reactive values
// ============================================================================

/// A computed value derived from one or more State values
///
/// Computed values are automatically recalculated when their
/// dependencies change.
///
/// # Example
///
/// ```no_run
/// use scarlet_ui::{State, Computed};
///
/// let first_name = State::new(String::from("John"));
/// let last_name = State::new(String::from("Doe"));
///
/// let full_name = Computed::new({
///     let first = first_name.clone();
///     let last = last_name.clone();
///     move || format!("{} {}", first.get(), last.get())
/// });
///
/// assert_eq!(full_name.get(), "John Doe");
/// ```
pub struct Computed<T> {
    compute: Arc<dyn Fn() -> T + Send + Sync>,
}

impl<T: 'static> Computed<T> {
    /// Create a new computed value
    pub fn new<F>(compute: F) -> Self
    where
        F: Fn() -> T + Send + Sync + 'static,
    {
        Self {
            compute: Arc::new(compute),
        }
    }

    /// Get the computed value
    pub fn get(&self) -> T {
        (self.compute)()
    }
}

impl<T> Clone for Computed<T> {
    fn clone(&self) -> Self {
        Self {
            compute: self.compute.clone(),
        }
    }
}

// ============================================================================
// Observable trait
// ============================================================================

/// Observable value for reactive UI updates
///
/// This trait allows values to be observed for changes.
pub trait Observable<T> {
    /// Get the current value
    fn get(&self) -> T;
    
    /// Set a new value
    fn set(&self, value: T);
    
    /// Register a change callback
    fn subscribe<F: FnMut() + Send + 'static>(&self, callback: F) -> SubscriptionId;
}

impl<T: Clone> Observable<T> for State<T> {
    fn get(&self) -> T {
        self.get()
    }
    
    fn set(&self, value: T) {
        self.set(value)
    }
    
    fn subscribe<F: FnMut() + Send + 'static>(&self, callback: F) -> SubscriptionId {
        self.subscribe(callback)
    }
}

// ============================================================================
// Process view refresh queue
// ============================================================================

/// Check if there are any pending view refreshes
pub fn has_pending_refreshes() -> bool {
    !VIEW_REFRESH_QUEUE.lock().is_empty()
}

/// Get and clear all pending refresh handles
pub fn take_pending_refreshes() -> Vec<ViewRefreshHandle> {
    core::mem::take(&mut *VIEW_REFRESH_QUEUE.lock())
}
