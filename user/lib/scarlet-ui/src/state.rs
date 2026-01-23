//! State management for ScarletUI
//!
//! Provides reactive state management with subscription notifications.

use alloc::sync::Arc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;
use core::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

/// Unique identifier for State instances
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StateId(u32);

impl StateId {
    /// Create a new StateId from a raw value
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    /// Get the raw ID value
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Global counter for generating unique StateIds
static STATE_ID_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Generate a new unique StateId
pub fn generate_state_id() -> StateId {
    let id = STATE_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    StateId(id)
}

/// Trait for types that can be subscribed to for change notifications
pub trait Listenable: Any {
    /// Subscribe to any changes in this listenable
    fn subscribe_any(&self, callback: Box<dyn Fn(&dyn Any) + Send + Sync>);
}

impl<T: Any> Listenable for State<T> {
    fn subscribe_any(&self, callback: Box<dyn Fn(&dyn Any) + Send + Sync>) {
        self.subscribe(Box::new(move |value: &T| callback(value)))
    }
}

/// Callback type for state change notifications
pub type SubscriberCallback<T> = Box<dyn Fn(&T) + Send + Sync>;

/// Inner state data shared across State clones
struct StateInner<T> {
    value: Mutex<T>,
    subscribers: Mutex<Vec<SubscriberCallback<T>>>,
}

impl<T> StateInner<T> {
    fn new(value: T) -> Self {
        Self {
            value: Mutex::new(value),
            subscribers: Mutex::new(Vec::new()),
        }
    }

    fn get(&self) -> T
    where
        T: Clone,
    {
        self.value.lock().clone()
    }

    fn set(&self, value: T) {
        *self.value.lock() = value;
    }

    fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut T),
    {
        let mut value = self.value.lock();
        f(&mut value);
    }

    fn notify(&self)
    where
        T: Clone,
    {
        let value = self.get();
        let subscribers = self.subscribers.lock();
        for callback in subscribers.iter() {
            callback(&value);
        }
    }

    fn subscribe(&self, callback: SubscriberCallback<T>) {
        self.subscribers.lock().push(callback);
    }
}

/// Reactive state container with subscription notifications
///
/// State<T> is a reference-counted container that can be cloned and shared.
/// When the value is updated, all subscribers are notified.
#[derive(Clone)]
pub struct State<T> {
    id: StateId,
    inner: Arc<StateInner<T>>,
}

impl<T> State<T> {
    /// Create a new State with an initial value and auto-generated ID
    pub fn initial(value: T) -> Self {
        Self {
            id: generate_state_id(),
            inner: Arc::new(StateInner::new(value)),
        }
    }

    /// Create a new State with a specific ID and initial value
    pub fn with_id(id: StateId, value: T) -> Self {
        Self {
            id,
            inner: Arc::new(StateInner::new(value)),
        }
    }

    /// Get the State's unique ID
    pub fn id(&self) -> StateId {
        self.id
    }

    /// Get a clone of the current value
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.inner.get()
    }

    /// Set a new value and notify subscribers
    pub fn set(&self, value: T)
    where
        T: Clone,
    {
        self.inner.set(value);
        self.inner.notify();
    }

    /// Update the value in place and notify subscribers
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut T),
        T: Clone,
    {
        self.inner.update(f);
        self.inner.notify();
    }

    /// Subscribe to value changes
    pub fn subscribe(&self, callback: SubscriberCallback<T>) {
        self.inner.subscribe(callback);
    }
}

impl<T: core::fmt::Debug> core::fmt::Debug for State<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("State")
            .field("id", &self.id)
            .field("value", &"::<T>")
            .finish()
    }
}
