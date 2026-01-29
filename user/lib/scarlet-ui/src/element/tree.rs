//! ElementTree - manages the element tree
//!
//! ElementTree owns the root Element and manages the element lifecycle.
//! StateRegistry is now managed by PipelineOwner to ensure there's only
//! one registry per application.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::element::{Element, ElementId};
use crate::geometry::Size;

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
pub struct ElementTree {
    root: Option<Box<dyn Element>>,
}

impl ElementTree {
    /// Create a new empty ElementTree
    pub fn new() -> Self {
        Self {
            root: None,
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

    /// Find the path of Element IDs from root to the target element.
    pub fn find_path_ids(&self, target: ElementId) -> Option<Vec<ElementId>> {
        let root = self.root.as_deref()?;
        let mut path = Vec::new();
        if Self::find_path_recursive(root, target, &mut path) {
            Some(path)
        } else {
            None
        }
    }

    /// Perform a hit test to find the element at a point
    pub fn hit_test(&self, point: crate::geometry::Point) -> Option<&dyn Element> {
        let root = self.root.as_deref()?;
        self.hit_test_recursive(root, point)
    }

    /// Find an element by its ID
    pub fn find_element_mut(&mut self, id: ElementId) -> Option<&mut Box<dyn Element>> {
        // Use a helper function that doesn't take self
        Self::find_element_recursive_helper(self.root.as_mut()?, id)
    }

    fn find_element_recursive_helper(element: &mut Box<dyn Element>, target_id: ElementId) -> Option<&mut Box<dyn Element>> {
        // Check this element
        if element.id() == target_id {
            return Some(element);
        }

        // Search children
        for child in element.children_mut().iter_mut() {
            if let Some(found) = Self::find_element_recursive_helper(child, target_id) {
                return Some(found);
            }
        }

        None
    }

    fn hit_test_recursive<'a>(&'a self, element: &'a dyn Element, point: crate::geometry::Point) -> Option<&'a dyn Element> {
        let local_point = crate::geometry::Point {
            x: point.x - element.position().x,
            y: point.y - element.position().y,
        };
        // Check children first (reverse order for z-index)
        for child in element.children().iter().rev() {
            if let Some(found) = self.hit_test_recursive(child.as_ref(), local_point) {
                return Some(found);
            }
        }

        // Check this element
        if element.hit_test(point) {
            return Some(element);
        }

        None
    }

    fn find_path_recursive(element: &dyn Element, target: ElementId, path: &mut Vec<ElementId>) -> bool {
        path.push(element.id());
        if element.id() == target {
            return true;
        }

        for child in element.children() {
            if Self::find_path_recursive(child.as_ref(), target, path) {
                return true;
            }
        }

        path.pop();
        false
    }

    /// Dump the element tree structure for debugging
    pub fn dump(&self) {
        if !crate::debug::is_enabled() {
            return;
        }
        scarlet_std::println!("[ElementTree] Dumping element tree:");
        if let Some(root) = self.root.as_deref() {
            self.dump_element(root, 0);
        } else {
            scarlet_std::println!("  (empty)");
        }
        scarlet_std::println!("[ElementTree] End of tree dump");
    }

    fn dump_element(&self, element: &dyn Element, depth: usize) {
        // Build indent string
        let mut indent = alloc::string::String::new();
        for _ in 0..depth {
            indent.push_str("  ");
        }

        let type_name = element.type_name_debug();
        let id = element.id().get();
        let children = element.children();
        let has_buffer = element.get_buffer().is_some();

        scarlet_std::println!(
            "{}[{}] id={} (children={}, buffer={})",
            indent,
            type_name,
            id,
            children.len(),
            has_buffer
        );

        for child in children {
            self.dump_element(child.as_ref(), depth + 1);
        }
    }
}

impl Default for ElementTree {
    fn default() -> Self {
        Self::new()
    }
}
