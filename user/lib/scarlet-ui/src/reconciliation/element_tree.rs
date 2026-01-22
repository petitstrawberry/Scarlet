use std::boxed::Box;
use std::vec::Vec;

use crate::traits::{Element, ElementId, View};
use crate::GenericElement;

/// ElementTree manages the tree of Elements
pub struct ElementTree {
    root: Option<Box<dyn Element>>,
}

impl ElementTree {
    pub fn new() -> Self {
        Self { root: None }
    }

    pub fn root(&self) -> Option<&dyn Element> {
        self.root.as_ref().map(|b| b.as_ref())
    }

    pub fn root_mut(&mut self) -> Option<&mut dyn Element> {
        match &mut self.root {
            Some(b) => Some(b.as_mut()),
            None => None,
        }
    }

    /// Reconcile the element tree with a new view
    pub fn reconcile(&mut self, new_view: Box<dyn View>) {
        if let Some(ref mut root) = self.root {
            // Try to update existing root
            use crate::ElementUpdateResult;

            match root.update(new_view.as_ref()) {
                ElementUpdateResult::Unchanged => {
                    // No changes needed
                }
                ElementUpdateResult::Changed(_flags) => {
                    // Element was updated, flags are already set
                }
                ElementUpdateResult::Replaced => {
                    // Need to replace the entire tree
                    self.replace_root(new_view);
                }
            }
        } else {
            // No root exists, build new tree
            self.replace_root(new_view);
        }
    }

    /// Replace the entire root element
    fn replace_root(&mut self, new_view: Box<dyn View>) {
        let mut root = Self::build_element_tree(new_view);
        root.mount(None);
        self.root = Some(root);
    }

    /// Build an element tree from a view
    fn build_element_tree(view: Box<dyn View>) -> Box<dyn Element> {
        // Create the element for this view
        let mut element = Self::create_element(view.as_ref());

        // Recursively build children using view.children()
        let view_children = view.children();
        for child_view in view_children {
            let child_element = Self::build_element_tree(child_view);
            element.add_child(child_element);
        }

        element
    }

    /// Create an element from a view
    fn create_element(view: &dyn View) -> Box<dyn Element> {
        // For now, only handle leaf views that don't have generic parameters
        // Container views will be handled in Phase 4 (Element Factory)
        let type_id = view.type_id();
        let type_name = view.type_name();

        // Import leaf view types
        use crate::views::{Text, Rectangle, Button, Toggle, Slider, TextField, Image};

        // Match on view type and create appropriate element
        if let Some(text) = view.as_any().downcast_ref::<Text>() {
            Box::new(GenericElement::new(text.clone()))
        } else if let Some(rect) = view.as_any().downcast_ref::<Rectangle>() {
            Box::new(GenericElement::new(rect.clone()))
        } else if let Some(button) = view.as_any().downcast_ref::<Button>() {
            Box::new(GenericElement::new(button.clone()))
        } else if let Some(toggle) = view.as_any().downcast_ref::<Toggle>() {
            Box::new(GenericElement::new(toggle.clone()))
        } else if let Some(slider) = view.as_any().downcast_ref::<Slider>() {
            Box::new(GenericElement::new(slider.clone()))
        } else if let Some(text_field) = view.as_any().downcast_ref::<TextField>() {
            Box::new(GenericElement::new(text_field.clone()))
        } else if let Some(image) = view.as_any().downcast_ref::<Image>() {
            Box::new(GenericElement::new(image.clone()))
        } else {
            // Placeholder for container views - will be implemented in Phase 4
            panic!("Container view type not yet supported: {} (typeid: {:?}). Will be implemented in Phase 4 (Element Factory).", type_name, type_id);
        }
    }

    /// Find an element by ID
    pub fn find_element(&self, id: ElementId) -> Option<&dyn Element> {
        self.root.as_ref().and_then(|r| Self::find_in_element(r.as_ref(), id))
    }

    /// Find an element by ID (mutable)
    pub fn find_element_mut(&mut self, id: ElementId) -> Option<&mut dyn Element> {
        self.root.as_mut().and_then(|r| Self::find_in_element_mut(r.as_mut(), id))
    }

    fn find_in_element(element: &dyn Element, id: ElementId) -> Option<&dyn Element> {
        if element.id() == id {
            return Some(element);
        }

        for child in element.children() {
            if let Some(found) = Self::find_in_element(child.as_ref(), id) {
                return Some(found);
            }
        }

        None
    }

    fn find_in_element_mut(element: &mut dyn Element, id: ElementId) -> Option<&mut dyn Element> {
        if element.id() == id {
            return Some(element);
        }

        for child in element.children_mut() {
            if let Some(found) = Self::find_in_element_mut(child.as_mut(), id) {
                return Some(found);
            }
        }

        None
    }
}

impl Default for ElementTree {
    fn default() -> Self {
        Self::new()
    }
}
