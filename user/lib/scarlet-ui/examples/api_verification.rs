//! API Verification Example for DirtyFlags and State API
//!
//! This example demonstrates the correct usage of the new API according to design.md

#![no_std]
#![no_main]

extern crate alloc;
extern crate scarlet_std as std;

use alloc::string::String;
use scarlet_ui::{State, StateId, DirtyFlags};

/// Example 1: Using State::new() with explicit ID and value
fn example_state_new() {
    // Create a state with explicit ID and value
    let counter = State::new(StateId::new(1), 42i32);

    // Verify initial value
    assert_eq!(counter.get(), 42);
    assert_eq!(counter.id(), StateId::new(1));

    // Update the value
    counter.set(100);
    assert_eq!(counter.get(), 100);

    // Update in place
    counter.update(|v| *v += 10);
    assert_eq!(counter.get(), 110);
}

/// Example 2: Using State::initial() with Default types
fn example_state_initial() {
    // For types that implement Default, we can use initial()
    let counter: State<i32> = State::initial(StateId::new(2));
    assert_eq!(counter.get(), 0); // i32::default() == 0

    let text: State<String> = State::initial(StateId::new(3));
    assert_eq!(text.get(), ""); // String::default() == ""

    // Vec also implements Default
    let items: State<alloc::vec::Vec<i32>> = State::initial(StateId::new(4));
    assert!(items.get().is_empty()); // Vec::default() == []
}

/// Example 3: Working with DirtyFlags
fn example_dirty_flags() {
    // Start with empty flags
    let mut flags = DirtyFlags::empty();
    assert!(flags.is_empty());

    // Mark as needing rebuild
    flags |= DirtyFlags::BUILD;
    assert!(flags.contains(DirtyFlags::BUILD));
    assert!(!flags.contains(DirtyFlags::LAYOUT));

    // Mark as also needing layout
    flags |= DirtyFlags::LAYOUT;
    assert!(flags.contains(DirtyFlags::BUILD));
    assert!(flags.contains(DirtyFlags::LAYOUT));

    // Clear the build flag
    flags -= DirtyFlags::BUILD;
    assert!(!flags.contains(DirtyFlags::BUILD));
    assert!(flags.contains(DirtyFlags::LAYOUT));

    // Combine multiple flags at once
    let all_flags = DirtyFlags::BUILD | DirtyFlags::LAYOUT | DirtyFlags::PAINT | DirtyFlags::CHILDREN;
    assert!(all_flags.contains(DirtyFlags::BUILD));
    assert!(all_flags.contains(DirtyFlags::LAYOUT));
    assert!(all_flags.contains(DirtyFlags::PAINT));
    assert!(all_flags.contains(DirtyFlags::CHILDREN));
}

/// Example 4: Using State with subscriptions
fn example_state_subscriptions() {
    let state = State::new(StateId::new(5), 0i32);
    let state_clone = state.clone();

    // Subscribe to changes
    state.subscribe(move |value| {
        println!("State changed to: {}", value);
    });

    // This will trigger the callback
    state_clone.set(42);
}

/// Example 5: State can be cloned and shared
fn example_state_cloning() {
    let original = State::new(StateId::new(6), 10i32);
    let clone1 = original.clone();
    let clone2 = original.clone();

    // All clones share the same underlying value
    assert_eq!(original.get(), 10);
    assert_eq!(clone1.get(), 10);
    assert_eq!(clone2.get(), 10);

    // Updating any clone affects all of them
    clone1.set(20);

    assert_eq!(original.get(), 20);
    assert_eq!(clone1.get(), 20);
    assert_eq!(clone2.get(), 20);
}

/// Example 6: Default implementation for DirtyFlags
fn example_dirty_flags_default() {
    // DirtyFlags implements Default
    let flags: DirtyFlags = Default::default();
    assert!(flags.is_empty());

    // You can also use DirtyFlags::empty() directly
    let flags2 = DirtyFlags::empty();
    assert!(flags2.is_empty());
}

#[no_mangle]
pub extern "C" fn main() {
    example_state_new();
    example_state_initial();
    example_dirty_flags();
    example_state_subscriptions();
    example_state_cloning();
    example_dirty_flags_default();

    println!("All API verification examples passed!");
}
