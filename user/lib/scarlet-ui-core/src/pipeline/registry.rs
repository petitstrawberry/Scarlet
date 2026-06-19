//! StateRegistry - Global registry for State instances
//!
//! StateRegistry is owned by PipelineOwner and provides centralized
//! storage for all State instances in the application.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use core::any::Any;

use crate::state::{State, StateId};

/// Global registry for State instances
///
/// StateRegistry allows the framework to track and access State instances
/// by their unique IDs. There is only one StateRegistry per application,
/// owned by PipelineOwner.
///
/// # Design
///
/// - State instances are stored in the registry with their StateId as key
/// - When a View creates a State, it should be registered here
/// - During rebuilds, Views retrieve the same State from the registry
/// - This ensures State persists across Element lifecycle changes
pub struct StateRegistry {
    states: BTreeMap<StateId, Box<dyn Any + Send + Sync>>,
}

impl StateRegistry {
    /// Create a new empty StateRegistry
    pub fn new() -> Self {
        Self {
            states: BTreeMap::new(),
        }
    }

    /// Register a State instance
    ///
    /// This should be called when a State is first created, typically
    /// during application initialization or View creation.
    ///
    /// # Returns
    ///
    /// The StateId of the registered State
    pub fn register<T: 'static + Send + Sync>(&mut self, state: State<T>) -> StateId {
        let id = state.id();
        let boxed: Box<dyn Any + Send + Sync> = Box::new(state);
        self.states.insert(id, boxed);
        id
    }

    /// Get a cloned State by ID
    ///
    /// Returns a cloned State<T> if it exists in the registry.
    /// The cloned State points to the same underlying data via Arc.
    ///
    /// # Returns
    ///
    /// - `Some(State<T>)` - A cloned State sharing the same data
    /// - `None` - If no State with this ID exists
    pub fn get<T: 'static + Clone>(&self, id: StateId) -> Option<State<T>> {
        self.states
            .get(&id)
            .and_then(|any| any.downcast_ref::<State<T>>())
            .cloned()
    }

    /// Get a State reference by ID
    ///
    /// This is useful for inspecting a State without cloning.
    pub fn get_ref<T: 'static>(&self, id: StateId) -> Option<&State<T>> {
        self.states.get(&id)?.downcast_ref::<State<T>>()
    }

    /// Remove a State from the registry
    ///
    /// # Returns
    ///
    /// - `true` - If a State was removed
    /// - `false` - If no State with this ID existed
    pub fn remove(&mut self, id: StateId) -> bool {
        self.states.remove(&id).is_some()
    }

    /// Check if a State ID is registered
    pub fn contains(&self, id: StateId) -> bool {
        self.states.contains_key(&id)
    }

    /// Get the number of registered States
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// Check if the registry is empty
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

impl Default for StateRegistry {
    fn default() -> Self {
        Self::new()
    }
}
