//! ComponentElement - wraps Views and manages their lifecycle
//!
//! ComponentElement is the bridge between Views (immutable descriptions) and
//! the element tree (mutable runtime objects).

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

use crate::element::{Element, ElementId, LayoutConstraints, UpdateResult};
use crate::geometry::{Point, Size};
use crate::state::SubscriptionId;
use crate::view::View;

/// Element that wraps a View and manages its lifecycle
///
/// ComponentElement is responsible for:
/// - Owning the View instance
/// - Tracking State subscriptions for rebuilds
/// - Managing child Elements created by the View
pub struct ComponentElement<V: View + Clone> {
    id: ElementId,
    view: V,
    child: Option<Box<dyn Element>>,
    size: Size,
    position: Point,
    last_constraints: Option<LayoutConstraints>,
    subscriptions: Vec<SubscriptionId>,
}

impl<V: View + Clone> ComponentElement<V> {
    /// Create a new ComponentElement with a View
    pub fn new(view: V) -> Self {
        let id = ElementId::generate();
        let child = view.create_element();
        Self {
            id,
            view,
            child: Some(child),
            size: Size::ZERO,
            position: Point::ZERO,
            last_constraints: None,
            subscriptions: Vec::new(),
        }
    }

    /// Get the View
    pub fn view(&self) -> &V {
        &self.view
    }

    /// Get mutable reference to the View
    pub fn view_mut(&mut self) -> &mut V {
        &mut self.view
    }
}

impl<V: View + Clone> Element for ComponentElement<V> {
    fn id(&self) -> ElementId {
        self.id
    }

    fn type_name(&self) -> &str {
        "ComponentElement"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn children(&self) -> &[Box<dyn Element>] {
        match &self.child {
            Some(child) => core::slice::from_ref(child),
            None => &[],
        }
    }

    fn children_mut(&mut self) -> &mut [Box<dyn Element>] {
        match &mut self.child {
            Some(child) => core::slice::from_mut(child),
            None => &mut [],
        }
    }

    fn update(&mut self, new_view: &dyn View) -> UpdateResult {
        // Type-checking reconciliation: check if the new View is the same type
        // Use Any's type_id method to avoid ambiguity
        if Any::type_id(new_view) != Any::type_id(&self.view) {
            // Different type - signal that this Element should be replaced
            return UpdateResult::Replaced;
        }

        // Same type - try to downcast and update properties
        if let Some(new_typed_view) = new_view.as_any().downcast_ref::<V>() {
            // Note: We can't use == since V may not implement PartialEq
            // For now, we'll assume the view has changed and update it

            // Update our stored view
            self.view = new_typed_view.clone();

            // Create new child element
            let new_child = self.view.create_element();

            // Replace the old child with the new one
            self.child = Some(new_child);

            UpdateResult::Updated
        } else {
            // Should never happen since we checked type_id above
            // but if it does, signal replacement
            UpdateResult::Replaced
        }
    }

    fn rebuild(&mut self) -> UpdateResult {
        // Unmount the old child
        if let Some(ref mut child) = self.child {
            child.unmount();
        }

        // Create new child element from the stored view
        let new_child = self.view.create_element();

        // Replace the old child with the new one
        self.child = Some(new_child);

        // Mount the new child
        if let Some(ref mut child) = self.child {
            child.mount();
        }

        UpdateResult::Updated
    }

    fn mount(&mut self) {
        // Subscribe to all Listenables from the View
        let listenables = self.view.listenables();

        // For each listenable, subscribe to rebuild notifications
        // Store the subscription IDs so we can unsubscribe later
        for listenable in listenables {
            let element_id = self.id;
            let callback = Arc::new(move || {
                // Mark this element as dirty via the PipelineOwner's global callback
                // This will notify PipelineOwner to schedule a rebuild
                crate::pipeline::mark_element_dirty(element_id);
            });
            let subscription_id = listenable.subscribe_any(callback);
            self.subscriptions.push(subscription_id);
        }

        // Mount the child
        if let Some(ref mut child) = self.child {
            child.mount();
        }
    }

    fn unmount(&mut self) {
        // Unmount the child
        if let Some(ref mut child) = self.child {
            child.unmount();
        }

        // Unsubscribe from all Listenables
        // Note: We need to get the listenables again to call unsubscribe
        // In the current design, State doesn't store a back-reference to subscribers,
        // so we need to keep track of subscriptions differently
        // For now, we'll clear the subscription list
        // TODO: Implement proper cleanup when we have access to State references
        self.subscriptions.clear();
    }

    fn layout(&mut self, constraints: LayoutConstraints) -> Size {
        self.last_constraints = Some(constraints);
        // Delegate layout to the child
        if let Some(ref mut child) = self.child {
            self.size = child.layout(constraints);
        } else {
            self.size = Size::ZERO;
        }
        self.size
    }

    fn last_layout_constraints(&self) -> Option<LayoutConstraints> {
        self.last_constraints
    }

    fn set_last_layout_constraints(&mut self, constraints: LayoutConstraints) {
        self.last_constraints = Some(constraints);
    }

    fn position(&self) -> Point {
        self.position
    }

    fn set_position(&mut self, position: Point) {
        self.position = position;
        // Update child position
        if let Some(ref mut child) = self.child {
            child.set_position(position);
        }
    }

    fn bounds(&self) -> crate::geometry::Rect {
        crate::geometry::Rect {
            origin: self.position,
            size: self.size,
        }
    }

    fn hit_test(&self, point: Point) -> bool {
        // Delegate to child
        if let Some(ref child) = self.child {
            child.hit_test(point)
        } else {
            self.bounds().contains(point)
        }
    }

    fn handle_event(&mut self, event: &crate::event::Event, phase: crate::event::Phase) -> bool {
        // Delegate to child
        if let Some(ref mut child) = self.child {
            child.handle_event(event, phase)
        } else {
            false
        }
    }
}
