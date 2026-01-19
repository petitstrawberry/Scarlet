//! #[observable] Macro Usage Example
//!
//! This example demonstrates the #[observable] and #[published] macros.

extern crate alloc;
use alloc::string::String;
use alloc::sync::Arc;

use scarlet_ui::{observable, published, StateObject, Observed, Observable, ObservableNotifier};

/// Example 1: Using the macro
#[observable]
struct UserSettings {
    #[published]
    username: String,

    #[published]
    is_premium: bool,

    #[published]
    score: u32,

    // Not published - no automatic notification
    internal_count: u32,
}

/// Example 2: Partially observable struct
#[observable]
struct AppConfig {
    #[published]
    dark_mode: bool,

    #[published]
    language: String,

    version: String,
}

/// Example usage
fn example_usage() {
    // Create a StateObject (owns the observable)
    let settings = StateObject::new(UserSettings {
        username: String::from("user123"),
        is_premium: false,
        score: 0,
        internal_count: 0,
    });

    // Access fields
    assert_eq!(settings.username(), "user123");
    assert_eq!(settings.is_premium(), false);
    assert_eq!(settings.score(), 0);
    assert_eq!(settings.internal_count, 0);

    // Modify fields (automatically notifies observers)
    settings.get_mut().username(String::from("new_user"));
    assert_eq!(settings.username(), "new_user");

    settings.get_mut().set_is_premium(true);
    assert!(settings.is_premium());

    settings.get_mut().score(100);
    assert_eq!(settings.score(), 100);

    // Direct modification (no notification)
    settings.get_mut().internal_count = 5;
    assert_eq!(settings.internal_count, 5);
}

/// Example: Creating with new() method
///
/// Note: The macro doesn't generate a new() method, you need to implement it yourself
impl UserSettings {
    pub fn new(username: String, is_premium: bool, score: u32) -> Self {
        Self {
            notifier: ObservableNotifier::new(),
            username,
            is_premium,
            score,
            internal_count: 0,
        }
    }
}

impl AppConfig {
    pub fn new() -> Self {
        Self {
            notifier: ObservableNotifier::new(),
            dark_mode: false,
            language: String::from("en"),
            version: String::from("1.0.0"),
        }
    }
}

/// Advanced: Using StateObject and Observed together
struct AppView {
    // Owned observable
    user_settings: StateObject<UserSettings>,

    // Observed from parent (in a real app, this would be passed in)
    // For simplicity, we're creating it here
    app_config: StateObject<AppConfig>,
}

impl AppView {
    pub fn new() -> Self {
        Self {
            user_settings: StateObject::new(UserSettings::new(
                String::from("user"),
                false,
                0,
            )),
            app_config: StateObject::new(AppConfig::new()),
        }
    }

    pub fn build(&self) {
        // In a real app, this would build a view hierarchy
        // The StateObjects can be passed to child views as Observed
    }

    pub fn update_username(&mut self, username: String) {
        self.user_settings.get_mut().username(username);
        // All observers are automatically notified
    }

    pub fn toggle_dark_mode(&mut self) {
        let current = self.app_config.dark_mode();
        self.app_config.get_mut().dark_mode(!current);
    }
}

/// Example: Subscribing to changes
fn example_subscription() {
    use scarlet_std::sync::Mutex;

    let settings = StateObject::new(UserSettings::new(
        String::from("user"),
        false,
        0,
    ));

    // Subscribe to changes
    let call_count = Arc::new(Mutex::new(0));
    let call_count_clone = call_count.clone();

    settings.subscribe(Box::new(move || {
        *call_count_clone.lock() += 1;
        println!("Settings changed!");
    }));

    // Trigger changes through setters
    settings.get_mut().username(String::from("new"));
    assert_eq!(*call_count.lock(), 1);

    settings.get_mut().set_is_premium(true);
    assert_eq!(*call_count.lock(), 2);

    settings.get_mut().score(100);
    assert_eq!(*call_count.lock(), 3);

    // Direct modification doesn't trigger notification
    settings.get_mut().internal_count = 5;
    assert_eq!(*call_count.lock(), 3); // Still 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use scarlet_std::sync::Mutex;

    #[test]
    fn test_observable_macro() {
        let settings = StateObject::new(UserSettings {
            username: String::from("user"),
            is_premium: false,
            score: 0,
            internal_count: 0,
        });

        assert_eq!(settings.username(), "user");
        assert_eq!(settings.is_premium(), false);
        assert_eq!(settings.score(), 0);
        assert_eq!(settings.internal_count, 0);

        // Test setter for username
        settings.get_mut().username(String::from("new"));
        assert_eq!(settings.username(), "new");

        // Test setter for is_premium
        settings.get_mut().set_is_premium(true);
        assert!(settings.is_premium());

        // Test setter for score
        settings.get_mut().score(100);
        assert_eq!(settings.score(), 100);

        // Direct access to internal_count
        settings.get_mut().internal_count = 5;
        assert_eq!(settings.internal_count, 5);
    }

    #[test]
    fn test_observable_trait() {
        let settings = StateObject::new(UserSettings {
            username: String::from("user"),
            is_premium: false,
            score: 0,
            internal_count: 0,
        });

        // Test that Observable trait is implemented
        let call_count = Arc::new(Mutex::new(0));
        let call_count_clone = call_count.clone();

        settings.subscribe(Box::new(move || {
            *call_count_clone.lock() += 1;
        }));

        settings.get_mut().username(String::from("new"));
        assert_eq!(*call_count.lock(), 1);
    }

    #[test]
    fn test_partial_observable() {
        let config = StateObject::new(AppConfig {
            dark_mode: false,
            language: String::from("en"),
            version: String::from("1.0"),
        });

        assert_eq!(config.dark_mode(), false);
        assert_eq!(config.language(), "en");
        assert_eq!(config.version(), "1.0");

        config.get_mut().dark_mode(true);
        assert!(config.dark_mode());

        config.get_mut().language(String::from("ja"));
        assert_eq!(config.language(), "ja");

        // Direct modification (no notification)
        config.get_mut().version = String::from("2.0");
        assert_eq!(config.version(), "2.0");
    }

    #[test]
    fn test_new_method() {
        let settings = UserSettings::new(String::from("user"), true, 100);
        assert_eq!(settings.username(), "user");
        assert!(settings.is_premium());
        assert_eq!(settings.score(), 100);
        assert_eq!(settings.internal_count, 0);
    }

    #[test]
    fn test_example_usage() {
        example_usage();
    }

    #[test]
    fn test_subscription() {
        example_subscription();
    }

    #[test]
    fn test_app_view() {
        let mut app = AppView::new();

        app.update_username(String::from("new_user"));
        assert_eq!(app.user_settings.username(), "new_user");

        app.toggle_dark_mode();
        assert!(app.app_config.dark_mode());
    }
}

/// Comparison: Manual vs Macro
///
/// BEFORE (Manual implementation):
/// ```ignore
/// struct UserSettings {
///     notifier: ObservableNotifier,
///     username: String,
///     is_premium: bool,
/// }
///
/// impl UserSettings {
///     fn set_username(&mut self, username: String) {
///         self.username = username;
///         self.notifier.notify();
///     }
///
///     fn set_is_premium(&mut self, is_premium: bool) {
///         self.is_premium = is_premium;
///         self.notifier.notify();
///     }
/// }
///
/// impl Observable for UserSettings {
///     type SubscriptionId = usize;
///     fn subscribe(&self, observer: Box<dyn Fn() + Send + Sync>) -> Self::SubscriptionId {
///         self.notifier.subscribe(observer)
///     }
///     fn unsubscribe(&self, id: Self::SubscriptionId) {
///         self.notifier.unsubscribe(id)
///     }
///     fn notify(&self) {
///         self.notifier.notify()
///     }
/// }
/// ```
///
/// AFTER (With macro):
/// ```ignore
/// #[observable]
/// struct UserSettings {
///     #[published]
///     username: String,
///     #[published]
///     is_premium: bool,
/// }
/// ```
///
/// The macro generates all the boilerplate automatically!
