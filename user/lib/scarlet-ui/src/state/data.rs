//! Data context for data-first MVC architecture
//!
//! This module provides DataContext<T>, which is the core of the new
//! data-first architecture (inspired by Druid and Xilem).
//!
//! # Key Concepts
//!
//! - **Single source of truth**: All application data flows through DataContext
//! - **Automatic invalidation**: Data changes automatically mark views dirty
//! - **Unidirectional flow**: Data flows down, events flow up
//! - **Version tracking**: Each data change increments a version for change detection

use crate::view::id::ViewId;
use crate::view::tracker::RenderTracker;
use scarlet_std::collections::{HashMap, HashSet};
use scarlet_std::sync::{Arc, Mutex};

/// Data context for managing application state
///
/// DataContext<T> is the single source of truth for application data.
/// It tracks:
/// - The current data value
/// - Which views are observing this data
/// - The data version (for change detection)
/// - Which views are dirty (need redraw)
pub struct DataContext<T> {
    inner: Arc<Mutex<DataContextInner<T>>>,
}

/// Inner state of DataContext (protected by Mutex)
struct DataContextInner<T> {
    /// The current data value
    data: T,
    /// Data version (increments on each change)
    version: u64,
    /// Views that are observing this data
    observers: HashMap<ViewId, ObserverInfo>,
    /// Views that are marked dirty (need redraw)
    dirty_views: HashSet<ViewId>,
}

/// Information about an observing view
struct ObserverInfo {
    /// Last data version this view saw
    last_version: u64,
    /// Whether this view is still active
    active: bool,
}

impl<T> DataContext<T> {
    /// Create a new data context with an initial value
    pub fn new(data: T) -> Self {
        Self {
            inner: Arc::new(Mutex::new(DataContextInner {
                data,
                version: 0,
                observers: HashMap::new(),
                dirty_views: HashSet::new(),
            })),
        }
    }

    /// Get an immutable reference to the data
    ///
    /// This is a read-only operation that doesn't mark views as dirty.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.inner.lock().data.clone()
    }

    /// Read the data with a closure (more efficient than cloning)
    pub fn read<U, F>(&self, f: F) -> U
    where
        F: FnOnce(&T) -> U,
    {
        let inner = self.inner.lock();
        f(&inner.data)
    }

    /// Modify the data and notify observers
    ///
    /// This will:
    /// 1. Apply the modification function to the data
    /// 2. Increment the version
    /// 3. Mark all observing views as dirty
    /// 4. Return the result from the modification function
    pub fn modify<U, F>(&self, f: F) -> U
    where
        F: FnOnce(&mut T) -> U,
    {
        let mut inner = self.inner.lock();
        let result = f(&mut inner.data);
        inner.version = inner.version.wrapping_add(1);

        // Mark all observers as dirty (collect keys first to avoid borrow issues)
        let view_ids: scarlet_std::vec::Vec<ViewId> = inner.observers.keys().copied().collect();
        for view_id in view_ids {
            inner.dirty_views.insert(view_id);
        }

        result
    }

    /// Modify the data and notify observers via RenderTracker
    ///
    /// This is the AppKit-style notification method. It modifies the data AND
    /// immediately notifies the RenderTracker, eliminating the need for
    /// recursive traversal.
    ///
    /// This will:
    /// 1. Apply the modification function to the data
    /// 2. Increment the version
    /// 3. Notify RenderTracker for all observing views (O(1) per view)
    /// 4. Return the result from the modification function
    ///
    /// # Arguments
    ///
    /// * `tracker` - The RenderTracker to notify
    /// * `f` - The modification function
    pub fn modify_with_tracker<U, F>(&self, tracker: &mut RenderTracker, f: F) -> U
    where
        F: FnOnce(&mut T) -> U,
    {
        let mut inner = self.inner.lock();
        let result = f(&mut inner.data);
        inner.version = inner.version.wrapping_add(1);

        // Collect observer IDs and mark them as dirty locally
        let view_ids: scarlet_std::vec::Vec<ViewId> = inner.observers.keys().copied().collect();
        for view_id in &view_ids {
            inner.dirty_views.insert(*view_id);
        }

        // Notify tracker (outside the lock to avoid deadlock)
        drop(inner);
        tracker.mark_dirty_paint_many(view_ids);

        result
    }

    /// Set a new value (replaces the entire data)
    pub fn set(&self, data: T)
    where
        T: PartialEq,
    {
        let mut inner = self.inner.lock();
        if inner.data != data {
            inner.data = data;
            inner.version = inner.version.wrapping_add(1);

            // Mark all observers as dirty (collect keys first to avoid borrow issues)
            let view_ids: scarlet_std::vec::Vec<ViewId> = inner.observers.keys().copied().collect();
            for view_id in view_ids {
                inner.dirty_views.insert(view_id);
            }
        }
    }

    /// Set a new value and notify RenderTracker
    ///
    /// This is the AppKit-style notification method for set operations.
    pub fn set_with_tracker(&self, tracker: &mut RenderTracker, data: T)
    where
        T: PartialEq,
    {
        let view_ids = {
            let mut inner = self.inner.lock();
            if inner.data != data {
                inner.data = data;
                inner.version = inner.version.wrapping_add(1);

                // Collect observer IDs while holding the lock
                inner.observers.keys().copied().collect::<scarlet_std::vec::Vec<_>>()
            } else {
                return; // No change, no notification
            }
        };

        // Notify tracker outside the lock
        tracker.mark_dirty_paint_many(view_ids);
    }

    /// Get the current data version
    pub fn version(&self) -> u64 {
        self.inner.lock().version
    }

    /// Subscribe a view to observe this data
    ///
    /// Returns the initial data version that this view should use.
    pub fn subscribe(&self, view_id: ViewId) -> u64 {
        let mut inner = self.inner.lock();
        let version = inner.version;

        inner.observers.insert(
            view_id,
            ObserverInfo {
                last_version: version,
                active: true,
            },
        );

        version
    }

    /// Unsubscribe a view from this data
    pub fn unsubscribe(&self, view_id: ViewId) {
        let mut inner = self.inner.lock();
        inner.observers.remove(&view_id);
        inner.dirty_views.remove(&view_id);
    }

    /// Check if a view needs an update
    ///
    /// This compares the view's last seen version with the current version.
    pub fn needs_update(&self, view_id: ViewId, last_version: u64) -> bool {
        let inner = self.inner.lock();
        inner.version != last_version || inner.dirty_views.contains(&view_id)
    }

    /// Get the set of dirty views
    pub fn dirty_views(&self) -> scarlet_std::vec::Vec<ViewId> {
        self.inner.lock().dirty_views.iter().copied().collect()
    }

    /// Clear the dirty flag for a specific view
    pub fn clear_dirty(&self, view_id: ViewId) {
        let mut inner = self.inner.lock();
        inner.dirty_views.remove(&view_id);

        // Update the observer's last version (need to store version first)
        let current_version = inner.version;
        if let Some(observer) = inner.observers.get_mut(&view_id) {
            observer.last_version = current_version;
        }
    }

    /// Mark a view as dirty (needs redraw)
    pub fn mark_dirty(&self, view_id: ViewId) {
        self.inner.lock().dirty_views.insert(view_id);
    }
}

impl<T: Clone> Clone for DataContext<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// Lens for focusing on a sub-field of data
///
/// Lenses allow views to observe a subset of the application state,
/// enabling fine-grained reactivity and change detection.
///
/// # Example
///
/// ```ignore
/// struct AppState {
///     counter: CounterState,
///     settings: SettingsState,
/// }
///
/// struct CounterState {
///     value: u32,
/// }
///
/// let app_data = DataContext::new(AppState { ... });
/// let counter_lens = Lens::new(|app| &app.counter, |app, f| f(&mut app.counter));
/// let counter_data = app_data.child(counter_lens);
/// ```
pub trait Lens<T, U> {
    /// Get a reference to the sub-field
    fn get<'a>(&self, data: &'a T) -> &'a U;

    /// Call a function with a mutable reference to the sub-field
    fn with_mut<V, F>(&self, data: &mut T, f: F) -> V
    where
        F: FnOnce(&mut U) -> V;
}

/// Function-based lens implementation
///
/// This allows creating lenses from closures.
pub struct FnLens<T, U, G, M>
where
    G: Fn(&T) -> &U,
    M: Fn(&mut T) -> &mut U,
{
    getter: G,
    mutter: M,
    _phantom: core::marker::PhantomData<(T, U)>,
}

impl<T, U, G, M> FnLens<T, U, G, M>
where
    G: Fn(&T) -> &U,
    M: Fn(&mut T) -> &mut U,
{
    /// Create a new lens from getter and mutter functions
    pub fn new(getter: G, mutter: M) -> Self {
        Self {
            getter,
            mutter,
            _phantom: core::marker::PhantomData,
        }
    }
}

impl<T, U, G, M> Lens<T, U> for FnLens<T, U, G, M>
where
    G: Fn(&T) -> &U,
    M: for<'a> Fn(&'a mut T) -> &'a mut U,
{
    fn get<'a>(&self, data: &'a T) -> &'a U {
        (self.getter)(data)
    }

    fn with_mut<V, F>(&self, data: &mut T, f: F) -> V
    where
        F: FnOnce(&mut U) -> V,
    {
        f((self.mutter)(data))
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

        ctx.modify(|v| *v += 1);
        assert_eq!(ctx.get(), 43);
        assert_eq!(ctx.version(), 1);
    }

    #[test]
    fn test_data_context_subscribe() {
        let ctx = DataContext::new(42);
        let view_id = ViewId::new();

        let initial_version = ctx.subscribe(view_id);
        assert_eq!(initial_version, 0);

        ctx.modify(|v| *v += 1);

        let dirty = ctx.dirty_views();
        assert!(dirty.contains(&view_id));
    }

    #[test]
    fn test_data_context_clear_dirty() {
        let ctx = DataContext::new(42);
        let view_id = ViewId::new();

        ctx.subscribe(view_id);
        ctx.modify(|v| *v += 1);

        ctx.clear_dirty(view_id);

        let dirty = ctx.dirty_views();
        assert!(!dirty.contains(&view_id));
    }

    #[test]
    fn test_lens() {
        struct TestData {
            inner: u32,
        }

        let mut data = TestData { inner: 42 };

        let lens = FnLens::new(|d: &TestData| &d.inner, |d: &mut TestData| &mut d.inner);

        assert_eq!(lens.get(&data), &42);

        lens.with_mut(&mut data, |v| *v += 1);
        assert_eq!(data.inner, 43);
    }
}

/// Create a DataContext with a single value (like SwiftUI's @State)
///
/// # Example
///
/// ```ignore
/// fn build_ui() {
///     let enabled = bindable!(false);
///     let volume = bindable!(50.0);
///     let app = bindable!(AppState::new());
///
///     let toggle = Toggle::bind(&enabled);
///     let slider = Slider::bind(&volume, 0.0, 100.0);
/// }
/// ```
#[macro_export]
macro_rules! bindable {
    ($value:expr) => {
        DataContext::new($value)
    };
}
