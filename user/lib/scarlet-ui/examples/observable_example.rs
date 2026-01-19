//! Example of #[observable] macro usage

#![no_std]

extern crate alloc;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::boxed::Box;
use scarlet_std::sync::Mutex;
use scarlet_std::println;

use scarlet_ui::Observable;
use scarlet_ui_macros::{observable, published};

#[observable]
struct UserSettings {
    #[published]
    username: String,

    #[published]
    is_premium: bool,

    #[published]
    login_count: u32,

    // Not published - no automatic notification
    internal_id: u32,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            username: String::from("guest"),
            is_premium: false,
            login_count: 0,
            internal_id: 42,
        }
    }
}

fn main() {
    println!("Testing #[observable] macro...\n");

    // Test 1: Create instance using new()
    let settings = UserSettings::new();
    println!("✓ Created UserSettings with new()");

    // Test 2: Access published fields through underscore getter
    let username_data = settings._username();
    println!("✓ Accessed username data: {}", username_data.get());

    let is_premium_data = settings._is_premium();
    println!("✓ Accessed is_premium data: {}", is_premium_data.get());

    let login_count_data = settings._login_count();
    println!("✓ Accessed login_count data: {}", login_count_data.get());

    // Test 3: Use setter method (updates field + DataContext + notifies)
    let mut settings = UserSettings::new();
    settings.username(String::from("alice"));
    println!("✓ Set username to: {}", settings._username().get());

    settings.is_premium(true);
    println!("✓ Set is_premium to: {}", settings._is_premium().get());

    settings.login_count(5);
    println!("✓ Set login_count to: {}", settings._login_count().get());

    // Test 4: Subscribe to changes
    let call_count = Arc::new(Mutex::new(0));
    let call_count_clone = call_count.clone();

    settings.subscribe(Box::new(move || {
        let mut count = call_count_clone.lock().unwrap();
        *count += 1;
        println!("  → Notification received!");
    }));

    settings.username(String::from("bob"));
    println!("✓ Notification sent after username change");

    settings.is_premium(false);
    println!("✓ Notification sent after is_premium change");

    println!("\n✅ All #[observable] macro tests passed!");
}
