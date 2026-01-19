//! Observable trait for reference types
//!
//! This module provides the Observable trait and related types for
//! implementing observable reference types (equivalent to SwiftUI's ObservableObject).
//!
//! # Example
//!
//! ```ignore
//! #[observable]
//! struct UserModel {
//!     #[published]
//!     name: String,
//!     #[published]
//!     age: u32,
//! }
//!
//! struct ProfileView {
//!     @StateObject private var settings = UserSettings()
//!     @ObservedObject var app_config: AppConfig
//! }
//! ```

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::sync::Arc;
use scarlet_std::sync::Mutex;

/// Observable trait for reference types
///
/// Types implementing this trait can notify subscribers when they change.
/// This is equivalent to Swift's ObservableObject protocol.
///
/// # Example
///
/// ```ignore
/// struct UserModel {
///     notifier: ObservableNotifier,
///     name: String,
///     age: u32,
/// }
///
/// impl UserModel {
///     fn set_name(&mut self, name: String) {
///         self.name = name;
///         self.notifier.notify();  // Notify subscribers
///     }
/// }
///
/// impl Observable for UserModel {
///     type SubscriptionId = usize;
///
///     fn subscribe(&self, observer: Box<dyn Fn() + Send + Sync>) -> Self::SubscriptionId {
///         self.notifier.subscribe(observer)
///     }
///
///     fn unsubscribe(&self, id: Self::SubscriptionId) {
///         self.notifier.unsubscribe(id)
///     }
/// }
/// ```
pub trait Observable {
    /// Unique identifier for a subscription
    type SubscriptionId: Clone;

    /// Subscribe to changes
    ///
    /// Returns a subscription ID that can be used to unsubscribe later.
    fn subscribe(&self, observer: Box<dyn Fn() + Send + Sync>) -> Self::SubscriptionId;

    /// Unsubscribe from changes
    fn unsubscribe(&self, id: Self::SubscriptionId);

    /// Notify all subscribers of a change
    fn notify(&self);
}

/// Subscription notifier for implementing Observable
///
/// This helper type can be embedded in your struct to implement
/// change notification.
///
/// # Example
///
/// ```ignore
/// struct MyModel {
///     notifier: ObservableNotifier,
///     value: i32,
/// }
///
/// impl MyModel {
///     fn set_value(&mut self, value: i32) {
///         self.value = value;
///         self.notifier.notify();
///     }
/// }
///
/// impl Observable for MyModel {
///     type SubscriptionId = usize;
///
///     fn subscribe(&self, observer: Box<dyn Fn() + Send + Sync>) -> Self::SubscriptionId {
///         self.notifier.subscribe(observer)
///     }
///
///     fn unsubscribe(&self, id: Self::SubscriptionId) {
///         self.notifier.unsubscribe(id)
///     }
///
///     fn notify(&self) {
///         self.notifier.notify()
///     }
/// }
/// ```
pub struct ObservableNotifier {
    observers: Arc<Mutex<Vec<Box<dyn Fn() + Send + Sync>>>>,
    next_id: Arc<Mutex<usize>>,
}

impl ObservableNotifier {
    /// Create a new notifier
    pub fn new() -> Self {
        Self {
            observers: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(Mutex::new(0)),
        }
    }

    /// Subscribe to changes
    ///
    /// Returns a subscription ID.
    pub fn subscribe(&self, observer: Box<dyn Fn() + Send + Sync>) -> usize {
        let mut next_id = self.next_id.lock();
        let id = *next_id;
        *next_id += 1;

        let mut observers = self.observers.lock();
        observers.push(observer);

        id
    }

    /// Unsubscribe (no-op in simple implementation)
    pub fn unsubscribe(&self, _id: usize) {
        // Simple implementation: observers are never removed
        // A full implementation would track (id, index) mappings
    }

    /// Notify all subscribers
    pub fn notify(&self) {
        let observers = self.observers.lock();
        for observer in observers.iter() {
            observer();
        }
    }
}

impl Default for ObservableNotifier {
    fn default() -> Self {
        Self::new()
    }
}

/// View-owned reference type (equivalent to SwiftUI's @StateObject)
///
/// `StateObject<T>` is used for reference types (implementing `Observable`)
/// that are owned by a View. The View creates the object and is responsible
/// for its lifetime.
///
/// # Type Parameters
///
/// * `T` - The observable reference type
///
/// # Example
///
/// ```ignore
/// struct AppView {
///     model: StateObject<UserModel>,
/// }
///
/// impl AppView {
///     fn new() -> Self {
///         Self {
///             model: StateObject::new(UserModel::new()),
///         }
///     }
///
///     fn build(&self) -> impl View {
///         // Use the model
///     }
/// }
/// ```
pub struct StateObject<T: Observable> {
    inner: T,
}

impl<T: Observable> StateObject<T> {
    /// Create a new state object
    pub fn new(value: T) -> Self {
        Self { inner: value }
    }

    /// Get a reference to the inner object
    pub fn get(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner object
    ///
    /// After modifying, call `notify()` on the object to trigger updates.
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Subscribe to changes in this object
    pub fn subscribe(&self, observer: Box<dyn Fn() + Send + Sync>) -> T::SubscriptionId {
        self.inner.subscribe(observer)
    }

    /// Unsubscribe from changes
    pub fn unsubscribe(&self, id: T::SubscriptionId) {
        self.inner.unsubscribe(id)
    }
}

impl<T: Observable> core::ops::Deref for StateObject<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: Observable> core::ops::DerefMut for StateObject<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Observation of parent's reference type (equivalent to SwiftUI's @ObservedObject)
///
/// `Observed<'a, T>` provides a read-only reference to a parent's observable object.
/// It can be safely passed to child views.
///
/// # Example
///
/// ```ignore
/// struct ChildView {
///     // Observed reference to parent's model
///     model: Observed<UserModel>,
/// }
///
/// impl ChildView {
///     fn new(model: Observed<UserModel>) -> Self {
///         Self { model }
///     }
///
///     fn build(&self) -> impl View {
///         // Observe changes in model
///     }
/// }
/// ```
#[derive(Clone, Copy)]
pub struct Observed<'a, T: Observable> {
    inner: &'a T,
}

impl<'a, T: Observable> Observed<'a, T> {
    /// Create a new observed reference
    pub fn new(inner: &'a T) -> Self {
        Self { inner }
    }

    /// Get a reference to the inner object
    pub fn get(&self) -> &T {
        self.inner
    }

    /// Subscribe to changes
    pub fn subscribe(&self, observer: Box<dyn Fn() + Send + Sync>) -> T::SubscriptionId {
        self.inner.subscribe(observer)
    }

    /// Unsubscribe from changes
    pub fn unsubscribe(&self, id: T::SubscriptionId) {
        self.inner.unsubscribe(id)
    }
}

impl<'a, T: Observable> core::ops::Deref for Observed<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test model implementing Observable
    struct TestModel {
        notifier: ObservableNotifier,
        value: i32,
    }

    impl TestModel {
        fn new(value: i32) -> Self {
            Self {
                notifier: ObservableNotifier::new(),
                value,
            }
        }

        fn set_value(&mut self, value: i32) {
            self.value = value;
            self.notifier.notify();
        }

        fn get_value(&self) -> i32 {
            self.value
        }
    }

    impl Observable for TestModel {
        type SubscriptionId = usize;

        fn subscribe(&self, observer: Box<dyn Fn() + Send + Sync>) -> Self::SubscriptionId {
            self.notifier.subscribe(observer)
        }

        fn unsubscribe(&self, id: Self::SubscriptionId) {
            self.notifier.unsubscribe(id)
        }

        fn notify(&self) {
            self.notifier.notify()
        }
    }

    #[test]
    fn test_observable_notifier() {
        let notifier = ObservableNotifier::new();

        let call_count = Arc::new(Mutex::new(0));
        let call_count_clone = call_count.clone();

        notifier.subscribe(Box::new(move || {
            *call_count_clone.lock() += 1;
        }));

        notifier.notify();
        assert_eq!(*call_count.lock(), 1);

        notifier.notify();
        assert_eq!(*call_count.lock(), 2);
    }

    #[test]
    fn test_state_object() {
        let model = StateObject::new(TestModel::new(42));
        assert_eq!(model.get_value(), 42);

        model.get_mut().set_value(100);
        assert_eq!(model.get_value(), 100);
    }

    #[test]
    fn test_observed() {
        let model = TestModel::new(42);
        let observed = Observed::new(&model);

        assert_eq!(observed.get_value(), 42);

        // Can subscribe to changes
        let call_count = Arc::new(Mutex::new(0));
        let call_count_clone = call_count.clone();

        observed.subscribe(Box::new(move || {
            *call_count_clone.lock() += 1;
        }));

        // Modify the original model
        // Note: This would require interior mutability in a real scenario
        // For this test, we just verify the subscription API works
    }

    #[test]
    fn test_state_object_deref() {
        let model = StateObject::new(TestModel::new(42));

        // Deref allows accessing methods directly
        assert_eq!(model.get_value(), 42);

        // DerefMut allows modifying
        model.get_mut().set_value(100);
        assert_eq!(model.get_value(), 100);
    }

    #[test]
    fn test_observed_deref() {
        let model = TestModel::new(42);
        let observed = Observed::new(&model);

        // Deref allows accessing methods directly
        assert_eq!(observed.get_value(), 42);
    }

    // Test #[observable] macro
    #[cfg(all(test, feature = "macro-test"))]
    mod macro_tests {
        use super::*;
        use scarlet_ui_macros::{observable, published};

        #[observable]
        struct TestUserSettings {
            #[published]
            username: String,

            #[published]
            is_premium: bool,

            #[published]
            login_count: u32,

            internal_id: u32,
        }

        impl Default for TestUserSettings {
            fn default() -> Self {
                Self {
                    username: String::from("test"),
                    is_premium: false,
                    login_count: 0,
                    internal_id: 999,
                }
            }
        }

        #[test]
        fn test_observable_macro() {
            // Test creating with new()
            let settings = TestUserSettings::new();
            assert_eq!(settings._username().get(), String::from("test"));
            assert_eq!(settings._is_premium().get(), false);
            assert_eq!(settings._login_count().get(), 0);
        }

        #[test]
        fn test_observable_macro_getters() {
            let settings = TestUserSettings::new();

            // Test underscore getters return DataContext
            let username_data = settings._username();
            assert_eq!(username_data.get(), String::from("test"));

            let is_premium_data = settings._is_premium();
            assert_eq!(is_premium_data.get(), false);
        }

        #[test]
        fn test_observable_macro_setters() {
            let mut settings = TestUserSettings::new();

            // Test setter updates both field and DataContext
            settings.username(String::from("alice"));
            assert_eq!(settings._username().get(), String::from("alice"));

            settings.is_premium(true);
            assert_eq!(settings._is_premium().get(), true);

            settings.login_count(42);
            assert_eq!(settings._login_count().get(), 42);
        }

        #[test]
        fn test_observable_macro_notifications() {
            let settings = TestUserSettings::new();

            let call_count = Arc::new(Mutex::new(0));
            let call_count_clone = call_count.clone();

            settings.subscribe(Box::new(move || {
                *call_count_clone.lock().unwrap() += 1;
            }));

            settings.username(String::from("bob"));
            assert_eq!(*call_count.lock(), 1);

            settings.is_premium(false);
            assert_eq!(*call_count.lock(), 2);
        }
    }
}
