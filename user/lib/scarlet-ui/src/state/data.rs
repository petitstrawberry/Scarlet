//! DataContext - Core reactive data storage
//!
//! DataContext<T> provides thread-safe reactive storage with change notification.
//! This is the foundation for all state management in ScarletUI.
//!
//! # SwiftUI-Style Automatic Invalidation
//!
//! When you bind a control to DataContext, it automatically subscribes to changes.
//! When the data changes, all subscribed views are marked dirty and will be redrawn.

extern crate alloc;
use alloc::sync::Arc;
use alloc::vec::Vec;
use scarlet_std::sync::Mutex;
use scarlet_std::collections::HashSet;
use crate::view::id::ViewId;
use crate::view::tracker::global_tracker;

/// Shared data context reference
///
/// Type alias for `Arc<DataContext<T>>` to reduce verbosity.
///
/// # Example
///
/// ```ignore
/// let enabled: SharedData<bool> = Arc::new(DataContext::new(false));
/// let toggle = Toggle::new("Enable").bind(&enabled);
/// ```
pub type SharedData<T> = Arc<DataContext<T>>;

/// Data context for managing reactive state
///
/// DataContext<T> stores a value and automatically invalidates views when it changes.
/// This is the SwiftUI-style approach: data changes automatically trigger redraws.
///
/// # Example
///
/// ```ignore
/// let enabled = Local::new(false);
///
/// // Toggle binds to the data and subscribes
/// let toggle = Toggle::new("Enable").bind(&enabled.bind());
///
/// // When Toggle is clicked:
/// enabled.set(true);  // ← Toggle is automatically redrawn
/// ```
pub struct DataContext<T> {
    inner: Arc<Mutex<DataInner<T>>>,
}

struct DataInner<T> {
    /// The current data value
    data: T,
    /// Data version (increments on each change)
    version: u64,
    /// Views that are observing this data (subscribeしたView)
    observers: HashSet<ViewId>,
}

impl<T> DataContext<T> {
    /// Create a new data context with an initial value
    pub fn new(data: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DataInner {
                data,
                version: 0,
                observers: HashSet::new(),
            })),
        }
    }

    /// Get the current value (clones for value types)
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.inner.lock().data.clone()
    }

    /// Read the value with a closure (more efficient)
    pub fn read<U, F>(&self, f: F) -> U
    where
        F: FnOnce(&T) -> U,
    {
        let inner = self.inner.lock();
        f(&inner.data)
    }

    /// Set a new value (automatically marks observers dirty)
    ///
    /// This is the SwiftUI-style approach: changing data automatically
    /// invalidates all views that observe it.
    pub fn set(&self, data: T)
    where
        T: PartialEq,
    {
        let observers = {
            let mut inner = self.inner.lock();
            if inner.data != data {
                inner.data = data;
                inner.version = inner.version.wrapping_add(1);

                // Mark all observers as dirty (SwiftUI-style automatic invalidation)
                inner.observers.iter().copied().collect::<Vec<_>>()
            } else {
                return;
            }
        };

        // Mark observers dirty in the global RenderTracker
        // (must be done outside the lock to avoid deadlock with global tracker)
        let tracker = global_tracker();
        for view_id in observers {
            tracker.mark_dirty_paint(view_id);
        }
    }

    /// Modify the value (automatically marks observers dirty)
    pub fn modify<U, F>(&self, f: F) -> U
    where
        F: FnOnce(&mut T) -> U,
    {
        let (result, observers) = {
            let mut inner = self.inner.lock();
            let result = f(&mut inner.data);
            inner.version = inner.version.wrapping_add(1);

            // Collect observers to mark dirty
            let observers = inner.observers.iter().copied().collect::<Vec<_>>();
            (result, observers)
        };

        // Mark observers dirty in the global RenderTracker
        // (must be done outside the lock to avoid deadlock with global tracker)
        let tracker = global_tracker();
        for view_id in observers {
            tracker.mark_dirty_paint(view_id);
        }

        result
    }

    /// Get the current version
    pub fn version(&self) -> u64 {
        self.inner.lock().version
    }

    /// Subscribe a view to observe this data
    ///
    /// Called automatically by `.bind()` on controls.
    /// When the data changes, this view will be marked dirty.
    pub fn subscribe(&self, view_id: ViewId) {
        let mut inner = self.inner.lock();
        inner.observers.insert(view_id);
    }

    /// Unsubscribe a view from this data
    pub fn unsubscribe(&self, view_id: ViewId) {
        let mut inner = self.inner.lock();
        inner.observers.remove(&view_id);
        // Note: dirty flag is managed by global RenderTracker now
    }

    /// Get the number of observers
    pub fn observer_count(&self) -> usize {
        self.inner.lock().observers.len()
    }
}

impl<T: Clone> Clone for DataContext<T> {
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
    fn test_data_context() {
        let ctx = DataContext::new(42);
        assert_eq!(ctx.get(), 42);
        assert_eq!(ctx.version(), 0);

        ctx.set(100);
        assert_eq!(ctx.get(), 100);
        assert_eq!(ctx.version(), 1);
    }

    #[test]
    fn test_data_context_modify() {
        let ctx = DataContext::new(42);
        ctx.modify(|v| *v += 1);
        assert_eq!(ctx.get(), 43);
        assert_eq!(ctx.version(), 1);
    }

    #[test]
    fn test_data_context_read() {
        let ctx = DataContext::new(42);
        let result = ctx.read(|v| *v * 2);
        assert_eq!(result, 84);
        // Version should not change on read
        assert_eq!(ctx.version(), 0);
    }

    #[test]
    fn test_subscribe_marks_dirty() {
        let ctx = DataContext::new(42);
        let view_id = ViewId::new();

        // Subscribe
        ctx.subscribe(view_id);

        // Change data
        ctx.set(100);

        // View should be marked dirty in the global tracker
        let tracker = global_tracker();
        let dirty = tracker.take_dirty_paint();
        assert!(dirty.contains(&view_id));
    }

    #[test]
    fn test_unsubscribe() {
        let ctx = DataContext::new(42);
        let view_id = ViewId::new();

        ctx.subscribe(view_id);
        assert_eq!(ctx.observer_count(), 1);

        ctx.unsubscribe(view_id);
        assert_eq!(ctx.observer_count(), 0);

        // Change data - view should NOT be marked dirty
        ctx.set(100);

        let tracker = global_tracker();
        let dirty = tracker.take_dirty_paint();
        assert!(!dirty.contains(&view_id));
    }
}
