//! Observable wrapper for reactive data
//!
//! This module provides Observable<T>, which wraps a DataContext<T>
//! and provides a more ergonomic API for reactive state management.
//!
//! # Key Concepts
//!
//! - **Observable wrapper**: Combines DataContext with ViewId tracking
//! - **Ergonomic API**: Simpler interface for common operations
//! - **View integration**: Designed to work seamlessly with View trait

use crate::view::id::ViewId;
use crate::view::tracker::RenderTracker;
use crate::state::data::DataContext;

/// Observable wrapper around DataContext
///
/// Observable<T> provides a simpler API for working with reactive data.
/// It combines a DataContext with ViewId subscription tracking.
///
/// # Example
///
/// ```ignore
/// let observable = Observable::new(42);
/// let view_id = ViewId::new();
///
/// // Subscribe to changes
/// observable.subscribe(view_id);
///
/// // Modify data (automatically notifies subscribers)
/// observable.modify(|v| *v += 1);
/// ```
pub struct Observable<T> {
    /// The underlying data context
    data: DataContext<T>,
    /// The ViewId that owns this observable (for auto-subscription)
    owner: Option<ViewId>,
}

impl<T> Observable<T> {
    /// Create a new observable with an initial value
    pub fn new(data: T) -> Self {
        Self {
            data: DataContext::new(data),
            owner: None,
        }
    }

    /// Create a new observable with an owner ViewId
    ///
    /// The owner will be automatically subscribed to changes.
    pub fn with_owner(data: T, owner: ViewId) -> Self {
        let observable = Self {
            data: DataContext::new(data),
            owner: Some(owner),
        };
        observable
    }

    /// Get an immutable reference to the data
    ///
    /// This is a read-only operation that doesn't mark views as dirty.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.data.get()
    }

    /// Read the data with a closure (more efficient than cloning)
    pub fn read<U, F>(&self, f: F) -> U
    where
        F: FnOnce(&T) -> U,
    {
        self.data.read(f)
    }

    /// Modify the data and notify observers
    ///
    /// This will:
    /// 1. Apply the modification function to the data
    /// 2. Increment the version
    /// 3. Mark all observing views as dirty
    /// 4. Return the result from the modification function
    ///
    /// Note: This does NOT notify the RenderTracker. Use `modify_with_tracker`
    /// for automatic render tracker integration.
    pub fn modify<U, F>(&self, f: F) -> U
    where
        F: FnOnce(&mut T) -> U,
    {
        self.data.modify(f)
    }

    /// Modify the data and notify RenderTracker
    ///
    /// This is the recommended method for modifying data in a view.
    /// It modifies the data AND immediately notifies the RenderTracker.
    ///
    /// # Arguments
    ///
    /// * `tracker` - The RenderTracker to notify
    /// * `f` - The modification function
    pub fn modify_with_tracker<U, F>(&self, tracker: &mut RenderTracker, f: F) -> U
    where
        F: FnOnce(&mut T) -> U,
    {
        self.data.modify_with_tracker(tracker, f)
    }

    /// Set a new value (replaces the entire data)
    ///
    /// Note: This does NOT notify the RenderTracker. Use `set_with_tracker`
    /// for automatic render tracker integration.
    pub fn set(&self, data: T)
    where
        T: PartialEq,
    {
        self.data.set(data)
    }

    /// Set a new value and notify RenderTracker
    ///
    /// This is the recommended method for setting data in a view.
    ///
    /// # Arguments
    ///
    /// * `tracker` - The RenderTracker to notify
    /// * `data` - The new data value
    pub fn set_with_tracker(&self, tracker: &mut RenderTracker, data: T)
    where
        T: PartialEq,
    {
        self.data.set_with_tracker(tracker, data)
    }

    /// Get the current data version
    pub fn version(&self) -> u64 {
        self.data.version()
    }

    /// Subscribe a view to observe this data
    ///
    /// Returns the initial data version that this view should use.
    pub fn subscribe(&self, view_id: ViewId) -> u64 {
        self.data.subscribe(view_id)
    }

    /// Unsubscribe a view from this data
    pub fn unsubscribe(&self, view_id: ViewId) {
        self.data.unsubscribe(view_id)
    }

    /// Check if a view needs an update
    ///
    /// This compares the view's last seen version with the current version.
    pub fn needs_update(&self, view_id: ViewId, last_version: u64) -> bool {
        self.data.needs_update(view_id, last_version)
    }

    /// Get the set of dirty views
    pub fn dirty_views(&self) -> scarlet_std::vec::Vec<ViewId> {
        self.data.dirty_views()
    }

    /// Clear the dirty flag for a specific view
    pub fn clear_dirty(&self, view_id: ViewId) {
        self.data.clear_dirty(view_id)
    }

    /// Mark a view as dirty (needs redraw)
    pub fn mark_dirty(&self, view_id: ViewId) {
        self.data.mark_dirty(view_id)
    }

    /// Get the underlying DataContext
    pub fn data_context(&self) -> &DataContext<T> {
        &self.data
    }

    /// Get the owner ViewId
    pub fn owner(&self) -> Option<ViewId> {
        self.owner
    }

    /// Set the owner ViewId
    pub fn set_owner(&mut self, owner: ViewId) {
        self.owner = Some(owner);
        self.data.subscribe(owner);
    }
}

impl<T: Clone> Clone for Observable<T> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            owner: self.owner,
        }
    }
}

/// Builder for creating Observable with configured options
pub struct ObservableBuilder<T> {
    data: T,
    owner: Option<ViewId>,
}

impl<T> ObservableBuilder<T> {
    /// Create a new builder with an initial value
    pub fn new(data: T) -> Self {
        Self {
            data,
            owner: None,
        }
    }

    /// Set the owner ViewId
    pub fn owner(mut self, owner: ViewId) -> Self {
        self.owner = Some(owner);
        self
    }

    /// Build the Observable
    pub fn build(self) -> Observable<T> {
        Observable::with_owner(self.data, self.owner.expect("owner must be set"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_observable_new() {
        let obs = Observable::new(42);
        assert_eq!(obs.get(), 42);
        assert_eq!(obs.version(), 0);
    }

    #[test]
    fn test_observable_modify() {
        let obs = Observable::new(42);
        obs.modify(|v| *v += 1);
        assert_eq!(obs.get(), 43);
        assert_eq!(obs.version(), 1);
    }

    #[test]
    fn test_observable_with_owner() {
        let view_id = ViewId::new();
        let obs = Observable::with_owner(42, view_id);

        assert_eq!(obs.owner(), Some(view_id));
        assert_eq!(obs.get(), 42);
    }

    #[test]
    fn test_observable_subscribe() {
        let obs = Observable::new(42);
        let view_id = ViewId::new();

        let initial_version = obs.subscribe(view_id);
        assert_eq!(initial_version, 0);

        obs.modify(|v| *v += 1);

        let dirty = obs.dirty_views();
        assert!(dirty.contains(&view_id));
    }

    #[test]
    fn test_observable_modify_with_tracker() {
        let obs = Observable::new(42);
        let view_id = ViewId::new();
        let mut tracker = RenderTracker::new();

        obs.subscribe(view_id);
        obs.modify_with_tracker(&mut tracker, |v| *v += 1);

        assert_eq!(obs.get(), 43);
        assert!(tracker.needs_paint());
        let dirty_paint = tracker.take_dirty_paint();
        assert!(dirty_paint.contains(&view_id));
    }

    #[test]
    fn test_observable_set() {
        let obs = Observable::new(42);
        obs.set(100);
        assert_eq!(obs.get(), 100);
    }

    #[test]
    fn test_observable_set_no_change() {
        let obs = Observable::new(42);
        obs.set(42); // Same value
        assert_eq!(obs.version(), 0); // Version should not change
    }

    #[test]
    fn test_observable_builder() {
        let view_id = ViewId::new();
        let obs = ObservableBuilder::new(42)
            .owner(view_id)
            .build();

        assert_eq!(obs.get(), 42);
        assert_eq!(obs.owner(), Some(view_id));
    }

    #[test]
    fn test_observable_read() {
        let obs = Observable::new(42);
        let result = obs.read(|v| *v * 2);
        assert_eq!(result, 84);
        // Version should not change on read
        assert_eq!(obs.version(), 0);
    }
}
