use std::any::TypeId;
use std::boxed::Box;
use std::vec::Vec;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::dirty::DirtyFlags;
use crate::traits::render_object::RenderObject;
use crate::traits::view::View;

/// Element manages the lifecycle of a RenderObject
pub trait Element {
    /// Unique identifier for this element
    fn id(&self) -> ElementId;

    /// The TypeId of the View that created this element
    fn view_type_id(&self) -> TypeId;

    /// Get the RenderObject owned by this element
    fn get_render_object(&self) -> &dyn RenderObject;

    /// Get mutable reference to the RenderObject owned by this element
    fn get_render_object_mut(&mut self) -> &mut dyn RenderObject;

    /// Update the RenderObject based on a new View
    fn update(&mut self, view: &dyn View) -> ElementUpdateResult;

    /// Get child elements
    fn children(&self) -> &[Box<dyn Element>];

    /// Get mutable reference to child elements
    fn children_mut(&mut self) -> &mut [Box<dyn Element>];

    /// Mount this element into the tree with an optional parent
    fn mount(&mut self, parent: Option<ElementId>);

    /// Unmount this element from the tree
    fn unmount(&mut self);

    /// Add a child element
    fn add_child(&mut self, child: Box<dyn Element>);
}

/// Result of updating an element with a new View
pub enum ElementUpdateResult {
    /// No changes needed
    Unchanged,
    /// Element changed, marked with dirty flags
    Changed(DirtyFlags),
    /// Element needs to be replaced
    Replaced,
}

/// Unique identifier for Elements
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ElementId(pub usize);

impl ElementId {
    pub fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(1);
        ElementId(COUNTER.fetch_add(1, Ordering::SeqCst))
    }
}

impl Default for ElementId {
    fn default() -> Self {
        Self::new()
    }
}

/// Generic implementation of Element for any View type
pub struct GenericElement<V: View + Clone> {
    id: ElementId,
    parent: Option<ElementId>,
    view: V,
    render_object: Box<dyn RenderObject>,
    children: Vec<Box<dyn Element>>,
}

impl<V: View + Clone> GenericElement<V> {
    pub fn new(view: V) -> Self {
        let render_object = view.build();
        Self {
            id: ElementId::new(),
            parent: None,
            view,
            render_object,
            children: Vec::new(),
        }
    }
}

impl<V: View + Clone> Element for GenericElement<V> {
    fn id(&self) -> ElementId {
        self.id
    }

    fn view_type_id(&self) -> TypeId {
        self.view.type_id()
    }

    fn get_render_object(&self) -> &dyn RenderObject {
        self.render_object.as_ref()
    }

    fn get_render_object_mut(&mut self) -> &mut dyn RenderObject {
        self.render_object.as_mut()
    }

    fn update(&mut self, view: &dyn View) -> ElementUpdateResult {
        // Check if the view type matches
        if view.type_id() != self.view_type_id() {
            return ElementUpdateResult::Replaced;
        }

        // Try to downcast and update the view
        if let Some(typed_view) = view.as_any().downcast_ref::<V>() {
            self.view = typed_view.clone();

            // Try to update the render object
            match self.render_object.update(view) {
                crate::traits::render_object::UpdateResult::Unchanged => {
                    ElementUpdateResult::Unchanged
                }
                crate::traits::render_object::UpdateResult::Changed(flags) => {
                    ElementUpdateResult::Changed(flags)
                }
                crate::traits::render_object::UpdateResult::Replaced(new_render_object) => {
                    self.render_object = new_render_object;
                    ElementUpdateResult::Changed(DirtyFlags::all())
                }
            }
        } else {
            ElementUpdateResult::Replaced
        }
    }

    fn children(&self) -> &[Box<dyn Element>] {
        &self.children
    }

    fn children_mut(&mut self) -> &mut [Box<dyn Element>] {
        &mut self.children
    }

    fn mount(&mut self, parent: Option<ElementId>) {
        self.parent = parent;
    }

    fn unmount(&mut self) {
        self.parent = None;
    }

    fn add_child(&mut self, child: Box<dyn Element>) {
        self.children.push(child);
    }
}
