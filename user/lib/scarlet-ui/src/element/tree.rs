//! ElementTree and StateRegistry - manages the element tree and global state
//!
//! ElementTree owns the root Element and manages the element lifecycle.
//! StateRegistry provides global storage for State instances.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use core::any::Any;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::element::Element;
use crate::geometry::Size;
use crate::state::{State, StateId};

/// Global registry for State instances
///
/// StateRegistry allows the framework to track and access State instances
/// by their unique IDs.
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
    pub fn register<T: 'static + Send + Sync>(&mut self, state: State<T>) -> StateId {
        let id = state.id();
        let boxed: Box<dyn Any + Send + Sync> = Box::new(state);
        self.states.insert(id, boxed);
        id
    }

    /// Get a State reference by ID
    pub fn get<T: 'static>(&self, id: StateId) -> Option<&State<T>> {
        self.states.get(&id)?.downcast_ref::<State<T>>()
    }

    /// Get a mutable State reference by ID
    pub fn get_mut<T: 'static>(&mut self, id: StateId) -> Option<&mut State<T>> {
        self.states.get_mut(&id)?.downcast_mut::<State<T>>()
    }

    /// Remove a State from the registry
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

/// Global counter for generating unique Element IDs
static ELEMENT_ID_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Generate a new unique Element ID
pub fn generate_element_id() -> u32 {
    ELEMENT_ID_COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// ElementTree - owns and manages the element tree
///
/// ElementTree is responsible for:
/// - Owning the root Element
/// - Managing the element lifecycle
/// - Providing access to elements by ID
/// - Coordinating with StateRegistry
pub struct ElementTree {
    root: Option<Box<dyn Element>>,
    registry: StateRegistry,
}

impl ElementTree {
    /// Create a new empty ElementTree
    pub fn new() -> Self {
        Self {
            root: None,
            registry: StateRegistry::new(),
        }
    }

    /// Set the root Element
    ///
    /// This will unmount the previous root (if any) and mount the new root.
    pub fn set_root(&mut self, root: Box<dyn Element>) {
        // Unmount the old root
        if let Some(ref mut old_root) = self.root {
            old_root.unmount();
        }

        // Set and mount the new root
        self.root = Some(root);
        if let Some(ref mut new_root) = self.root {
            new_root.mount();
        }
    }

    /// Get the root Element
    pub fn root(&self) -> Option<&(dyn Element + '_)> {
        self.root.as_deref()
    }

    /// Get mutable reference to the root Element
    ///
    /// Note: This is a simplified version. For full mutable access,
    /// use the layout method or work with the tree directly.
    pub fn root_mut(&mut self) -> Option<&mut Box<dyn Element>> {
        self.root.as_mut()
    }

    /// Get the StateRegistry
    pub fn registry(&self) -> &StateRegistry {
        &self.registry
    }

    /// Get mutable reference to the StateRegistry
    pub fn registry_mut(&mut self) -> &mut StateRegistry {
        &mut self.registry
    }

    /// Layout the entire tree
    ///
    /// This performs a layout pass starting from the root.
    pub fn layout(&mut self, constraints: crate::element::LayoutConstraints) -> Size {
        if let Some(ref mut root) = self.root {
            root.layout(constraints)
        } else {
            Size::ZERO
        }
    }

    /// Perform a hit test to find the element at a point
    pub fn hit_test(&self, point: crate::geometry::Point) -> Option<&dyn Element> {
        let root = self.root.as_deref()?;
        self.hit_test_recursive(root, point)
    }

    fn hit_test_recursive<'a>(&'a self, element: &'a dyn Element, point: crate::geometry::Point) -> Option<&'a dyn Element> {
        // Check children first (reverse order for z-index)
        for child in element.children().iter().rev() {
            if let Some(found) = self.hit_test_recursive(child.as_ref(), point) {
                return Some(found);
            }
        }

        // Check this element
        if element.hit_test(point) {
            return Some(element);
        }

        None
    }
}

impl Default for ElementTree {
    fn default() -> Self {
        Self::new()
    }
}
