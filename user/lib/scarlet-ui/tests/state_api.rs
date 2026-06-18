//! Test the new State API

use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use scarlet_ui::{Listenable, State, StateId};

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
    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_for_callback = Arc::clone(&call_count);
    let _subscription = state.subscribe_any(Arc::new(move || {
        call_count_for_callback.fetch_add(1, Ordering::SeqCst);
    }));

    state.set(42);
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}
