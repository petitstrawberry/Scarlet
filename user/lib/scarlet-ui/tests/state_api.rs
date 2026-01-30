//! Test the new State API

use scarlet_ui::{State, StateId};

#[test]
fn test_state_new_with_id_and_value() {
    let state = State::new(StateId::new(1), 42);
    assert_eq!(state.id(), StateId::new(1));
    assert_eq!(state.get(), 42);
}

#[test]
fn test_state_initial_with_default() {
    let state: State<i32> = State::initial(StateId::new(2));
    assert_eq!(state.id(), StateId::new(2));
    assert_eq!(state.get(), 0); // i32::default() is 0
}

#[test]
fn test_state_initial_with_string() {
    let state: State<String> = State::initial(StateId::new(3));
    assert_eq!(state.id(), StateId::new(3));
    assert_eq!(state.get(), ""); // String::default() is ""
}

#[test]
fn test_state_set_and_update() {
    let state = State::new(StateId::new(4), 10);
    assert_eq!(state.get(), 10);

    state.set(20);
    assert_eq!(state.get(), 20);

    state.update(|v| *v += 5);
    assert_eq!(state.get(), 25);
}

#[test]
fn test_state_subscriptions() {
    let state = State::new(StateId::new(5), 0);
    let state_clone = state.clone();

    // Test subscription callback
    let mut call_count = 0;
    state.subscribe(move |value| {
        call_count += 1;
        assert_eq!(*value, 42);
    });

    state_clone.set(42);
    // Note: In a real test, we'd need to ensure the notification was delivered
    // This is just to verify the API compiles
}
