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
use crate::pipeline::{MountContext, PipelineId};
use crate::state::{InvalidationKind, SubscriptionId};
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
    build_child: fn(&V) -> Box<dyn Element>,
    child: Option<Box<dyn Element>>,
    size: Size,
    position: Point,
    last_constraints: Option<LayoutConstraints>,
    subscriptions: Vec<SubscriptionId>,
    pipeline_id: PipelineId,
}

impl<V: View + Clone> ComponentElement<V> {
    /// Create a new ComponentElement with a View
    pub fn new(view: V) -> Self {
        Self::new_with_builder(view, default_component_child::<V>)
    }

    /// Create a new ComponentElement with an explicit child builder.
    ///
    /// # Arguments
    ///
    /// * `view` - View value owned by this component.
    /// * `build_child` - Function that creates the child element for `view`.
    ///
    /// # Returns
    ///
    /// Component element that subscribes to `view` listenables and rebuilds
    /// children through `build_child`.
    pub fn new_with_builder(view: V, build_child: fn(&V) -> Box<dyn Element>) -> Self {
        let id = ElementId::generate();
        let child = build_child(&view);
        Self {
            id,
            view,
            build_child,
            child: Some(child),
            size: Size::ZERO,
            position: Point::ZERO,
            last_constraints: None,
            subscriptions: Vec::new(),
            pipeline_id: PipelineId::default(),
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

fn default_component_child<V: View + Clone>(view: &V) -> Box<dyn Element> {
    view.create_element()
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

            let focused_path = self
                .child
                .as_ref()
                .and_then(|child| crate::element::focused_descendant_path(child.as_ref()));

            if let Some(ref mut child) = self.child {
                child.unmount();
            }

            // Create new child element
            let mut new_child = (self.build_child)(&self.view);
            let ctx = MountContext::new(self.pipeline_id);
            new_child.mount(&ctx);

            // Replace the old child with the new one
            self.child = Some(new_child);
            if let (Some(path), Some(child)) = (focused_path.as_deref(), self.child.as_mut()) {
                crate::element::restore_focus_at_path(child.as_mut(), path);
            }

            UpdateResult::Updated
        } else {
            // Should never happen since we checked type_id above
            // but if it does, signal replacement
            UpdateResult::Replaced
        }
    }

    fn rebuild(&mut self) -> UpdateResult {
        let focused_path = self
            .child
            .as_ref()
            .and_then(|child| crate::element::focused_descendant_path(child.as_ref()));

        if let Some(ref mut child) = self.child {
            match child.update(&self.view) {
                UpdateResult::NoChange => return UpdateResult::NoChange,
                UpdateResult::Updated => {
                    crate::pipeline::mark_element_needs_layout(self.pipeline_id, child.id());
                    return UpdateResult::NoChange;
                }
                UpdateResult::Replaced => {
                    child.unmount();
                }
            }
        }

        // Create new child element from the stored view
        let new_child = (self.build_child)(&self.view);

        // Replace the old child with the new one
        self.child = Some(new_child);

        // Mount the new child
        if let Some(ref mut child) = self.child {
            let ctx = MountContext::new(self.pipeline_id);
            child.mount(&ctx);
            if let Some(path) = focused_path.as_deref() {
                crate::element::restore_focus_at_path(child.as_mut(), path);
            }
        }

        if let Some(ref child) = self.child {
            crate::pipeline::mark_element_needs_layout(self.pipeline_id, child.id());
        }
        UpdateResult::NoChange
    }

    fn mount(&mut self, ctx: &MountContext) {
        self.pipeline_id = ctx.pipeline_id();
        // Subscribe to all Listenables from the View
        let listenables = self.view.listenables();

        // For each listenable, subscribe to rebuild notifications
        // Store the subscription IDs so we can unsubscribe later
        for listenable in listenables {
            let element_id = self.id;
            let pipeline_id = self.pipeline_id;
            let invalidation_kind = listenable.invalidation_kind();
            let callback = Arc::new(move || match invalidation_kind {
                InvalidationKind::Build => {
                    crate::pipeline::mark_element_dirty(pipeline_id, element_id)
                }
                InvalidationKind::Paint => {
                    crate::pipeline::mark_element_needs_paint(pipeline_id, element_id)
                }
            });
            let subscription_id = listenable.subscribe_any(callback);
            self.subscriptions.push(subscription_id);
        }

        // Mount the child
        if let Some(ref mut child) = self.child {
            child.mount(ctx);
        }
    }

    fn unmount(&mut self) {
        // Unmount the child first
        if let Some(ref mut child) = self.child {
            child.unmount();
        }

        // Unsubscribe from all Listenables to prevent stale callbacks
        let listenables = self.view.listenables();
        for (listenable, subscription_id) in listenables.iter().zip(self.subscriptions.iter()) {
            listenable.unsubscribe(*subscription_id);
        }
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

    fn take_window_action(&mut self) -> Option<crate::event::WindowEvent> {
        self.child
            .as_mut()
            .and_then(|child| child.take_window_action())
    }

    fn fill_width(&self) -> bool {
        self.child
            .as_ref()
            .map(|child| child.fill_width())
            .unwrap_or(false)
    }

    fn fill_height(&self) -> bool {
        self.child
            .as_ref()
            .map(|child| child.fill_height())
            .unwrap_or(false)
    }
}
