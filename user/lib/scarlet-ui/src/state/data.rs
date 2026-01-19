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
    /// Views that are marked dirty (need redraw)
    dirty_views: HashSet<ViewId>,
}

impl<T> DataContext<T> {
    /// Create a new data context with an initial value
    pub fn new(data: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DataInner {
                data,
                version: 0,
                observers: HashSet::new(),
                dirty_views: HashSet::new(),
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
        let mut inner = self.inner.lock();
        if inner.data != data {
            inner.data = data;
            inner.version = inner.version.wrapping_add(1);

            // Mark all observers as dirty (SwiftUI-style automatic invalidation)
            let observers: Vec<ViewId> = inner.observers.iter().copied().collect();
            for view_id in observers {
                inner.dirty_views.insert(view_id);
            }
        }
    }

    /// Modify the value (automatically marks observers dirty)
    pub fn modify<U, F>(&self, f: F) -> U
    where
        F: FnOnce(&mut T) -> U,
    {
        let mut inner = self.inner.lock();
        let result = f(&mut inner.data);
        inner.version = inner.version.wrapping_add(1);

        // Mark all observers as dirty
        let observers: Vec<ViewId> = inner.observers.iter().copied().collect();
        for view_id in observers {
            inner.dirty_views.insert(view_id);
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
        inner.dirty_views.remove(&view_id);
    }

    /// Get all dirty views (views that need redraw)
    ///
    /// Called by the Application render loop.
    pub fn take_dirty_views(&self) -> HashSet<ViewId> {
        let mut inner = self.inner.lock();
        let dirty = core::mem::take(&mut inner.dirty_views);
        dirty
    }

    /// Check if a specific view is dirty
    pub fn is_dirty(&self, view_id: ViewId) -> bool {
        self.inner.lock().dirty_views.contains(&view_id)
    }

    /// Clear dirty flag for a specific view
    ///
    /// Called after the view is redrawn.
    pub fn clear_dirty(&self, view_id: ViewId) {
        let mut inner = self.inner.lock();
        inner.dirty_views.remove(&view_id);
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

        // View should be marked dirty
        assert!(ctx.is_dirty(view_id));
    }

    #[test]
    fn test_take_dirty_views() {
        let ctx = DataContext::new(42);
        let view1 = ViewId::new();
        let view2 = ViewId::new();

        ctx.subscribe(view1);
        ctx.subscribe(view2);

        // Change data
        ctx.set(100);

        // Both views should be dirty
        let dirty = ctx.take_dirty_views();
        assert_eq!(dirty.len(), 2);
        assert!(dirty.contains(&view1));
        assert!(dirty.contains(&view2));

        // After take_dirty, should be empty
        assert!(!ctx.is_dirty(view1));
        assert!(!ctx.is_dirty(view2));
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
        assert!(!ctx.is_dirty(view_id));
    }

    #[test]
    fn test_clear_dirty() {
        let ctx = DataContext::new(42);
        let view_id = ViewId::new();

        ctx.subscribe(view_id);
        ctx.set(100);

        assert!(ctx.is_dirty(view_id));

        ctx.clear_dirty(view_id);
        assert!(!ctx.is_dirty(view_id));
    }
}
