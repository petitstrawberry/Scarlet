//! Observable Pattern Usage Example
//!
//! This example demonstrates how to use the Observable pattern for
//! reference types (equivalent to SwiftUI's @StateObject/@ObservedObject).
//!
//! # SwiftUI Pattern
//!
//! ```swift
//! class UserSettings: ObservableObject {
//!     @Published var username: String
//!     @Published var isPremium: Bool
//! }
//!
//! struct ProfileView: View {
//!     @StateObject private var settings = UserSettings()
//!     @ObservedObject var appSettings: AppSettings
//! }
//! ```
//!
//! # ScarletUI Pattern (Manual Implementation)
//!
//! ```ignore
//! struct UserSettings {
//!     notifier: ObservableNotifier,
//!     username: String,
//!     is_premium: bool,
//! }
//!
//! impl UserSettings {
//!     fn set_username(&mut self, username: String) {
//!         self.username = username;
//!         self.notifier.notify();  // Important: notify observers
//!     }
//! }
//!
//! impl Observable for UserSettings { ... }
//!
//! struct ProfileView {
//!     settings: StateObject<UserSettings>,
//!     app_settings: Observed<AppSettings>,
//! }
//! ```

use scarlet_ui::{
    Observable, ObservableNotifier, StateObject, Observed,
    Local, View, ViewId, VStack, Toggle,
};
use alloc::sync::Arc;
use scarlet_std::sync::Mutex;

/// UserSettings model (manual Observable implementation)
pub struct UserSettings {
    notifier: ObservableNotifier,
    username: String,
    is_premium: bool,
    notification_count: u32,
}

impl UserSettings {
    pub fn new(username: String) -> Self {
        Self {
            notifier: ObservableNotifier::new(),
            username,
            is_premium: false,
            notification_count: 0,
        }
    }

    // Getters
    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn is_premium(&self) -> bool {
        self.is_premium
    }

    pub fn notification_count(&self) -> u32 {
        self.notification_count
    }

    // Setters that notify observers
    pub fn set_username(&mut self, username: String) {
        self.username = username;
        self.notifier.notify();
    }

    pub fn set_premium(&mut self, is_premium: bool) {
        self.is_premium = is_premium;
        self.notifier.notify();
    }

    pub fn increment_notifications(&mut self) {
        self.notification_count += 1;
        self.notifier.notify();
    }
}

impl Observable for UserSettings {
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

/// AppSettings (shared across the app)
pub struct AppSettings {
    notifier: ObservableNotifier,
    dark_mode: bool,
    language: String,
}

impl AppSettings {
    pub fn new() -> Self {
        Self {
            notifier: ObservableNotifier::new(),
            dark_mode: false,
            language: String::from("en"),
        }
    }

    pub fn dark_mode(&self) -> bool {
        self.dark_mode
    }

    pub fn set_dark_mode(&mut self, enabled: bool) {
        self.dark_mode = enabled;
        self.notifier.notify();
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    pub fn set_language(&mut self, language: String) {
        self.language = language;
        self.notifier.notify();
    }
}

impl Observable for AppSettings {
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

/// ProfileView - owns its UserSettings (@StateObject equivalent)
pub struct ProfileView {
    // @StateObject equivalent - this view owns the settings
    settings: StateObject<UserSettings>,
    // @ObservedObject equivalent - observes parent's app settings
    app_settings: Observed<'static, AppSettings>,
    // Local state for UI
    show_details: Local<bool>,
}

impl ProfileView {
    pub fn new(app_settings: &'static AppSettings) -> Self {
        Self {
            settings: StateObject::new(UserSettings::new(String::from("user123"))),
            app_settings: Observed::new(app_settings),
            show_details: Local::new(false),
        }
    }

    pub fn build(&self) -> impl View {
        VStack::new()
            // Toggle bound to local state
            .child(Toggle::new("Show Details").bind(self.show_details.bind()))

        // Note: In a real implementation, you would subscribe to
        // settings changes to trigger redraws
    }

    pub fn update_username(&mut self, username: String) {
        self.settings.get_mut().set_username(username);
        // After this, all observers are notified
    }

    pub fn upgrade_to_premium(&mut self) {
        self.settings.get_mut().set_premium(true);
    }
}

/// AppView - owns AppSettings and passes to children
pub struct AppView {
    // The app owns this settings object
    app_settings: StateObject<AppSettings>,
    // User settings
    user_settings: StateObject<UserSettings>,
}

impl AppView {
    pub fn new() -> Self {
        // Create static app settings
        static APP_SETTINGS: AppSettings = AppSettings {
            notifier: ObservableNotifier::new(),
            dark_mode: false,
            language: String::from("en"),
        };

        Self {
            app_settings: StateObject::new(APP_SETTINGS),
            user_settings: StateObject::new(UserSettings::new(String::from("user123"))),
        }
    }

    pub fn build(&self) -> impl View {
        // Create child views and pass app_settings
        // Note: This requires 'static lifetime for simplicity
        // In real code, you'd use a different approach
    }

    pub fn toggle_dark_mode(&mut self) {
        let current = self.app_settings.dark_mode();
        self.app_settings.get_mut().set_dark_mode(!current);
    }
}

/// ChildView - receives shared state from parent
pub struct ChildView {
    // Observes parent's state (doesn't own it)
    settings: Observed<'static, UserSettings>,
    local_enabled: Local<bool>,
}

impl ChildView {
    pub fn new(settings: &'static UserSettings) -> Self {
        Self {
            settings: Observed::new(settings),
            local_enabled: Local::new(false),
        }
    }

    pub fn build(&self) -> impl View {
        VStack::new()
            .child(Toggle::new("Local Toggle").bind(self.local_enabled.bind()))

        // Can access parent's settings through self.settings
    }

    pub fn username(&self) -> &str {
        self.settings.username()
    }
}

/// Advanced: Combining @State and @StateObject
pub struct AdvancedView {
    // Local state (value types)
    counter: Local<u32>,
    enabled: Local<bool>,

    // Observable state (reference types)
    user_data: StateObject<UserSettings>,
}

impl AdvancedView {
    pub fn new() -> Self {
        Self {
            counter: Local::new(0),
            enabled: Local::new(true),
            user_data: StateObject::new(UserSettings::new(String::from("user"))),
        }
    }

    pub fn build(&self) -> impl View {
        // Combine local and observable state
    }

    pub fn increment(&self) {
        self.counter.modify(|c| *c += 1);
    }

    pub fn update_username(&mut self, username: String) {
        self.user_data.get_mut().set_username(username);
    }
}

/// Example: Subscribing to changes
pub mod subscription_example {
    use super::*;
    use alloc::sync::Arc;

    pub fn demonstrate_subscription() {
        let settings = UserSettings::new(String::from("test_user"));

        // Subscribe to changes
        let call_count = Arc::new(Mutex::new(0));
        let call_count_clone = call_count.clone();

        settings.subscribe(Box::new(move || {
            *call_count_clone.lock() += 1;
            println!("Settings changed!");
        }));

        // Trigger a change
        settings.set_username(String::from("new_user"));

        // Check that the observer was called
        assert_eq!(*call_count.lock(), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_settings() {
        let mut settings = UserSettings::new(String::from("user123"));

        assert_eq!(settings.username(), "user123");
        assert_eq!(settings.is_premium(), false);

        settings.set_username(String::from("new_user"));
        assert_eq!(settings.username(), "new_user");

        settings.set_premium(true);
        assert!(settings.is_premium());
    }

    #[test]
    fn test_state_object() {
        let settings = StateObject::new(UserSettings::new(String::from("user")));

        // Can access through deref
        assert_eq!(settings.username(), "user");

        // Can modify
        settings.get_mut().set_username(String::from("modified"));
        assert_eq!(settings.username(), "modified");
    }

    #[test]
    fn test_observed() {
        let settings = UserSettings::new(String::from("user"));
        let observed = Observed::new(&settings);

        // Can access through deref
        assert_eq!(observed.username(), "user");
    }

    #[test]
    fn test_observable_notification() {
        let mut settings = UserSettings::new(String::from("user"));

        let call_count = Arc::new(Mutex::new(0));
        let call_count_clone = call_count.clone();

        settings.subscribe(Box::new(move || {
            *call_count_clone.lock() += 1;
        }));

        settings.set_username(String::from("new"));
        assert_eq!(*call_count.lock(), 1);

        settings.set_premium(true);
        assert_eq!(*call_count.lock(), 2);
    }

    #[test]
    fn test_app_settings() {
        let mut app = AppSettings::new();
        assert_eq!(app.dark_mode(), false);

        app.set_dark_mode(true);
        assert!(app.dark_mode());
    }

    #[test]
    fn test_subscription_example() {
        subscription_example::demonstrate_subscription();
    }
}

/// Future: Macro-based implementation
///
/// TODO: Implement #[observable] and #[published] macros
///
/// ```ignore
/// #[observable]
/// struct UserSettings {
///     #[published]
///     username: String,
///     #[published]
///     is_premium: bool,
///     notification_count: u32,  // Not published, no auto-notify
/// }
///
/// // Will expand to:
/// struct UserSettings {
///     notifier: ObservableNotifier,
///     username: String,
///     is_premium: bool,
///     notification_count: u32,
/// }
///
/// impl UserSettings {
///     fn set_username(&mut self, username: String) {
///         self.username = username;
///         self.notifier.notify();
///     }
///     // ... other setters
/// }
///
/// impl Observable for UserSettings { ... }
/// ```
///
/// This will be implemented in scarlet-ui-macros crate.
